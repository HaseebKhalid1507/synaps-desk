//! The transcript's own scrolling (cohesion/fluid.md §3.2): the wheel
//! glides, a touchpad tracks the finger and coasts when it lets go, and a
//! jump springs to its target.
//!
//! The list's wheel is taken in the capture phase ([`owner`]), over the
//! list, so GPUI's own handler (bubble phase) never runs; what this
//! computes reaches the list each frame as `ListState::scroll_by`, a
//! relative move, so everything else that moves the list (a dock's
//! compensation, the hold-back, following) still composes with it.
//!
//! - **Wheel** (steps of a wheel's quantum: both Linux backends count a
//!   notch as 120 units, 3 lines, and a hi-res wheel's step as a fraction
//!   of those 120, so every wheel delta is a whole multiple of
//!   [`WHEEL_QUANTUM`]; a touchpad's almost never is): each step moves a
//!   target 20 px a line; the offset chases it on `wheel`, critically
//!   damped, and a step in flight retargets it, keeping its speed: a spun
//!   wheel is one glide, and a wheel never coasts.
//! - **Precise** (anything else: a touchpad's fractions, pixels): one to
//!   one, drawn at frame time from the packets ([`LAG`] behind,
//!   interpolated), so a 125 Hz touchpad moves the column every frame of a
//!   144 or 240 Hz display.
//! - **Coast** (Linux, which sends no gesture end and no momentum of its
//!   own): a gesture of [`MIN_PACKETS`] packets over [`MIN_SPAN`] or more,
//!   moving at [`COAST_FLOOR`] or faster, is drawn on past its last packet
//!   on the coast's own curve; two intervals with no packet ([`GAP`]) end
//!   the gesture and the coast goes on from there, decaying at 0.998 a
//!   millisecond until under 0.05 px/ms. A finger that slowed to rest has
//!   no speed: no coast. A packet within [`TAKE_BACK`] of the coast's start
//!   is the finger still down: what the coast moved is taken back from it;
//!   later, it is a new gesture, and the coast stops where it is.
//! - **Ends**: nothing moves past the top or the end; reaching the end of a
//!   measured list re-follows the tail; short of an end not measured yet
//!   (rows not laid out) a motion goes no nearer than 2 px, and goes on
//!   once the next layout has measured them.
//! - Every jump, drag or page cancels all of it first ([`Scroller::cancel`]).
//!
//! Reduced and Off: the list's own (instant) wheel, as before.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::*;

use crate::motion::{GLIDE_RESPONSE, Motion, SNAP_RESPONSE, Spring, WHEEL_RESPONSE};

/// A line of the wheel, as GPUI's list counts it.
pub(crate) const LINE_PX: f32 = 20.;
/// A wheel's quantum, in lines: 3 lines a 120-unit notch (X11's valuator
/// increment and Wayland's `value120`); hi-res wheels send whole fractions
/// of the 120.
pub(crate) const WHEEL_QUANTUM: f32 = 3. / 120.;
/// Precise input is drawn this far behind its packets, at most.
const LAG: Duration = Duration::from_millis(12);
/// No packet for two intervals (and at least this) ends a gesture: 16 ms
/// at 125 Hz. A packet that comes late under load (the events dispatch in
/// clumps on a slow frame) is not an end.
const GAP: Duration = Duration::from_millis(16);
/// A packet this soon after a coast began is the finger still down (a late
/// packet); later, the finger came back for a new gesture.
const TAKE_BACK: Duration = Duration::from_millis(30);
/// A gesture coasts only from this many packets over this long: a nudge,
/// or a wheel's burst that slipped through, doesn't fling the list.
const MIN_PACKETS: u32 = 4;
const MIN_SPAN: Duration = Duration::from_millis(40);
/// A gesture's first packet moved from one 125 Hz interval before it.
const FIRST: Duration = Duration::from_millis(8);
/// A frame advances a coast by this much at most (a stall slows it, as a
/// glide's `dt` does, rather than landing its rest in one frame).
const MAX_DT: f32 = 50.;
/// The coast's decay per millisecond (iOS "normal": a 500 ms time constant).
const DECAY: f32 = 0.998;
/// Coasts start at this speed or faster (px/ms), and stop under [`STOP`].
const COAST_FLOOR: f32 = 0.2;
const STOP: f32 = 0.05;
/// The fastest coast (px/ms).
const COAST_MAX: f32 = 8.;
/// A jump farther than this many viewports cuts to one viewport short first.
/// (Today's one jump is a track click's page, never more than a viewport:
/// this is for F2's jumps, to the end or to a search hit.)
const PRE_CUT: f32 = 1.5;
/// Linux only: X11 and Wayland send a touchpad's fling as packets that just
/// stop. macOS and Windows send the OS's own momentum (or phases), so a
/// coast of ours there would be a second one.
const COASTS: bool = cfg!(any(target_os = "linux", target_os = "freebsd"));

