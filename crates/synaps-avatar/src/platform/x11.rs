//! X11 shim for what gpui-pre 0.3.12 doesn't expose on X11: input/bounding shape
//! (click-through), absolute moves, stacking, monitors, compositor detection and a
//! focus request. Uses its own x11rb connection; the window id comes from GPUI's
//! `HasWindowHandle`.

use anyhow::{Context as _, Result, bail};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use x11rb::connection::{Connection as _, RequestConnection as _};
use x11rb::protocol::Event;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xfixes::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{self, ConnectionExt as _};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

pub type ShapeRect = (i16, i16, u16, u16);

#[derive(Clone, Debug)]
pub struct Monitor {
    pub name: String,
    pub primary: bool,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Monitor {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
    fn distance2(&self, x: i32, y: i32) -> i64 {
        let dx = (self.x - x).max(0).max(x - (self.x + self.w - 1)) as i64;
        let dy = (self.y - y).max(0).max(y - (self.y + self.h - 1)) as i64;
        dx * dx + dy * dy
    }
}

pub struct X11 {
    conn: Arc<RustConnection>,
    pub win: u32,
    root: u32,
    pub screen_w: u16,
    pub screen_h: u16,
    composited: Arc<AtomicBool>,
    atoms: Atoms,
}

#[derive(Clone, Copy)]
struct Atoms {
    cm: u32,
    wm_class: u32,
    gtk_frame_extents: u32,
}

fn intern(conn: &RustConnection, name: &str) -> Result<u32> {
    Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
}

impl X11 {
    /// Connect; `win` is set once GPUI has created the window.
    pub fn connect() -> Result<X11> {
        let (conn, screen) = x11rb::connect(None).context("x11: cannot connect to the X server")?;
        if conn.extension_information(shape::X11_EXTENSION_NAME)?.is_none() {
            bail!("x11: the SHAPE extension is missing (needed for click-through)");
        }
        conn.shape_query_version()?.reply()?;
        let (root, screen_w, screen_h) = {
            let s = &conn.setup().roots[screen];
            (s.root, s.width_in_pixels, s.height_in_pixels)
        };
        let atoms = Atoms {
            cm: intern(&conn, &format!("_NET_WM_CM_S{screen}"))?,
            wm_class: xproto::AtomEnum::WM_CLASS.into(),
            gtk_frame_extents: intern(&conn, "_GTK_FRAME_EXTENTS")?,
        };
        let composited = conn.get_selection_owner(atoms.cm)?.reply()?.owner != x11rb::NONE;
        Ok(X11 {
            conn: Arc::new(conn),
            win: 0,
            root,
            screen_w,
            screen_h,
            composited: Arc::new(AtomicBool::new(composited)),
            atoms,
        })
    }

    pub fn composited(&self) -> bool {
        self.composited.load(Ordering::Relaxed)
    }

    /// Properties for the compositor: a `_GTK_FRAME_EXTENTS` of zero keeps picom's stock
    /// `blur-background-exclude = [ "_GTK_FRAME_EXTENTS@" ]` from frosting our bounding box.
    pub fn set_compositor_hints(&self) -> Result<()> {
        self.conn.change_property32(
            xproto::PropMode::REPLACE,
            self.win,
            self.atoms.gtk_frame_extents,
            xproto::AtomEnum::CARDINAL,
            &[0, 0, 0, 0],
        )?;
        self.conn.flush()?;
        Ok(())
    }

    /// Input region = `input`; everything else clicks through to the windows below.
    /// Without a compositor the bounding shape is set to `visible`, so no black box shows.
    pub fn set_shape(&self, input: &[ShapeRect], visible: &[ShapeRect]) -> Result<()> {
        let conv = |r: &[ShapeRect]| -> Vec<xproto::Rectangle> {
            r.iter().map(|&(x, y, width, height)| xproto::Rectangle { x, y, width, height }).collect()
        };
        let rects = conv(input);
        self.conn.shape_rectangles(
            shape::SO::SET,
            shape::SK::INPUT,
            xproto::ClipOrdering::UNSORTED,
            self.win,
            0,
            0,
            &rects,
        )?;
        if self.composited() {
            self.conn.shape_mask(shape::SO::SET, shape::SK::BOUNDING, self.win, 0, 0, x11rb::NONE)?;
        } else {
            self.conn.shape_rectangles(
                shape::SO::SET,
                shape::SK::BOUNDING,
                xproto::ClipOrdering::UNSORTED,
                self.win,
                0,
                0,
                &conv(visible),
            )?;
        }
        self.conn.flush()?;
        Ok(())
    }

    pub fn move_to(&self, x: i32, y: i32) -> Result<()> {
        self.conn.configure_window(self.win, &xproto::ConfigureWindowAux::new().x(x).y(y))?;
        self.conn.flush()?;
        Ok(())
    }

    pub fn raise(&self) -> Result<()> {
        self.conn
            .configure_window(self.win, &xproto::ConfigureWindowAux::new().stack_mode(xproto::StackMode::ABOVE))?;
        self.conn.flush()?;
        Ok(())
    }

    pub fn pointer(&self) -> Result<(i32, i32)> {
        let r = self.conn.query_pointer(self.root)?.reply()?;
        Ok((r.root_x as i32, r.root_y as i32))
    }

    pub fn window_origin(&self) -> Result<(i32, i32)> {
        let g = self.conn.get_geometry(self.win)?.reply()?;
        Ok((g.x as i32, g.y as i32))
    }

    /// Ask the server for keyboard focus (override-redirect windows never get it from the WM).
    pub fn take_focus(&self) -> Result<()> {
        self.conn.set_input_focus(xproto::InputFocus::PARENT, self.win, x11rb::CURRENT_TIME)?;
        self.conn.flush()?;
        Ok(())
    }

