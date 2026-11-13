//! X11 shim for what gpui-pre 0.3.12 doesn't expose on X11: input/bounding shape
//! (click-through), absolute moves, stacking, monitors, compositor detection and a
//! focus request. Uses its own x11rb connection; the window id comes from GPUI's
//! `HasWindowHandle`.

use anyhow::{Context as _, Result, bail};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use x11rb::connection::{Connection as _, RequestConnection as _};
use x11rb::protocol::Event;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xfixes::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{self, ConnectionExt as _};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

pub use crate::shape::ShapeRect;

#[derive(Clone, Debug)]
pub struct Monitor {
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

/// A top-level window's outer rect in root px.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WinRect {
    pub id: u32,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// What the watcher thread reports.
#[derive(Clone, Copy, Debug)]
pub enum WatchEvent {
    Compositor(bool),
    /// The perch target moved or resized.
    TargetMoved(WinRect),
    /// The perch target was unmapped or destroyed.
    TargetGone(u32),
}

pub struct X11 {
    conn: Arc<RustConnection>,
    pub win: u32,
    root: u32,
    pub screen_w: u16,
    pub screen_h: u16,
    composited: Arc<AtomicBool>,
    atoms: Atoms,
    /// Top-level window the watcher follows (0 = none).
    target: Arc<AtomicU32>,
    /// Effect windows (teleport bolt layers), created on first use: (window, mapped).
    /// Effect layers: (window, mapped, background colour).
    fx: Mutex<Vec<(u32, bool, u32)>>,
}

#[derive(Clone, Copy)]
struct Atoms {
    cm: u32,
    wm_class: u32,
    gtk_frame_extents: u32,
    compton_shadow: u32,
    active_window: u32,
}

fn intern(conn: &RustConnection, name: &str) -> Result<u32> {
    Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
}

#[derive(Clone, Copy)]
enum Stack {
    Top,
    BelowAvatar,
    BelowLayer(usize),
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
            compton_shadow: intern(&conn, "_COMPTON_SHADOW")?,
            active_window: intern(&conn, "_NET_ACTIVE_WINDOW")?,
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
            target: Arc::new(AtomicU32::new(0)),
            fx: Mutex::new(Vec::new()),
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
        // picom/compton: no drop shadow around the transparent window.
        self.conn.change_property32(
            xproto::PropMode::REPLACE,
            self.win,
            self.atoms.compton_shadow,
            xproto::AtomEnum::CARDINAL,
            &[0],
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

    /// Ask the server for keyboard focus (override-redirect windows never get it from the WM).
    pub fn take_focus(&self) -> Result<()> {
        self.conn.set_input_focus(xproto::InputFocus::PARENT, self.win, x11rb::CURRENT_TIME)?;
        self.conn.flush()?;
        Ok(())
    }

    pub fn focused_window(&self) -> Result<u32> {
        Ok(self.conn.get_input_focus()?.reply()?.focus)
    }

    // ---- other windows (perching) -------------------------------------------------

    /// The focused client (`_NET_ACTIVE_WINDOW`), if the WM publishes one.
    pub fn active_window(&self) -> Option<u32> {
        let r = self.conn.get_property(false, self.root, self.atoms.active_window, xproto::AtomEnum::WINDOW, 0, 1).ok()?.reply().ok()?;
        r.value32()?.next().filter(|&w| w != 0)
    }

    /// The child of the root that contains `w` (the WM frame for a managed client).
    pub fn toplevel_of(&self, mut w: u32) -> Option<u32> {
        for _ in 0..32 {
            let t = self.conn.query_tree(w).ok()?.reply().ok()?;
            if t.parent == self.root || t.parent == x11rb::NONE {
                return Some(w);
            }
            w = t.parent;
        }
        None
    }

    /// Outer rect of a top-level window, if it is mapped and viewable.
    pub fn toplevel_rect(&self, w: u32) -> Option<WinRect> {
        let a = self.conn.get_window_attributes(w).ok()?.reply().ok()?;
        if a.map_state != xproto::MapState::VIEWABLE {
            return None;
        }
        let g = self.conn.get_geometry(w).ok()?.reply().ok()?;
        let b = g.border_width as i32;
        Some(WinRect { id: w, x: g.x as i32, y: g.y as i32, w: g.width as i32 + 2 * b, h: g.height as i32 + 2 * b })
    }

    /// Viewable top-level windows, bottom to top, except ours and tiny ones.
    pub fn toplevels(&self) -> Vec<WinRect> {
        let Ok(Ok(tree)) = self.conn.query_tree(self.root).map(|c| c.reply()) else { return Vec::new() };
        let fx: Vec<u32> = self.fx.lock().map(|v| v.iter().map(|f| f.0).collect()).unwrap_or_default();
        let cookies: Vec<_> = tree
            .children
            .iter()
            .filter(|&&w| w != self.win && !fx.contains(&w))
            .filter_map(|&w| Some((w, self.conn.get_window_attributes(w).ok()?, self.conn.get_geometry(w).ok()?)))
            .collect();
        cookies
            .into_iter()
            .filter_map(|(w, a, g)| {
                let (a, g) = (a.reply().ok()?, g.reply().ok()?);
                if a.map_state != xproto::MapState::VIEWABLE || g.width < 120 || g.height < 60 {
                    return None;
                }
                let b = g.border_width as i32;
                Some(WinRect { id: w, x: g.x as i32, y: g.y as i32, w: g.width as i32 + 2 * b, h: g.height as i32 + 2 * b })
            })
            .collect()
    }

    /// Follow `w` (0 = nothing) in the watcher thread.
    pub fn set_target(&self, w: u32) {
        self.target.store(w, Ordering::Relaxed);
    }

    // ---- effect windows -----------------------------------------------------------

    /// Show effect layer `layer` (solid `rgb`) on top of everything at root `origin` with
    /// the bounding shape `rects` (relative to `origin`). Input passes through. Created on
    /// first use. Not flushed, so several layers go out in one write.
    pub fn fx_set(&self, layer: usize, rgb: u32, origin: (i32, i32), size: (i32, i32), rects: &[ShapeRect]) -> Result<()> {
        self.fx_place(layer, rgb, origin, size, Some(rects), Stack::Top)
    }

    /// Like `fx_set`, stacked directly below layer `over`. Stacking a group top-down this
    /// way never puts a lower layer above a higher one, even for a moment (raising each
    /// to the top in turn does, and a screen grab catches it).
    pub fn fx_set_below(&self, layer: usize, over: usize, rgb: u32, origin: (i32, i32), size: (i32, i32), rects: &[ShapeRect]) -> Result<()> {
        self.fx_place(layer, rgb, origin, size, Some(rects), Stack::BelowLayer(over))
    }

    /// An effect layer stacked directly below the avatar window (e.g. a contact shadow).
    /// `rects: None` keeps the current shape and only moves the window.
    pub fn fx_under(&self, layer: usize, rgb: u32, origin: (i32, i32), size: (i32, i32), rects: Option<&[ShapeRect]>) -> Result<()> {
        self.fx_place(layer, rgb, origin, size, rects, Stack::BelowAvatar)?;
        self.conn.flush()?;
        Ok(())
    }

    fn fx_place(&self, layer: usize, rgb: u32, origin: (i32, i32), size: (i32, i32), rects: Option<&[ShapeRect]>, stack: Stack) -> Result<()> {
        let mut fx = self.fx.lock().map_err(|_| anyhow::anyhow!("fx lock"))?;
        while fx.len() <= layer {
            let id = self.conn.generate_id()?;
            self.conn.create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                id,
                self.root,
                0,
                0,
                1,
                1,
                0,
                xproto::WindowClass::INPUT_OUTPUT,
                x11rb::COPY_FROM_PARENT,
                &xproto::CreateWindowAux::new().background_pixel(rgb).override_redirect(1),
            )?;
            self.conn.change_property8(
                xproto::PropMode::REPLACE,
                id,
                xproto::AtomEnum::WM_CLASS,
                xproto::AtomEnum::STRING,
                b"synaps-avatar-fx\0synaps-avatar-fx\0",
            )?;
            self.conn.shape_rectangles(shape::SO::SET, shape::SK::INPUT, xproto::ClipOrdering::UNSORTED, id, 0, 0, &[])?;
            fx.push((id, false, rgb));
        }
        let (id, mapped, colour) = fx[layer];
        if colour != rgb {
            // Layers are created lazily in a batch, and some are reused: keep the colour current.
            self.conn.change_window_attributes(id, &xproto::ChangeWindowAttributesAux::new().background_pixel(rgb))?;
            if mapped {
                self.conn.clear_area(false, id, 0, 0, 0, 0)?;
            }
            fx[layer].2 = rgb;
        }
        if rects.is_some_and(|r| r.is_empty()) {
            if mapped {
                self.conn.unmap_window(id)?;
                fx[layer].1 = false;
            }
            return Ok(());
        }
        if let Some(rects) = rects {
            let r: Vec<xproto::Rectangle> = rects.iter().map(|&(x, y, width, height)| xproto::Rectangle { x, y, width, height }).collect();
            self.conn.shape_rectangles(shape::SO::SET, shape::SK::BOUNDING, xproto::ClipOrdering::UNSORTED, id, 0, 0, &r)?;
        }
        let aux = xproto::ConfigureWindowAux::new().x(origin.0).y(origin.1).width(size.0.max(1) as u32).height(size.1.max(1) as u32);
        let aux = match stack {
            Stack::Top => aux.stack_mode(xproto::StackMode::ABOVE),
            Stack::BelowAvatar => aux.sibling(self.win).stack_mode(xproto::StackMode::BELOW),
            Stack::BelowLayer(j) => match fx.get(j) {
                Some(&(sib, true, _)) => aux.sibling(sib).stack_mode(xproto::StackMode::BELOW),
                _ => aux.stack_mode(xproto::StackMode::ABOVE),
            },
        };
        self.conn.configure_window(id, &aux)?;
        if !mapped {
            self.conn.map_window(id)?;
            fx[layer].1 = true;
        }
        Ok(())
    }

