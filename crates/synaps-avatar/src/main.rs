//! synaps-avatar: a desktop companion overlay. A transparent, click-through,
//! always-on-top window animates a character pack; one Synaps daemon session drives it
//! (moods from turn events, the reply in the bubble, a text field that submits to the
//! session). A dev control socket takes commands for testing and overrides.

mod anim;
mod control;
mod frames;
mod fx;
mod pack;
mod platform;
mod session;
mod shape;
mod ticker;
mod state;
mod view;

use anyhow::{Context as _, Result, bail};
use gpui_kit::*;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

struct Args {
    pack: PathBuf,
    height: f32,
    verify: bool,
    focus_probe: bool,
    name: Option<String>,
    threads: usize,
    check: bool,
    /// Daemon profile (`None`: `$SYNAPS_PROFILE`, else the default daemon).
    profile: Option<String>,
    target: session::Target,
    cwd: Option<PathBuf>,
    model: Option<String>,
    no_session: bool,
    sleepy_after: std::time::Duration,
}

const USAGE: &str = "usage: synaps-avatar [--profile P] [--session NAME|ID | --new [--cwd DIR] [--model M]] [--no-session]
                     [--sleepy-after MIN] [--pack DIR] [--height PX] [--name NAME] [--threads N] [--no-verify]
                     [--focus-probe] [--check]

Session (one Synaps daemon session drives the body):
  --profile P     daemon profile (default $SYNAPS_PROFILE, else the default daemon)
  --session S     attach to this live session (name or id)
  --new           create a new session (--cwd: its working directory, absolute;
                  --model: its model); afterwards follow it
                  with neither: attach to the session named `avatar`, or create it
  --no-session    the body alone (dev control only)
  --sleepy-after MIN   sleepy after this many minutes without a turn (default 10)

  --pack DIR      character pack (default $XDG_DATA_HOME/synaps-avatar/packs/capri)
  --height PX     standing body height in logical pixels (default 340)
  --name NAME     name shown in the HUD (default: the pack's character name)
  --threads N     decode threads (default min(8, cores/2))
  --no-verify     skip the sha256 pass over the pack's frames
  --focus-probe   dev: show a text field in the bubble to test keyboard focus
  --check         validate and decode the pack, print stats, exit (no window)

Dev control: newline commands on $XDG_RUNTIME_DIR/synaps-avatar.sock
  mood <idle|thinking|working|talking|needs_you|done|annoyed|sleepy|away>
  clip <clip, action or frame id>   say <text>   page next|prev   collapse   expand
  pos <x> <y> [now]   perch [sit|stand] [window id]   perch off
  music on|off   glitch   state   hide   quit
  submit <text>   abort   prompt confirm|secret [text]   (session / dev)";

fn parse_args() -> Result<Args> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_default();
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let mut a = Args {
        pack: data.join("synaps-avatar/packs/capri"),
        height: 340.0,
        verify: true,
        focus_probe: false,
        name: None,
        threads: (cores / 2).clamp(1, 8),
        check: false,
        profile: None,
        target: session::Target::Default,
        cwd: None,
        model: None,
        no_session: false,
        sleepy_after: std::time::Duration::from_secs(600),
    };
    let (mut session, mut new) = (None, false);
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = |what: &str| it.next().with_context(|| format!("{arg} needs {what}"));
        match arg.as_str() {
            "--pack" => a.pack = PathBuf::from(val("a directory")?),
            "--height" => a.height = val("pixels")?.parse().context("--height: not a number")?,
            "--name" => a.name = Some(val("a name")?),
            "--threads" => a.threads = val("a count")?.parse().context("--threads: not a number")?,
            "--no-verify" => a.verify = false,
            "--focus-probe" => a.focus_probe = true,
            "--check" => a.check = true,
            "--profile" => a.profile = Some(val("a profile name")?),
            "--session" => session = Some(val("a session name or id")?),
            "--new" => new = true,
            "--cwd" => a.cwd = Some(PathBuf::from(val("a directory")?)),
            "--model" => a.model = Some(val("a model")?),
            "--no-session" => a.no_session = true,
            "--sleepy-after" => {
                let m: f64 = val("minutes")?.parse().context("--sleepy-after: not a number")?;
                if !(m > 0.0 && m < 1e6) {
                    bail!("--sleepy-after must be a positive number of minutes");
                }
                a.sleepy_after = std::time::Duration::from_secs_f64(m * 60.0);
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            _ => bail!("unknown argument `{arg}`\n{USAGE}"),
        }
    }
    if !(120.0..=1200.0).contains(&a.height) {
        bail!("--height {} is out of range (120..1200)", a.height);
    }
    a.target = match (session, new) {
        (Some(_), true) => bail!("--session and --new exclude each other"),
        (Some(s), false) => {
            if a.cwd.is_some() || a.model.is_some() {
                bail!("--cwd and --model apply to a session the avatar creates, not to --session");
            }
            session::Target::Session(s)
        }
        (None, true) => session::Target::New,
        (None, false) => session::Target::Default,
    };
    if a.cwd.as_ref().is_some_and(|c| !c.is_absolute()) {
        bail!("--cwd must be an absolute path (it is resolved on the daemon's machine)");
    }
    Ok(a)
}