/// Where the list is, as the scroller needs it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Room {
    /// How far it is scrolled, px (≥ 0).
    pub(crate) offset: f32,
    /// The farthest it scrolls.
    pub(crate) max: f32,
    /// Every row has a height (measured or hinted): the end is where `max` says.
    pub(crate) end_known: bool,
}

/// What a frame does to the list.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Step {
    pub(crate) by: f32,
    /// It reached the end: follow the tail.
    pub(crate) follow: bool,
}

#[derive(Clone, Copy, Debug)]
struct Coast {
    /// When it started, and its own time since (ms; a frame adds at most
    /// [`MAX_DT`]), as of `at`.
    from: Instant,
    t: f32,
    at: Instant,
    v0: f32,
    done: f32,
}

/// A coast's distance `t` ms in at `v0` px/ms.
fn coasted(v0: f32, t: f32) -> f32 {
    v0 * (DECAY.powf(t) - 1.) / DECAY.ln()
}

/// A delta of `lines` is a wheel's: a whole number of its quanta.
pub(crate) fn is_wheel_step(lines: f32) -> bool {
    let q = lines / WHEEL_QUANTUM;
    lines != 0. && (q - q.round()).abs() < 1e-3
}

#[derive(Default)]
struct Inner {
    /// The glide (wheel or jump): its displacement from its target.
    glide: Spring,
    response: f32,
    /// Precise packets: (when, how far in all since the gesture began);
    /// while `origin`, the first is the gesture's stand-in start, not a packet.
    packets: VecDeque<(Instant, f32)>,
    origin: bool,
    /// The gesture's packets so far, and its first's time.
    count: u32,
    start: Option<Instant>,
    total: f32,
    /// How much of `total` the list has been moved.
    applied: f32,
    /// How far past the last packet the finger was drawn on (a gesture
    /// that may coast, in its gap).
    drawn_on: f32,
    /// What a coast moved that the finger, back on the pad, has yet to make up.
    debt: f32,
    /// The platform sends phases (and its own momentum): no coast of ours.
    phased: bool,
    /// Tests: coast on any OS.
    coasts: bool,
    coast: Option<Coast>,
    last: Option<Instant>,
    /// The user moved the list (a wheel or a finger over it, its glide or
    /// coast, a jump) since [`Scroller::touched`] last asked: what tells a
    /// scroll from rows changing height (Shady #23 r3 H2, M1).
    touched: bool,
}

/// The transcript's scroller: shared by the wheel's listener and the frame.
#[derive(Clone)]
pub(crate) struct Scroller(Rc<RefCell<Inner>>);

impl Default for Scroller {
    fn default() -> Self {
        Scroller(Rc::new(RefCell::new(Inner { coasts: COASTS, ..Inner::default() })))
    }
}

impl Scroller {
    /// A scroller that coasts whatever the OS (the tests run on all three).
    #[cfg(test)]
    pub(crate) fn coasting() -> Self {
        let s = Scroller::default();
        s.0.borrow_mut().coasts = true;
        s
    }

    /// Where the glide in flight is still going (px, + down): a page
    /// clicked during one pages on from its end.
    pub(crate) fn remaining(&self) -> f32 {
        -self.0.borrow().glide.x
    }

