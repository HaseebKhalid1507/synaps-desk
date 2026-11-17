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
//! - **Wheel** (whole lines: X11 sends a notch as 3 lines, and a touchpad as
//!   fractions of one): each notch moves a target 20 px a line; the offset
//!   chases it on `wheel`, critically damped, and a notch in flight
//!   retargets it, keeping its speed: a spun wheel is one glide.
//! - **Precise** (fractions of a line, pixels): one to one, drawn at frame
//!   time from the packets ([`LAG`] behind, interpolated), so a 125 Hz
//!   touchpad moves the column every frame of a 144 or 240 Hz display.
//! - **Coast**: Linux sends no gesture end, so a packet half an interval
//!   overdue ends it ([`GAP`]); the speed is the last packets' (a finger that slowed
//!   to rest has none), floored at [`COAST_FLOOR`]; it decays at 0.998 a
//!   millisecond and stops below 0.05 px/ms. A packet during a coast is the
//!   finger still down: what the coast moved is taken back from it.
//! - **Ends**: nothing moves past the top or the end; reaching the end of a
//!   measured list re-follows the tail; short of a measured end (rows not
//!   laid out yet) a motion stops 2 px before it.
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
/// Precise input is drawn this far behind its packets, at most.
const LAG: Duration = Duration::from_millis(12);
/// No packet for half an interval past the next one due (and at least
/// this) ends a gesture: about 12 ms at 125 Hz, so the coast takes over
/// from the drawn finger without a frame standing still.
const GAP: Duration = Duration::from_millis(10);
/// The coast's decay per millisecond (iOS "normal": a 500 ms time constant).
const DECAY: f32 = 0.998;
/// Coasts start at this speed or faster (px/ms), and stop under [`STOP`].
const COAST_FLOOR: f32 = 0.2;
const STOP: f32 = 0.05;
/// The fastest coast (px/ms).
const COAST_MAX: f32 = 8.;
/// A jump farther than this many viewports cuts to one viewport short first.
const PRE_CUT: f32 = 1.5;

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
    from: Instant,
    v0: f32,
    done: f32,
}

#[derive(Default)]
struct Inner {
    /// The glide (wheel or jump): its displacement from its target.
    glide: Spring,
    response: f32,
    /// Precise packets: (when, how far in all since the gesture began).
    packets: VecDeque<(Instant, f32)>,
    total: f32,
    /// How much of `total` the list has been moved.
    applied: f32,
    /// What a coast moved that the finger, back on the pad, has yet to make up.
    debt: f32,
    /// The platform sends phases (and its own momentum): no coast of ours.
    phased: bool,
    coast: Option<Coast>,
    last: Option<Instant>,
}

/// The transcript's scroller: shared by the wheel's listener and the frame.
#[derive(Clone, Default)]
pub(crate) struct Scroller(Rc<RefCell<Inner>>);

impl Scroller {
    /// Something moves: frames are wanted.
    pub(crate) fn active(&self) -> bool {
        let s = self.0.borrow();
        s.glide != Spring::default() || !s.packets.is_empty() || s.coast.is_some()
    }

    /// A jump, a drag, a page, another list: everything in flight stops.
    pub(crate) fn cancel(&self) {
        *self.0.borrow_mut() = Inner::default();
    }

    /// A wheel event over the list, at `now`: whether it was taken
    /// (vertical, and not a touchscreen, which GPUI coasts itself).
    pub(crate) fn wheel(&self, ev: &ScrollEvent, now: Instant) -> bool {
        if ev.source == ScrollSource::Touch {
            return false;
        }
        let (dx, dy, lines) = match ev.delta {
            ScrollDelta::Lines(p) => (p.x * LINE_PX, p.y * LINE_PX, Some(p.y)),
            ScrollDelta::Pixels(p) => (f32::from(p.x), f32::from(p.y), None),
        };
        if dx.abs() > dy.abs() {
            return false;
        }
        let mut s = self.0.borrow_mut();
        if matches!(ev.touch_phase, TouchPhase::Started | TouchPhase::Ended | TouchPhase::Cancelled) {
            s.phased = true;
        }
        if dy == 0. {
            return true;
        }
        // The list's offset grows by −delta.y.
        let by = -dy;
        if lines.is_some_and(|l| l.abs() >= 1. && (l - l.round()).abs() < 1e-3) {
            // A notch: the target moves; the glide keeps its speed.
            s.coast = None;
            s.packets.clear();
            (s.total, s.applied, s.debt) = (0., 0., 0.);
            if s.glide == Spring::default() {
                s.last = Some(now);
            }
            s.response = WHEEL_RESPONSE;
            s.glide.x -= by;
        } else {
            // The finger: the glide's rest is dropped where it is.
            s.glide = Spring::default();
            if let Some(c) = s.coast.take() {
                // Back down during a coast: what it moved is taken back.
                s.debt = c.done;
                s.packets.clear();
                (s.total, s.applied) = (0., 0.);
            }
            s.total += by;
            let total = s.total;
            if s.packets.is_empty() {
                // The gesture starts where the list is.
                s.packets.push_back((now, 0.));
                s.last = Some(now);
            }
            s.packets.push_back((now, total));
            while s.packets.len() > 8 {
                s.packets.pop_front();
            }
        }
        true
    }