/// `SA_LOG=debug|info|warn` prints GPUI's own log lines to stderr.
struct StderrLog;
impl log::Log for StderrLog {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, r: &log::Record) {
        eprintln!("[{} {}] {}", r.level(), r.target(), r.args());
    }
    fn flush(&self) {}
}

/// Process setup that has to happen before any other thread exists. Each knob was
/// measured on bella (release, `ps` RSS 6 s after start, see LOG.md):
///
/// * Render on the boot VGA device (the iGPU on a hybrid laptop) unless `ZED_DEVICE_ID` is
///   set. On bella every vkQueuePresent to this 32-bit override-redirect window
///   busy-waited ~2.3 s on the NVIDIA dGPU (0.4 fps); the Intel iGPU presents at once. A
///   desktop pet belongs on the low-power GPU anyway.
/// * Keep GPUI's GL fallback from probing EGL when a Vulkan driver is installed. The probe
///   loads every EGL vendor (NVIDIA's, Mesa's llvmpipe with LLVM) and 50 threads that stay
///   for the life of the process: 371 → 226 MB RSS. `SA_GL=1` keeps it.
/// * glibc malloc: two arenas and a 1 MiB trim threshold, so the full-size PNG decode
///   buffers go back to the OS after scaling: 226 → 84 MB.
fn process_setup() {
    if std::env::var_os("SA_GL").is_none() && has_vulkan_driver() {
        // SAFETY (all set_var calls here): first thing in main, single-threaded.
        unsafe { std::env::set_var("__EGL_VENDOR_LIBRARY_DIRS", "/nonexistent") };
    }
    #[cfg(target_env = "gnu")]
    // SAFETY: plain libc calls with valid parameters.
    unsafe {
        libc::mallopt(libc::M_ARENA_MAX, 2);
        libc::mallopt(libc::M_TRIM_THRESHOLD, 1 << 20);
    }
    if std::env::var_os("ZED_DEVICE_ID").is_some() || std::env::var_os("SA_ANY_GPU").is_some() {
        return;
    }
    let read = |p: std::path::PathBuf| std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default();
    let mut cards = Vec::new();
    for e in std::fs::read_dir("/sys/class/drm").into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let dev = e.path().join("device");
        cards.push((read(dev.join("device")), read(dev.join("boot_vga")) == "1"));
    }
    if cards.len() < 2 {
        return;
    }
    if let Some((id, _)) = cards.iter().find(|c| c.1 && !c.0.is_empty()) {
        eprintln!("synaps-avatar: {} GPUs, rendering on the boot VGA device {id} (SA_ANY_GPU=1 to let GPUI pick)", cards.len());
        unsafe { std::env::set_var("ZED_DEVICE_ID", id) };
    }
}