    /// Something moves: frames are wanted.
    pub(crate) fn active(&self) -> bool {
        let s = self.0.borrow();
        s.glide != Spring::default() || !s.packets.is_empty() || s.coast.is_some()
    }

    /// A jump, a drag, a page, another list: everything in flight stops.
    pub(crate) fn cancel(&self) {
        let mut s = self.0.borrow_mut();
        *s = Inner { coasts: s.coasts, touched: s.touched, ..Inner::default() };
    }

    /// The user's input reached the list (any scroll over it: one the list
    /// takes itself too, a touchscreen's or below Full).
    pub(crate) fn touch(&self) {
        self.0.borrow_mut().touched = true;
    }

    /// Whether the user moved the list since the last ask (and forget it).
    pub(crate) fn touched(&self) -> bool {
        std::mem::take(&mut self.0.borrow_mut().touched)
    }

    /// A wheel event over the list, at `now`: whether it was taken
    /// (vertical, and not a touchscreen, which GPUI coasts itself). A
    /// sideways packet inside a gesture is the same finger (a diagonal
    /// swipe): taken, its sideways part dropped; outside one, it is not ours.
    pub(crate) fn wheel(&self, ev: &ScrollEvent, now: Instant) -> bool {
        if ev.source == ScrollSource::Touch {
            return false;
        }
        let (dx, dy, lines) = match ev.delta {
            ScrollDelta::Lines(p) => (p.x * LINE_PX, p.y * LINE_PX, Some(p.y)),
            ScrollDelta::Pixels(p) => (f32::from(p.x), f32::from(p.y), None),
        };
        let mut s = self.0.borrow_mut();
        // A finger on the pad: packets in, or a coast it may have started.
        let touching = !s.packets.is_empty() || s.coast.is_some_and(|c| now.saturating_duration_since(c.from) <= TAKE_BACK);
        let in_gesture = touching || s.coast.is_some();
        if dx.abs() > dy.abs() && !in_gesture {
            return false;
        }
        if matches!(ev.touch_phase, TouchPhase::Started | TouchPhase::Ended | TouchPhase::Cancelled) {
            s.phased = true;
        }
        if dy == 0. && !in_gesture {
            return true;
        }
        // The list's offset grows by −delta.y.
        let by = -dy;
        if lines.is_some_and(is_wheel_step) && !touching {
            // A wheel's step: the target moves; the glide keeps its speed.
            s.coast = None;
            s.packets.clear();
            (s.total, s.applied, s.drawn_on, s.debt, s.count, s.start) = (0., 0., 0., 0., 0, None);
            if s.glide == Spring::default() {
                s.last = Some(now);
            }
            s.response = WHEEL_RESPONSE;
            s.glide.x -= by;
        } else {
            // The finger: the glide's rest is dropped where it is.
            s.glide = Spring::default();
            let mut resumed = false;
            if let Some(c) = s.coast.take() {
                s.packets.clear();
                (s.total, s.applied, s.drawn_on) = (0., 0., 0.);
                // Still down (a late packet): what the coast moved is taken
                // back, and the gesture goes on. Later, the finger came back
                // for a new one, from where the coast is.
                resumed = now.saturating_duration_since(c.from) <= TAKE_BACK;
                s.debt = if resumed { c.done } else { 0. };
            } else if s.drawn_on != 0. {
                // A late packet: the finger was drawn on past the last one;
                // it moves the list again once it passes what was drawn.
                s.debt += s.drawn_on;
                s.applied -= s.drawn_on;
                s.drawn_on = 0.;
            }
            s.total += by;
            let total = s.total;
            if s.packets.is_empty() {
                if !resumed {
                    (s.count, s.start) = (0, None);
                }
                // The gesture starts where the list is, an interval before
                // its first packet (not at it: that packet's move took time).
                s.packets.push_back((now.checked_sub(FIRST).unwrap_or(now), 0.));
                s.origin = true;
                s.last = Some(now);
            }
            s.packets.push_back((now, total));
            s.count += 1;
            s.start.get_or_insert(now);
            while s.packets.len() > 8 {
                s.packets.pop_front();
                s.origin = false;
            }
        }
        true
    }