    pub fn flush(&self) {
        self.conn.flush().ok();
    }

    /// Unmap one effect layer if it exists and is up.
    pub fn fx_unmap(&self, layer: usize) {
        if let Ok(mut fx) = self.fx.lock()
            && let Some(f) = fx.get_mut(layer)
            && f.1
        {
            self.conn.unmap_window(f.0).ok();
            f.1 = false;
        }
    }

    pub fn fx_hide(&self) {
        if let Ok(mut fx) = self.fx.lock() {
            for f in fx.iter_mut().filter(|f| f.1) {
                self.conn.unmap_window(f.0).ok();
                f.1 = false;
            }
            self.conn.flush().ok();
        }
    }

    pub fn monitors(&self) -> Vec<Monitor> {
        let mut out = Vec::new();
        if let Ok(Ok(r)) = self.conn.randr_get_monitors(self.root, true).map(|c| c.reply()) {
            for m in r.monitors {
                out.push(Monitor {
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

    /// Background thread on the same connection: keeps the avatar above managed windows,
    /// tracks the compositor appearing or going away, and follows the perch target.
    /// `on_event` runs on that thread.
    pub fn spawn_watcher(&self, on_event: impl Fn(WatchEvent) + Send + 'static) -> Result<()> {
        let conn = self.conn.clone();
        let (win, root, atoms, composited) = (self.win, self.root, self.atoms, self.composited.clone());
        let target = self.target.clone();
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
                let handle = |ev: Event, restack: &mut bool, may_cover: &mut HashMap<u32, bool>| {
                    let t = target.load(Ordering::Relaxed);
                    match ev {
                        Event::ConfigureNotify(e) if e.window != win => {
                            *restack = true;
                            if t != 0 && e.window == t {
                                let b = e.border_width as i32;
                                on_event(WatchEvent::TargetMoved(WinRect {
                                    id: t,
                                    x: e.x as i32,
                                    y: e.y as i32,
                                    w: e.width as i32 + 2 * b,
                                    h: e.height as i32 + 2 * b,
                                }));
                            }
                        }
                        Event::MapNotify(e) if e.window != win => *restack = true,
                        Event::UnmapNotify(e) if t != 0 && e.window == t => on_event(WatchEvent::TargetGone(t)),
                        Event::DestroyNotify(e) => {
                            may_cover.remove(&e.window);
                            if t != 0 && e.window == t {
                                on_event(WatchEvent::TargetGone(t));
                            }
                        }
                        Event::XfixesSelectionNotify(e) if e.selection == atoms.cm => {
                            let now = e.owner != x11rb::NONE;
                            if composited.swap(now, Ordering::Relaxed) != now {
                                on_event(WatchEvent::Compositor(now));
                            }
                        }
                        _ => {}
                    }
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