/// Whether the Vulkan loader will find at least one driver manifest.
fn has_vulkan_driver() -> bool {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    for var in ["VK_DRIVER_FILES", "VK_ICD_FILENAMES"] {
        if let Some(v) = std::env::var_os(var) {
            return std::env::split_paths(&v).any(|p| p.exists());
        }
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.extend(data_dirs.split(':').map(|d| std::path::Path::new(d).join("vulkan/icd.d")));
    dirs.push("/etc/vulkan/icd.d".into());
    dirs.iter().any(|d| std::fs::read_dir(d).into_iter().flatten().flatten().any(|e| e.path().extension().is_some_and(|x| x == "json")))
}

fn main() {
    let t_start = Instant::now();
    process_setup();
    if let Ok(level) = std::env::var("SA_LOG")
        && let Ok(level) = level.parse::<log::LevelFilter>()
    {
        log::set_logger(&StderrLog).ok();
        log::set_max_level(level);
    }
    if let Err(e) = run(t_start) {
        eprintln!("synaps-avatar: {e:#}");
        std::process::exit(1);
    }
}

fn run(t_start: Instant) -> Result<()> {
    let args = parse_args()?;
    let pack = Arc::new(pack::Pack::load(&args.pack)?);
    if args.verify {
        frames::verify_poses(&pack)?;
    }
    let t_pack = t_start.elapsed().as_millis();

    if args.check {
        let layout = view::Layout::new(&pack, 1.0, args.height);
        let all: Vec<usize> = (0..pack.frames.len()).collect();
        let d = frames::decode(&pack, &layout.geo, &all, args.verify, args.threads)?;
        let bytes: usize = d.frames.len() * (layout.geo.width * layout.geo.height * 4) as usize;
        let rects: Vec<usize> = d.frames.iter().map(|f| f.rects.len()).collect();
        println!(
            "pack `{}` ok: {} frames + {} poses, crop {:?}, frame {}x{} px, {:.1} MB scaled RGBA, \
             shape rects/frame {}..{}, load {t_pack} ms, decode {} ms ({} threads, verify={})",
            pack.name,
            pack.frames.len(),
            pack.poses.len(),
            pack.crop_rect(),
            layout.geo.width,
            layout.geo.height,
            bytes as f64 / 1e6,
            rects.iter().min().unwrap_or(&0),
            rects.iter().max().unwrap_or(&0),
            d.decode_ms,
            args.threads,
            args.verify
        );
        return Ok(());
    }

    let sock = control::socket_path();
    let listener = control::bind(&sock)?;
    let mut x11 = platform::x11::X11::connect()?;
    let fonts: Vec<Cow<'static, [u8]>> = pack
        .behaviour
        .font_files
        .iter()
        .filter_map(|f| std::fs::read(pack.dir.join(f)).ok().map(Cow::Owned))
        .collect();
    let name = args.name.clone().unwrap_or_else(|| pack.name.clone());

    gpui_kit::application().run(move |cx: &mut App| {
        if !fonts.is_empty()
            && let Err(e) = cx.text_system().add_fonts(fonts)
        {
            eprintln!("synaps-avatar: pack fonts not loaded: {e:#}");
        }
        // GPUI's X11 displays are the root size divided by its scale factor.
        let display_w = cx.primary_display().map(|d| f32::from(d.bounds().size.width)).unwrap_or(0.0);
        let sf = if display_w > 0.0 { x11.screen_w as f32 / display_w } else { 1.0 };
        let layout = view::Layout::new(&pack, sf, args.height);
        let origin = state::load()
            .map(|(gx, gy)| view::Avatar::clamp(&x11, &layout, (gx - layout.ground_x, gy - layout.ground_y)))
            .unwrap_or_else(|| view::Avatar::default_origin(&x11, &layout));

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(origin.0 as f32 / sf), px(origin.1 as f32 / sf)),
                size(px(layout.win_w as f32 / sf), px(layout.win_h as f32 / sf)),
            ))),
            titlebar: Some(TitlebarOptions { title: Some("synaps-avatar".into()), ..Default::default() }),
            focus: false,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            window_background: WindowBackgroundAppearance::Transparent,
            app_id: Some("synaps-avatar".into()),
            ..Default::default()
        };

        let (tx, rx) = async_channel::unbounded::<(control::Command, control::Reply)>();
        let mut built = None;
        let pack2 = pack.clone();
        let tx2 = tx.clone();
        let opened = cx.open_window(options, |window, cx| {
            let win = match HasWindowHandle::window_handle(&*window).map(|h| h.as_raw()) {
                Ok(RawWindowHandle::Xcb(h)) => h.window.get(),
                Ok(RawWindowHandle::Xlib(h)) => h.window as u32,
                other => {
                    eprintln!("synaps-avatar: not an X11 window ({other:?}); phase 1 supports X11 only");
                    std::process::exit(1);
                }
            };
            x11.win = win;
            let x11 = Arc::new(x11);
            // Nothing clickable until the first frame is decoded. Keep one pixel visible:
            // an empty bounding shape makes X report the window fully obscured, and GPUI
            // stops drawing a window it thinks is hidden.
            x11.set_compositor_hints().ok();
            let px1 = (layout.ground_x as i16, (layout.ground_y - 1) as i16, 1, 1);
            x11.set_shape(&[], &[px1]).ok();
            x11.move_to(origin.0, origin.1).ok();
            x11.raise().ok();
            let tx = tx2.clone();
            x11.spawn_watcher(move |ev| {
                let (r, _) = std::sync::mpsc::channel();
                tx.send_blocking((control::Command::Watch(ev), r)).ok();
            })
            .map_err(|e| eprintln!("synaps-avatar: x11 watcher: {e:#}"))
            .ok();
            eprintln!(
                "synaps-avatar: pack `{}` ({} frames) sf {sf:.3} window 0x{win:x} {}x{} at {},{} composited={} (pack load {t_pack} ms)",
                pack2.name,
                pack2.frames.len(),
                layout.win_w,
                layout.win_h,
                origin.0,
                origin.1,
                x11.composited()
            );
            let init = view::Init { pack: pack2.clone(), layout, x11, name, origin, t_start, focus_probe: args.focus_probe };
            let view = cx.new(|cx| view::Avatar::new(init, window, cx));
            built = Some(view.clone());
            view
        });
        if let Err(e) = opened {
            eprintln!("synaps-avatar: cannot open the window: {e:#}");
            std::fs::remove_file(control::socket_path()).ok();
            std::process::exit(1);
        }
        let view = built.expect("window built");

        // Decode: the default frame first so something shows quickly, then the rest.
        let geo = layout.geo;
        let (verify, threads) = (args.verify, args.threads);
        let decode_view = view.clone();
        let p = pack.clone();
        cx.spawn(async move |cx| {
            let default = p.frame(&p.catalog.default_sprite).unwrap_or(0);
            let p1 = p.clone();
            let first = off_thread(move || frames::decode(&p1, &geo, &[default], verify, 1)).await;
            let first = match first {
                Ok(d) => d,
                Err(e) => fatal(e),
            };
            decode_view.update(cx, |v, cx| v.add_frames(vec![(default, first.frames.into_iter().next().unwrap())], false, cx));
            let rest: Vec<usize> = (0..p.frames.len()).filter(|&i| i != default).collect();
            let p2 = p.clone();
            let rest2 = rest.clone();
            let all = off_thread(move || frames::decode(&p2, &geo, &rest2, verify, threads)).await;
            let all = match all {
                Ok(d) => d,
                Err(e) => fatal(e),
            };
            eprintln!(
                "synaps-avatar: decoded {} frames in {} ms on {threads} threads; all frames ready {} ms after start",
                all.frames.len(),
                all.decode_ms,
                t_start.elapsed().as_millis()
            );
            decode_view.update(cx, |v, cx| v.add_frames(rest.into_iter().zip(all.frames).collect(), true, cx));
            if let Some(m) = p.behaviour.music.clone() {
                let p3 = p.clone();
                let size = crate::view::note_px(&geo);
                match off_thread(move || frames::decode_notes(&p3, &m.notes, size)).await {
                    Ok(notes) => decode_view.update(cx, |v, _| v.set_notes(notes)),
                    Err(e) => eprintln!("synaps-avatar: music notes unavailable: {e:#}"),
                }
            }
            #[cfg(target_env = "gnu")]
            // SAFETY: plain libc call; hands freed decode buffers back to the OS.
            unsafe {
                libc::malloc_trim(0);
            }
        })
        .detach();

        if !args.no_session {
            connect_session(&args, &view, cx);
        }

        control::serve(listener, tx);
        cx.spawn(async move |cx| {
            while let Ok((cmd, reply)) = rx.recv().await {
                let r = view.update(cx, |v, cx| v.apply(cmd, cx));
                reply.send(r).ok();
            }
        })
        .detach();
    });
    Ok(())
}