    /// A jump of `by` px, sprung on `glide` (`page`: on `snap`). Farther
    /// than [`PRE_CUT`] viewports, it cuts to one viewport short at once.
    pub(crate) fn jump(&self, by: f32, page: bool, view: f32, now: Instant) -> f32 {
        self.cancel();
        let mut s = self.0.borrow_mut();
        s.touched = true;
        let cut = if by.abs() > PRE_CUT * view { by - by.signum() * view } else { 0. };
        s.glide.x = -(by - cut);
        s.response = if page { SNAP_RESPONSE } else { GLIDE_RESPONSE };
        s.last = Some(now);
        cut
    }

    /// The frame at `now`: how far to move the list, inside `room`.
    pub(crate) fn frame(&self, now: Instant, room: Room) -> Step {
        let mut s = self.0.borrow_mut();
        let dt = s.last.map_or(0., |t| now.saturating_duration_since(t).as_secs_f32()).min(0.05);
        s.last = Some(now);
        let mut by = 0.;
        if s.glide != Spring::default() {
            let before = s.glide.x;
            let response = if s.response > 0. { s.response } else { WHEEL_RESPONSE };
            s.glide.step(response, dt);
            if s.glide.settled(response) {
                s.glide = Spring::default();
            }
            by += s.glide.x - before;
        }
        if !s.packets.is_empty() {
            by += s.track(now);
        }
        if let Some(c) = s.coast.as_mut() {
            if now > c.at {
                c.t += (now.duration_since(c.at).as_secs_f32() * 1000.).min(MAX_DT);
                c.at = now;
            }
            let x = coasted(c.v0, c.t);
            by += x - c.done;
            c.done = x;
            if (c.v0 * DECAY.powf(c.t)).abs() < STOP {
                (s.coast, s.count, s.start) = (None, 0, None);
            }
        }
        // The ends: nothing goes past them.
        let to = room.offset + by;
        let mut step = Step { by, follow: false };
        if to <= 0. && by < 0. {
            step.by = -room.offset;
            s.stop();
        } else if to >= room.max - 1. && by > 0. {
            if room.end_known {
                step = Step { by: room.max - room.offset, follow: true };
                s.stop();
            } else {
                // Rows not laid out yet: no nearer than 2 px; the motion
                // goes on, and the next layout has measured them.
                step.by = (room.max - 2. - room.offset).max(0.);
            }
        }
        s.touched |= step.by != 0. || step.follow;
        step
    }
}

impl Inner {
    fn stop(&mut self) {
        *self = Inner { phased: self.phased, coasts: self.coasts, touched: self.touched, ..Inner::default() };
    }

    /// The gesture's speed (px/ms) from its last three packets, if it may
    /// coast: long enough ([`MIN_PACKETS`], [`MIN_SPAN`]), fast enough, on
    /// a platform with no momentum of its own.
    fn coast_speed(&self) -> Option<f32> {
        let n = self.packets.len();
        let real = n - usize::from(self.origin);
        let (t_last, total) = self.packets[n - 1];
        let span = self.start.map_or(Duration::ZERO, |s| t_last.saturating_duration_since(s));
        if !self.coasts || self.phased || self.count < MIN_PACKETS || span < MIN_SPAN || real < 3 {
            return None;
        }
        let (t0, x0) = self.packets[n - 3];
        let ms = t_last.saturating_duration_since(t0).as_secs_f32() * 1000.;
        let v = if ms > 0. { ((total - x0) / ms).clamp(-COAST_MAX, COAST_MAX) } else { 0. };
        (v.abs() >= COAST_FLOOR).then_some(v)
    }