    pub fn focused_window(&self) -> Result<u32> {
        Ok(self.conn.get_input_focus()?.reply()?.focus)
    }

    pub fn monitors(&self) -> Vec<Monitor> {
        let mut out = Vec::new();
        if let Ok(Ok(r)) = self.conn.randr_get_monitors(self.root, true).map(|c| c.reply()) {
            for m in r.monitors {
                let name = self
                    .conn
                    .get_atom_name(m.name)
                    .ok()
                    .and_then(|c| c.reply().ok())
                    .map(|r| String::from_utf8_lossy(&r.name).into_owned())
                    .unwrap_or_default();
                out.push(Monitor {
                    name,
                    primary: m.primary,
                    x: m.x as i32,
                    y: m.y as i32,
                    w: m.width as i32,
                    h: m.height as i32,
                });
            }
        }
        if out.is_empty() {
            out.push(Monitor {
                name: "screen".into(),
                primary: true,
                x: 0,
                y: 0,
                w: self.screen_w as i32,
                h: self.screen_h as i32,
            });
        }
        out
    }

    pub fn primary(&self) -> Monitor {
        let ms = self.monitors();
        ms.iter().find(|m| m.primary).cloned().unwrap_or_else(|| ms[0].clone())
    }

    /// The monitor containing (x, y), else the nearest one.
    pub fn monitor_at(&self, x: i32, y: i32) -> Monitor {
        let ms = self.monitors();
        ms.iter()
            .find(|m| m.contains(x, y))
            .cloned()
            .unwrap_or_else(|| ms.iter().min_by_key(|m| m.distance2(x, y)).cloned().expect("a monitor"))
    }

    /// Background thread on the same connection: keeps the avatar above managed windows
    /// and tracks the compositor appearing or going away. `on_change(composited)` runs on
    /// that thread when the compositor state flips.
    pub fn spawn_watcher(&self, on_change: impl Fn(bool) + Send + 'static) -> Result<()> {
        let conn = self.conn.clone();
        let (win, root, atoms, composited) = (self.win, self.root, self.atoms, self.composited.clone());
        conn.change_window_attributes(
            root,
            &xproto::ChangeWindowAttributesAux::new().event_mask(xproto::EventMask::SUBSTRUCTURE_NOTIFY),
        )?;
        let have_xfixes = conn.extension_information(xfixes::X11_EXTENSION_NAME)?.is_some();
        if have_xfixes {
            conn.xfixes_query_version(5, 0)?.reply()?;
            conn.xfixes_select_selection_input(
                root,
                atoms.cm,
                xfixes::SelectionEventMask::SET_SELECTION_OWNER
                    | xfixes::SelectionEventMask::SELECTION_WINDOW_DESTROY
                    | xfixes::SelectionEventMask::SELECTION_CLIENT_CLOSE,
            )?;
        }
        conn.flush()?;
        std::thread::Builder::new().name("x11-watch".into()).spawn(move || {
            // Whether a top-level window may sit above us: menus, launchers and other
            // override-redirect popups may; i3 frames and managed windows may not.
            let mut may_cover: HashMap<u32, bool> = HashMap::new();
            loop {
                let Ok(ev) = conn.wait_for_event() else { return };
                let mut restack = false;
                let handle = |ev: Event, restack: &mut bool, may_cover: &mut HashMap<u32, bool>| match ev {
                    Event::ConfigureNotify(e) if e.window != win => *restack = true,
                    Event::MapNotify(e) if e.window != win => *restack = true,
                    Event::DestroyNotify(e) => {
                        may_cover.remove(&e.window);
                    }
                    Event::XfixesSelectionNotify(e) if e.selection == atoms.cm => {
                        let now = e.owner != x11rb::NONE;
                        if composited.swap(now, Ordering::Relaxed) != now {
                            on_change(now);
                        }
                    }
                    _ => {}
                };
                handle(ev, &mut restack, &mut may_cover);
                while let Ok(Some(ev)) = conn.poll_for_event() {
                    handle(ev, &mut restack, &mut may_cover);
                }
                if restack {
                    keep_above(&conn, win, root, atoms, &mut may_cover).ok();
                }
            }
        })?;
        Ok(())
    }
}

fn keep_above(conn: &RustConnection, win: u32, root: u32, atoms: Atoms, may_cover: &mut HashMap<u32, bool>) -> Result<()> {
    let tree = conn.query_tree(root)?.reply()?;
    let Some(pos) = tree.children.iter().position(|&w| w == win) else { return Ok(()) };
    for &w in &tree.children[pos + 1..] {
        let allowed = match may_cover.get(&w) {
            Some(&a) => a,
            None => {
                let attrs = conn.get_window_attributes(w)?.reply()?;
                let class = conn
                    .get_property(false, w, atoms.wm_class, xproto::AtomEnum::STRING, 0, 64)?
                    .reply()
                    .map(|r| r.value)
                    .unwrap_or_default();
                let i3_frame = class.windows(8).any(|s| s == b"i3-frame");
                let a = attrs.override_redirect && !i3_frame;
                may_cover.insert(w, a);
                a
            }
        };
        if allowed {
            continue;
        }
        let mapped = conn.get_window_attributes(w)?.reply()?.map_state == xproto::MapState::VIEWABLE;
        if mapped {
            conn.configure_window(win, &xproto::ConfigureWindowAux::new().stack_mode(xproto::StackMode::ABOVE))?;
            conn.flush()?;
            return Ok(());
        }
    }
    Ok(())
}