/// Start the session client and pump its events into the view. The pump drains the
/// channel whether or not the window draws (lynx's H4: a hidden overlay that only
/// applied events in render would let the queue grow without bound).
fn connect_session(args: &Args, view: &Entity<view::Avatar>, cx: &mut App) {
    let identity = match synaps_client::Identity::new("avatar") {
        Ok(i) => i,
        Err(e) => fatal(anyhow::anyhow!("identity: {e}")),
    };
    let (client, rx) = match synaps_client::Client::connect(args.profile.as_deref(), identity) {
        Ok(c) => c,
        Err(e) => fatal(anyhow::anyhow!("cannot start the session client: {e}")),
    };
    eprintln!(
        "synaps-avatar: session: profile {}, target {:?}",
        args.profile.clone().or_else(|| std::env::var("SYNAPS_PROFILE").ok()).unwrap_or_else(|| "default".into()),
        args.target
    );
    let opts = session::Options { target: args.target.clone(), cwd: args.cwd.clone(), model: args.model.clone(), sleepy_after: args.sleepy_after };
    let link = session::Link::new(opts, Instant::now());
    view.update(cx, |v, cx| v.connect_session(client, link, cx));
    let view = view.downgrade();
    cx.spawn(async move |cx| {
        while let Ok(first) = rx.recv().await {
            let mut batch = Vec::with_capacity(rx.len() + 1);
            batch.push(first);
            while batch.len() < 1024
                && let Ok(e) = rx.try_recv()
            {
                batch.push(e);
            }
            if view.update(cx, |v, cx| v.session_events(batch, cx)).is_err() {
                return;
            }
            YieldNow(false).await;
        }
        view.update(cx, |v, cx| v.session_closed(cx)).ok();
    })
    .detach();
}

/// Pending once, so a frame can run between two event batches.
struct YieldNow(bool);

impl std::future::Future for YieldNow {
    type Output = ();
    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        if std::mem::replace(&mut self.0, true) {
            return std::task::Poll::Ready(());
        }
        cx.waker().wake_by_ref();
        std::task::Poll::Pending
    }
}

/// Run `f` on a plain thread and await its result. GPUI's background executor can be
/// slow to hand a finished task back to an idle main loop (see ticker.rs).
async fn off_thread<T: Send + 'static>(f: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> anyhow::Result<T> {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        tx.send_blocking(f()).ok();
    });
    rx.recv().await.map_err(|_| anyhow::anyhow!("decode thread died"))?
}

fn fatal(e: anyhow::Error) -> ! {
    eprintln!("synaps-avatar: {e:#}");
    std::fs::remove_file(control::socket_path()).ok();
    std::process::exit(1);
}