    /// The finger at `now`: drawn [`LAG`] (at most a packet) behind its
    /// packets, so every frame falls between two. Past its last packet, a
    /// gesture that may coast is drawn on along the coast's curve (no frame
    /// stands still while the gap is told from a late packet); a packet
    /// that comes after all moves nothing until the finger passes what was
    /// drawn. Two intervals with no packet end the gesture: the coast goes
    /// on from where it was drawn.
    fn track(&mut self, now: Instant) -> f32 {
        let n = self.packets.len();
        let (t_last, total) = self.packets[n - 1];
        let first = usize::from(self.origin);
        let interval = if n - first >= 3 { self.packets[n - 1].0.saturating_duration_since(self.packets[n - 3].0) / 2 } else { FIRST };
        let lag = interval.min(LAG);
        let at = now.checked_sub(lag).unwrap_or(now);
        let v = self.coast_speed();
        // How long the finger has been drawn on past its last packet (ms; a
        // stall slows it, as it slows a coast).
        let on = (at.saturating_duration_since(t_last).as_secs_f32() * 1000.).min(MAX_DT);
        let shown = match self.packets.iter().position(|p| p.0 > at) {
            Some(0) => self.packets[0].1,
            Some(k) => {
                let ((t0, x0), (t1, x1)) = (self.packets[k - 1], self.packets[k]);
                let span = t1.saturating_duration_since(t0).as_secs_f32();
                let f = if span > 0. { at.saturating_duration_since(t0).as_secs_f32() / span } else { 1. };
                x0 + (x1 - x0) * f.clamp(0., 1.)
            }
            None => total + v.map_or(0., |v| coasted(v, on)),
        };
        self.drawn_on = if at >= t_last { shown - total } else { 0. };
        let mut by = shown - self.applied;
        self.applied = shown;
        if self.debt != 0. {
            if by.signum() == self.debt.signum() {
                let take = by.abs().min(self.debt.abs()) * by.signum();
                by -= take;
                self.debt -= take;
            } else if by != 0. {
                self.debt = 0.;
            }
        }
        if now.saturating_duration_since(t_last) >= GAP.max(interval * 2) {
            // The coast goes on from what was drawn past the last packet.
            let drawn = self.drawn_on;
            self.packets.clear();
            (self.total, self.applied, self.drawn_on, self.debt) = (0., 0., 0., 0.);
            if let Some(v0) = v {
                self.coast = Some(Coast { from: t_last + lag, t: on, at: now, v0, done: drawn });
            } else {
                (self.count, self.start) = (0, None);
            }
        }
        by
    }
}