    /// A jump of `by` px, sprung on `glide` (`page`: on `snap`). Farther
    /// than [`PRE_CUT`] viewports, it cuts to one viewport short at once.
    pub(crate) fn jump(&self, by: f32, page: bool, view: f32, now: Instant) -> f32 {
        self.cancel();
        let mut s = self.0.borrow_mut();
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
            let t = now.saturating_duration_since(c.from).as_secs_f32() * 1000.;
            let x = c.v0 * (DECAY.powf(t) - 1.) / DECAY.ln();
            by += x - c.done;
            c.done = x;
            if (c.v0 * DECAY.powf(t)).abs() < STOP {
                s.coast = None;
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
            } else {
                step.by = (room.max - 2. - room.offset).max(0.);
            }
            s.stop();
        }
        step
    }
}

impl Inner {
    fn stop(&mut self) {
        *self = Inner { phased: self.phased, ..Inner::default() };
    }

    /// The finger at `now`: drawn [`LAG`] (at most a packet) behind its
    /// packets, so every frame falls between two; a pause ends the gesture,
    /// which coasts from the last packets' speed.
    fn track(&mut self, now: Instant) -> f32 {
        let n = self.packets.len();
        let (t_last, total) = self.packets[n - 1];
        let interval = if n >= 3 { self.packets[n - 1].0.saturating_duration_since(self.packets[n - 3].0) / 2 } else { LAG };
        let lag = interval.min(LAG);
        let at = now.checked_sub(lag).unwrap_or(now);
        let shown = match self.packets.iter().position(|p| p.0 > at) {
            Some(0) => self.packets[0].1,
            Some(k) => {
                let ((t0, x0), (t1, x1)) = (self.packets[k - 1], self.packets[k]);
                let span = t1.saturating_duration_since(t0).as_secs_f32();
                let f = if span > 0. { at.saturating_duration_since(t0).as_secs_f32() / span } else { 1. };
                x0 + (x1 - x0) * f.clamp(0., 1.)
            }
            None => total,
        };
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
        if now.saturating_duration_since(t_last) >= GAP.max(interval * 3 / 2) {
            let v0 = if n >= 3 {
                let (t0, x0) = self.packets[n - 3];
                let ms = t_last.saturating_duration_since(t0).as_secs_f32() * 1000.;
                if ms > 0. { ((total - x0) / ms).clamp(-COAST_MAX, COAST_MAX) } else { 0. }
            } else {
                0.
            };
            self.packets.clear();
            (self.total, self.applied, self.debt) = (0., 0., 0.);
            if v0.abs() >= COAST_FLOOR && !self.phased {
                // From when the drawn finger reached its last packet.
                self.coast = Some(Coast { from: t_last + lag, v0, done: 0. });
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
                if phase != DispatchPhase::Capture
                    || !hitbox.should_handle_scroll(window)
                    || crate::motion::Level::current(cx) != crate::motion::Level::Full
                {
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
        let s = Scroller::default();
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
        let s = Scroller::default();
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
        let s = Scroller::default();
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
        let s = Scroller::default();
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
        let s = Scroller::default();
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
        // Short of an end not yet measured, it stops 2 px before it.
        let s = Scroller::default();
        s.wheel(&lines(-30.), t0);
        let step = s.frame(t0 + ms(100), Room { offset: 99_900., end_known: false, ..ROOM });
        assert_eq!(step, super::Step { by: 98., follow: false });
    }

    /// A packet during a coast is the finger back down: the list doesn't jump back.
    #[test]
    fn a_resumed_finger_takes_the_coast_back() {
        let s = Scroller::default();
        let t0 = Instant::now();
        let mut next = 0u32;
        let path = run(&s, t0, 120, 0., |k, now| {
            // 10 packets, a 40 ms hole (a coast starts), then 10 more.
            let due = |i: u32| t0 + ms(8 * i as u64 + if i >= 10 { 40 } else { 0 });
            while next < 20 && due(next) <= now {
                assert!(s.wheel(&lines(-0.8 - 0.013), now));
                next += 1;
            }
            let _ = k;
        });
        let v: Vec<f32> = path.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(v.iter().all(|d| *d >= 0.), "never backwards: {v:?}");
    }

    /// A jump far away cuts to a viewport short, then glides the rest.
    #[test]
    fn a_far_jump_cuts_then_glides() {
        let s = Scroller::default();
        let t0 = Instant::now();
        let cut = s.jump(5000., false, 900., t0);
        assert_eq!(cut, 4100.);
        let path = run(&s, t0, 80, 4100., |_, _| {});
        assert!((path[79] - 5000.).abs() < 0.01);
        assert!(path.windows(2).all(|w| w[1] >= w[0]));
    }

    /// A touchscreen (GPUI coasts it) and sideways scrolling are not ours.
    #[test]
    fn touch_and_sideways_pass_through() {
        let s = Scroller::default();
        let mut e = lines(-3.);
        e.source = ScrollSource::Touch;
        assert!(!s.wheel(&e, Instant::now()));
        let mut e = lines(0.);
        e.delta = ScrollDelta::Lines(point(-3., 0.));
        assert!(!s.wheel(&e, Instant::now()));
    }
}