/// The capture-phase owner of the list's vertical wheel: an absolute,
/// full-size canvas over the list's box. Full only (the user's motion
/// plays in low power too); otherwise the list's own wheel.
pub(crate) fn owner(scroller: Scroller, fx: Rc<Motion>) -> impl IntoElement {
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |_, hitbox, window, _| {
            let view = window.current_view();
            window.on_mouse_event(move |ev: &ScrollEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture || !hitbox.should_handle_scroll(window) {
                    return;
                }
                scroller.touch();
                if crate::motion::Level::current(cx) != crate::motion::Level::Full {
                    return;
                }
                if scroller.wheel(ev, fx.clock()) {
                    cx.stop_propagation();
                    cx.notify(view);
                }
            });
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use gpui_kit::{Modifiers, ScrollDelta, ScrollEvent, ScrollSource, TouchPhase, point, px};

    use super::{Room, Scroller};

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn lines(y: f32) -> ScrollEvent {
        ScrollEvent {
            position: point(px(0.), px(0.)),
            delta: ScrollDelta::Lines(point(0., y)),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
            source: ScrollSource::Pointer,
        }
    }

    const ROOM: Room = Room { offset: 5000., max: 100_000., end_known: true };

    /// Frames at 144 Hz from `t0` for `n` frames: the list's offset each frame.
    fn run(s: &Scroller, t0: Instant, n: u32, mut offset: f32, mut input: impl FnMut(u32, Instant)) -> Vec<f32> {
        let mut out = Vec::new();
        for k in 0..n {
            let now = t0 + Duration::from_micros(6944 * k as u64);
            input(k, now);
            let step = s.frame(now, Room { offset, ..ROOM });
            offset += step.by;
            out.push(offset);
        }
        out
    }

    /// A spun wheel is one glide: every frame moves, a notch in flight
    /// retargets without a jump in speed, and it lands exactly on 60 px a notch.
    #[test]
    fn wheel_retarget_is_continuous() {
        let s = Scroller::coasting();
        let t0 = Instant::now();
        let notches = [0u32, 6, 12, 18];
        let path = run(&s, t0, 80, 0., |k, now| {
            if notches.contains(&k) {
                assert!(s.wheel(&lines(-3.), now));
            }
        });
        let v: Vec<f32> = path.windows(2).map(|w| w[1] - w[0]).collect();
        // Moving from the first frame after the first notch until it settles.
        assert!(v[..30].iter().all(|d| *d > 0.), "{v:?}");
        // No step in speed bigger than a few px a frame between frames.
        let dv = v.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0., f32::max);
        assert!(dv < 6., "speed jumped {dv} px/frame: {v:?}");
        assert!((path[79] - 240.).abs() < 0.01, "{}", path[79]);
        assert!(!s.active());
        // A reversal mid-flight turns smoothly too.
        let s = Scroller::coasting();
        let path = run(&s, t0, 60, 0., |k, now| match k {
            0 => assert!(s.wheel(&lines(-3.), now)),
            5 => assert!(s.wheel(&lines(3.), now)),
            _ => {}
        });
        let v: Vec<f32> = path.windows(2).map(|w| w[1] - w[0]).collect();
        let dv = v.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0., f32::max);
        assert!(dv < 6., "{v:?}");
        assert!(path[59].abs() < 0.01);
    }

    /// A touchpad's packets (fractions of a line at 125 Hz) move the list
    /// every frame of a 144 Hz display, then coast when they stop, and the
    /// coast stops by itself; at the end it stops dead and follows.
    #[test]
    fn coast_stops_and_refollows() {
        let s = Scroller::coasting();
        let t0 = Instant::now();
        // 30 packets 8 ms apart at 2 px/ms.
        let mut next = 0u32;
        let path = run(&s, t0, 700, 0., |_, now| {
            while next < 30 && t0 + ms(8 * next as u64) <= now {
                assert!(s.wheel(&lines(-16. / 20. - 0.013), now));
                next += 1;
            }
        });
        let v: Vec<f32> = path.windows(2).map(|w| w[1] - w[0]).collect();
        // From the first packet's frame on, no frame stands still until the coast slows.
        let first = v.iter().position(|d| *d > 0.).unwrap();
        let still = v[first..60].iter().filter(|d| **d == 0.).count();
        assert_eq!(still, 0, "{:?}", &v[..60]);
        // It coasted well past the finger's 480 px, and stopped.
        assert!(path[699] > 900., "{}", path[699]);
        assert_eq!(v[698], 0.);
        assert!(!s.active());

        // A finger that slows to rest doesn't coast.
        let s = Scroller::coasting();
        let speeds = [16., 16., 16., 16., 12., 8., 4., 2., 1., 0.5];
        let mut next = 0usize;
        let path = run(&s, t0, 200, 0., |_, now| {
            while next < speeds.len() && t0 + ms(8 * next as u64) <= now {
                assert!(s.wheel(&lines(-speeds[next] / 20. - 0.0001), now));
                next += 1;
            }
        });
        let fed: f32 = speeds.iter().map(|p| p + 0.002).sum();
        assert!((path[199] - fed).abs() < 1., "{} vs {fed}", path[199]);

        // Into the end: stops at it and follows.
        let s = Scroller::coasting();
        let mut offset = 99_000.;
        let mut followed = false;
        for k in 0..300u32 {
            let now = t0 + Duration::from_micros(6944 * k as u64);
            if k < 20 {
                s.wheel(&lines(-3. * 8. / 20. - 0.013), now);
            }
            let step = s.frame(now, Room { offset, ..ROOM });
            offset += step.by;
            followed |= step.follow;
            assert!(offset <= ROOM.max);
        }
        assert!(followed);
        assert_eq!(offset, ROOM.max);
        assert!(!s.active());
        // Short of an end not yet measured, it goes no nearer than 2 px,
        // and on once the rows there are measured (Shady L6).
        let s = Scroller::coasting();
        s.wheel(&lines(-30.), t0);
        let step = s.frame(t0 + ms(100), Room { offset: 99_900., end_known: false, ..ROOM });
        assert_eq!(step, super::Step { by: 98., follow: false });
        assert!(s.active(), "still going");
        let step = s.frame(t0 + ms(107), Room { offset: 99_998., max: 100_400., end_known: true });
        assert!(step.by > 0. && !step.follow, "{step:?}");
    }

    /// A late packet (the finger still down, within [`super::TAKE_BACK`]
    /// of the coast's start) takes back what the coast moved: the list
    /// doesn't jump back, and isn't ahead of the finger by the coast.
    #[test]
    fn a_resumed_finger_takes_the_coast_back() {
        let s = Scroller::coasting();
        let t0 = Instant::now();
        let mut next = 0u32;
        let step = 20. * (0.8 + 0.013);
        // 10 packets, a 30 ms hole (a coast starts), then 10 more.
        let due = |i: u32| t0 + ms(8 * i as u64 + if i >= 10 { 22 } else { 0 });
        let path = run(&s, t0, 120, 0., |_, now| {
            while next < 20 && due(next) <= now {
                assert!(s.wheel(&lines(-0.8 - 0.013), now));
                next += 1;
            }
        });
        let v: Vec<f32> = path.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(v.iter().all(|d| *d >= 0.), "never backwards: {v:?}");
        // As the finger reaches its last packet, the list is where the
        // finger is (give or take one packet drawn on), not a coast ahead.
        let k = (0..120u32).find(|k| t0 + Duration::from_micros(6944 * *k as u64) >= due(19) + ms(8)).unwrap() as usize;
        let finger = 20. * step;
        assert!(path[k] > finger - step && path[k] < finger + step, "{} vs the finger's {finger}", path[k]);
    }

    /// Shady H2: a second flick 250 ms into a coast is a new gesture: the
    /// coast stops where it is and the second swipe moves the list.
    #[test]
    fn a_second_flick_moves_the_list() {
        let s = Scroller::coasting();
        let t0 = Instant::now();
        let mut next = 0u32;
        let due = |i: u32| t0 + ms(8 * i as u64 + if i >= 10 { 250 } else { 0 });
        let at = |k: usize| t0 + Duration::from_micros(6944 * k as u64);
        let path = run(&s, t0, 200, 0., |_, now| {
            while next < 20 && due(next) <= now {
                assert!(s.wheel(&lines(-0.8 - 0.013), now));
                next += 1;
            }
        });
        let (a, b) = ((0..200).find(|k| at(*k) >= due(10)).unwrap(), (0..200).find(|k| at(*k) >= due(19) + ms(8)).unwrap());
        let moved = path[b] - path[a];
        // The finger's 9 packets after its first: 146 px; drawn 8 ms behind.
        assert!(moved > 120., "the second swipe moved the list {moved} px");
    }

    /// Shady H3: a hi-res wheel (15 of 120 units a packet, 0.375 lines;
    /// 8 to a detent, in about 30 ms) glides on `wheel` and lands exactly:
    /// it never coasts, however fast its bursts.
    #[test]
    fn a_hi_res_wheel_never_coasts() {
        assert!(super::is_wheel_step(3.) && super::is_wheel_step(-0.375) && super::is_wheel_step(0.025));
        assert!(!super::is_wheel_step(0.813) && !super::is_wheel_step(-0.0251) && !super::is_wheel_step(0.));
        let s = Scroller::coasting();
        let t0 = Instant::now();
        let mut sent = 0u32;
        let due = |i: u32| t0 + ms(100 * (i / 8) as u64 + 4 * (i % 8) as u64);
        let path = run(&s, t0, 200, 0., |_, now| {
            while sent < 24 && due(sent) <= now {
                assert!(s.wheel(&lines(-0.375), now));
                sent += 1;
            }
        });
        assert!((path[199] - 180.).abs() < 0.01, "3 detents, 60 px each: {}", path[199]);
        assert!(path.iter().all(|y| *y <= 180.01), "no fling: {:?}", path.iter().fold(0f32, |a, b| a.max(*b)));
        assert!(!s.active());
    }

    /// A nudge (3 packets, however fast) doesn't coast, and a swipe's coast
    /// starts at the finger's real speed, not double it (the gesture's
    /// start is an interval before its first packet).
    #[test]
    fn a_nudge_doesnt_coast_and_a_swipe_coasts_at_its_speed() {
        let s = Scroller::coasting();
        let t0 = Instant::now();
        let mut next = 0u32;
        let path = run(&s, t0, 200, 0., |_, now| {
            while next < 3 && t0 + ms(8 * next as u64) <= now {
                assert!(s.wheel(&lines(-1.513), now));
                next += 1;
            }
        });
        assert!((path[199] - 3. * 30.26).abs() < 0.01, "{}", path[199]);
        // 10 packets at 2.03 px/ms: the coast adds (v0 − 0.05) / −ln 0.998.
        let s = Scroller::coasting();
        let mut next = 0u32;
        let step = 20. * (0.8 + 0.013);
        // (Each packet at its own time, not the frame's: the speed is exact.)
        let path = run(&s, t0, 900, 0., |_, now| {
            while next < 10 && t0 + ms(8 * next as u64) <= now {
                assert!(s.wheel(&lines(-0.8 - 0.013), t0 + ms(8 * next as u64)));
                next += 1;
            }
        });
        let v0 = step / 8.;
        let coast = (v0 - 0.05) / -(0.998f32.ln());
        let want = 10. * step + coast;
        assert!((path[899] - want).abs() < 0.03 * want, "{} vs {want}", path[899]);
    }

    /// Off Linux the OS sends its own momentum: no coast of ours.
    #[test]
    fn coasts_only_where_the_os_has_none() {
        assert_eq!(super::COASTS, cfg!(any(target_os = "linux", target_os = "freebsd")));
    }

    /// A jump far away cuts to a viewport short, then glides the rest.
    #[test]
    fn a_far_jump_cuts_then_glides() {
        let s = Scroller::coasting();
        let t0 = Instant::now();
        let cut = s.jump(5000., false, 900., t0);
        assert_eq!(cut, 4100.);
        let path = run(&s, t0, 80, 4100., |_, _| {});
        assert!((path[79] - 5000.).abs() < 0.01);
        assert!(path.windows(2).all(|w| w[1] >= w[0]));
    }

    /// A touchscreen (GPUI coasts it) and sideways scrolling are not ours;
    /// a sideways packet inside a gesture (a diagonal swipe) is the same
    /// finger: taken, its sideways part dropped (Shady L4).
    #[test]
    fn touch_and_sideways_pass_through() {
        let s = Scroller::coasting();
        let mut e = lines(-3.);
        e.source = ScrollSource::Touch;
        assert!(!s.wheel(&e, Instant::now()));
        let sideways = |y: f32| {
            let mut e = lines(0.);
            e.delta = ScrollDelta::Lines(point(-1.213, y));
            e
        };
        assert!(!s.wheel(&sideways(-0.4), Instant::now()));
        let t0 = Instant::now();
        assert!(s.wheel(&lines(-0.813), t0));
        assert!(s.wheel(&sideways(-0.413), t0 + ms(8)));
        let path = run(&s, t0, 10, 0., |_, _| {});
        assert!((path[9] - 20. * (0.813 + 0.413)).abs() < 0.01, "both packets' vertical parts: {}", path[9]);
    }

    /// A stall mid-coast (a minimised window, a long frame) slows it: no
    /// frame moves it more than 50 ms of its own curve (Shady L5).
    #[test]
    fn a_stalled_coast_doesnt_jump() {
        let s = Scroller::coasting();
        let t0 = Instant::now();
        let mut next = 0u32;
        let mut path = run(&s, t0, 60, 0., |_, now| {
            while next < 12 && t0 + ms(8 * next as u64) <= now {
                assert!(s.wheel(&lines(-1.613), t0 + ms(8 * next as u64)));
                next += 1;
            }
        });
        assert!(s.active(), "coasting");
        let before = *path.last().unwrap();
        let step = s.frame(t0 + ms(60 * 7 + 500), Room { offset: before, ..ROOM });
        path.push(before + step.by);
        // At ≤ 4 px/ms, 50 ms of coast is under 200 px; 500 ms would be ~1300.
        assert!(step.by > 0. && step.by < 200., "{}", step.by);
    }
}
