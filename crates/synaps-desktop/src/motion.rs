//! Motion (cohesion/motion.md): phosphor. Light comes on at once and
//! decays; things that arrive draw their edges; running things scan on one
//! 8 Hz heartbeat; words never move and never wait.
//!
//! An additive layer over the static contract: every value drawn here ends
//! at what the static app draws, and with [`Level::Off`] nothing here asks
//! for a frame, arms a timer or changes a colour.
//!
//! One frame source per window ([`Motion`]): during a frame every animated
//! element samples its value from the frame's clock and says when it next
//! changes: the next frame (display rate), the next 30 Hz tick (colour
//! decays), or the next beat (ambient). After the frame
//! ([`Motion::end_frame`], painted last) the window asks for one animation
//! frame, or arms **one** timer to the earliest change, or nothing. So N
//! animated things cost one frame, and an idle window draws none.
//!
//! What changes on the beat alone (a running glyph's scan row, a breath's
//! cell) is not the app's frame: the app's view is cached
//! ([`crate::beat::Shell`]), those glyphs are drawn still and say where
//! they were painted ([`Site`]), and a beat repaints only the beat layer
//! over the cached frame ([`Motion::paint_beats`]). A clock's second, a
//! decay, a draw: the app's frame, as before.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::base::motion::Easing;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

// ── the setting ───────────────────────────────────────────────────────

/// Settings → App → Motion (§3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Level {
    Full,
    /// Spatial motion ends at once; colour ≤ [`QUICK`]; no ambient.
    Reduced,
    /// The static app: no frames, no timers.
    Off,
}

impl Level {
    pub(crate) const ALL: [Level; 3] = [Level::Full, Level::Reduced, Level::Off];

    pub(crate) fn key(self) -> &'static str {
        match self {
            Level::Full => "full",
            Level::Reduced => "reduced",
            Level::Off => "off",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<Level> {
        Level::ALL.into_iter().find(|l| l.key().eq_ignore_ascii_case(s.trim()))
    }

    /// The level in force: the pinned one (a choice or a lock), else the
    /// OS's (gpui-base reads it into the kit's flag, and follows it).
    pub(crate) fn current(cx: &App) -> Level {
        match cx.try_global::<Setting>().and_then(|s| s.level) {
            Some(l) => l,
            None if cx.reduce_motion() => Level::Reduced,
            None => Level::Full,
        }
    }

    /// The kit's own motion follows (`cx.set_reduce_motion`): a pinned
    /// level wins over the OS; unset, the OS's stays.
    pub(crate) fn apply(cx: &mut App) {
        if let Some(l) = cx.try_global::<Setting>().and_then(|s| s.level) {
            cx.set_reduce_motion(l != Level::Full);
        }
        // A global: cached views don't see it change (the app's view is one).
        cx.refresh_windows();
    }
}

/// Why the level cannot be chosen in Settings this run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lock {
    /// `--motion`.
    Flag,
    /// `SYNAPS_NO_BOOT_FX=1` (the TUI's switch): Off.
    NoBootFx,
}

/// The TUI's own switch: set, motion is off.
pub(crate) const ENV_NO_BOOT_FX: &str = "SYNAPS_NO_BOOT_FX";

/// The app's Motion setting (a `Global`): the pinned level, if any.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Setting {
    pub(crate) level: Option<Level>,
    pub(crate) lock: Option<Lock>,
}

impl Global for Setting {}

impl Setting {
    /// `SYNAPS_NO_BOOT_FX=1` (the TUI's switch, exactly), else `--motion`,
    /// else the saved choice, else unset (the OS's).
    pub(crate) fn resolve(flag: Option<&str>, no_boot_fx: Option<&str>, saved: Option<&str>) -> (Setting, Option<String>) {
        if no_boot_fx == Some("1") {
            return (Setting { level: Some(Level::Off), lock: Some(Lock::NoBootFx) }, None);
        }
        if let Some(f) = flag {
            return match Level::parse(f) {
                Some(l) => (Setting { level: Some(l), lock: Some(Lock::Flag) }, None),
                None => {
                    (Setting { level: saved.and_then(Level::parse), lock: None }, Some(format!("--motion takes full, reduced or off: {f}")))
                }
            };
        }
        (Setting { level: saved.and_then(Level::parse), lock: None }, None)
    }
}

// ── tokens (§4) ───────────────────────────────────────────────────────

pub(crate) const QUICK: Duration = Duration::from_millis(120);
pub(crate) const BASE: Duration = Duration::from_millis(200);
pub(crate) const SLOW: Duration = Duration::from_millis(360);
pub(crate) const GLOW: Duration = Duration::from_millis(480);
pub(crate) const SIG: Duration = Duration::from_millis(600);
pub(crate) const STAGGER: Duration = Duration::from_millis(30);
pub(crate) const STAGGER_CAP: Duration = Duration::from_millis(150);
/// Overlays rise this far from their anchor (PR 5: `overlay.rs`).
#[allow(dead_code)]
pub(crate) const RISE: f32 = 4.;
pub(crate) const SLIDE_CAP: f32 = 48.;
/// The heartbeat: 8 Hz, phase-locked app-wide.
pub(crate) const BEAT: Duration = Duration::from_millis(125);
/// Colour-only motion of `slow` and longer: 30 Hz (the kit's stream-fade tick).
pub(crate) const DECAY_TICK: Duration = Duration::from_nanos(33_333_333);
/// The write head stays lit this long after the last delta.
pub(crate) const HEAD_HOLD: Duration = Duration::from_millis(160);
/// The needs-you breath: 19 beats a cycle (2.4 s), 3 cycles, then held.
pub(crate) const BREATH_BEATS: u32 = 19;
pub(crate) const BREATHS: u32 = 3;

/// An exit's duration: ×0.75.
pub(crate) const fn exit(d: Duration) -> Duration {
    Duration::from_nanos(d.as_nanos() as u64 * 3 / 4)
}

/// Item `i` of a burst: [`STAGGER`] each, the sixth onward with the fifth.
pub(crate) fn stagger(i: usize) -> Duration {
    (STAGGER * i as u32).min(STAGGER_CAP)
}

/// The curves (§4.2). Never `ease`, `linear` (unstepped), bounce or overshoot.
pub(crate) mod ease {
    use gpui_kit::base::motion::Easing;

    /// Enters, draws, traces, rises, decays.
    pub(crate) const OUT: Easing = Easing::CubicBezier { x1: 0.16, y1: 1., x2: 0.3, y2: 1. };
    /// Exits.
    pub(crate) const IN: Easing = Easing::CubicBezier { x1: 0.7, y1: 0., x2: 0.84, y2: 0. };
    /// Cross-fades and the fold chevron's turn.
    pub(crate) const INOUT: Easing = Easing::CubicBezier { x1: 0.65, y1: 0., x2: 0.35, y2: 1. };
}

/// `snap`: a critically damped spring, response 220 ms, settled at 0.5 px.
pub(crate) const SNAP_RESPONSE: f32 = 0.22;
const SNAP_EPSILON: f32 = 0.5;

/// `a` toward `b` by `t` (0..1), in sRGB: what an opacity paints on an
/// opaque ground.
pub(crate) fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (x, y) = (Rgba::from(a), Rgba::from(b));
    let t = t.clamp(0., 1.);
    let l = |p: f32, q: f32| p + (q - p) * t;
    Hsla::from(Rgba { r: l(x.r, y.r), g: l(x.g, y.g), b: l(x.b, y.b), a: l(x.a, y.a) })
}

// ── fluid (cohesion/fluid.md §2): matter on springs ─────────────────

/// `wheel`: wheel-notch scrolling (91 ms to 95 %).
pub(crate) const WHEEL_RESPONSE: f32 = 0.12;
/// `flick`: small travel (≤ 16 px): arrival offsets, streamed line growth.
pub(crate) const FLICK_RESPONSE: f32 = 0.16;
/// `glide`: scroll jumps, a view arriving.
pub(crate) const GLIDE_RESPONSE: f32 = 0.3;
/// An arriving block's or word's first strength: never lower (law 3).
pub(crate) const LIFT: f32 = 0.7;
/// A block or row arriving in a list travels this far, from below.
pub(crate) const ARRIVE: f32 = 8.;
/// A view (session, tab, worker) arriving travels this far, in its direction.
pub(crate) const SHIFT: f32 = 12.;
/// A streamed word reaches full strength over this.
pub(crate) const STREAM_FADE: Duration = Duration::from_millis(180);
/// Each further word of one update starts this much later.
pub(crate) const STREAM_STAGGER: Duration = Duration::from_millis(12);
/// Positions settle within this (px).
const FLUID_EPSILON: f32 = 0.5;

/// `stream`: a streamed word's strength, piecewise linear 0.70 at 0, 0.88
/// at 0.35, 1.0 at 1: it starts at the floor, so no word is ever faint.
pub(crate) fn stream_curve() -> Easing {
    use gpui_kit::base::motion::LinearStop;
    Easing::linear_stops([LinearStop::at(LIFT, 0.), LinearStop::at(0.88, 0.35), LinearStop::at(1., 1.)]).unwrap_or(Easing::Linear)
}

/// The one way a text view gets a motion policy (fluid.md §3.4): words
/// streamed into it start at [`LIFT`] and reach full colour over
/// [`STREAM_FADE`]. The kit plays it only while its reduce-motion flag is
/// clear ([`Level::apply`]): Full.
pub(crate) fn text_motion() -> gpui_kit::base::TextViewMotion {
    gpui_kit::base::TextViewMotion::default()
        .with_stream_fade(STREAM_FADE)
        .with_stream_fade_stagger(STREAM_STAGGER)
        .with_stream_fade_easing(stream_curve())
}

/// The one sanctioned opacity (fluid.md §3.6, law 3): something arriving
/// lifts from [`LIFT`] to full, never lower. Opaque grounds only.
#[allow(clippy::disallowed_methods)]
pub(crate) fn lift<E: Styled>(el: E, strength: f32) -> E {
    if strength >= 1. { el } else { el.opacity(strength.max(LIFT)) }
}

/// A critically damped spring's state (ζ = 1): its displacement `x` from
/// the target and its velocity `v` (per second). Stepped analytically, so
/// any frame interval samples the same curve; a retarget shifts `x` by the
/// move and keeps the velocity.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Spring {
    pub(crate) x: f32,
    pub(crate) v: f32,
}

impl Spring {
    /// `dt` seconds on, `response` the undamped period.
    pub(crate) fn step(&mut self, response: f32, dt: f32) {
        let w = std::f32::consts::TAU / response;
        let e = (-w * dt).exp();
        let c = self.v + w * self.x;
        (self.x, self.v) = ((self.x + c * dt) * e, (self.v - w * c * dt) * e);
    }

    pub(crate) fn settled(&self, response: f32) -> bool {
        self.x.abs() < FLUID_EPSILON && self.v.abs() < FLUID_EPSILON * std::f32::consts::TAU / response
    }

    /// The displacement `t` seconds after a rest at `x0`.
    pub(crate) fn from_rest(x0: f32, response: f32, t: f32) -> f32 {
        let mut s = Spring { x: x0, v: 0. };
        s.step(response, t.max(0.));
        s.x
    }
}

/// How a row is drawn this frame ([`Motion::grow`]): clipped to `shown`
/// (none: its own height), where its measured height goes, and (arriving)
/// its content's travel from below and strength.
#[derive(Clone, Debug)]
pub(crate) struct Grow {
    /// The row's key (its display id).
    pub(crate) key: u64,
    pub(crate) shown: Option<f32>,
    pub(crate) sink: Rc<Cell<f32>>,
    pub(crate) travel: f32,
    pub(crate) strength: f32,
}

#[derive(Clone, Debug)]
struct Growing {
    /// Its row in the list (this frame's).
    ix: usize,
    shown: f32,
    spring: Spring,
    arriving: bool,
    born: Option<Instant>,
    at: Instant,
    frame: u64,
    sink: Rc<Cell<f32>>,
}

/// The wrapper a growing row is drawn in: `g.shown` tall, its body laid
/// out at its own height as the wrapper's child (one layout, not measured
/// apart: no double layout however many rows arrive), that height sent to
/// the sink for the next frame, and the body clipped to the wrapper's top
/// and bottom only ([`ClipY`]). What hangs in the gutter is drawn outside
/// the clip: the `label` (positioned, and at `top` in the row) travels and
/// lifts with the body, whole from the first frame; the write head `head`
/// sits on the wrapper's bottom, the row's shown bottom.
pub(crate) fn grow_block(body: AnyElement, label: Option<(Div, f32)>, head: Option<Div>, g: Grow) -> AnyElement {
    let (sink, key) = (g.sink.clone(), g.key);
    let content = div().w_full().flex_shrink_0().relative().top(px(g.travel)).child(body);
    let clip = div()
        .w_full()
        .flex()
        .flex_col()
        .debug_selector(move || format!("grow-clip-{key}"))
        .when_some(g.shown, |d, h| d.h(px(h)))
        .child(lift(content, g.strength))
        .on_children_prepainted(move |b, _, _| {
            if let Some(b) = b.first() {
                sink.set(f32::from(b.size.height));
            }
        });
    div()
        .relative()
        .w_full()
        .flex()
        .flex_col()
        .child(ClipY(clip.into_any_element()))
        .children(label.map(|(l, top)| lift(l.top(px(top + g.travel)), g.strength)))
        .children(head)
        .into_any_element()
}

/// Its child, clipped to its own top and bottom and nothing else: a
/// revealing row's gutter (the hanging labels, the write head, left of the
/// row's box) stays drawn. GPUI's `overflow_*` clip x as well, whichever
/// axis they hide. The list's own mask still bounds x.
struct ClipY(AnyElement);

impl ClipY {
    fn mask(b: Bounds<Pixels>) -> ContentMask<Pixels> {
        let wide = px(1.0e6);
        ContentMask { bounds: Bounds::from_corners(point(b.left() - wide, b.top()), point(b.right() + wide, b.bottom())) }
    }
}

impl IntoElement for ClipY {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for ClipY {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.0.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_content_mask(Some(Self::mask(bounds)), |window| {
            self.0.prepaint(window, cx);
        });
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_content_mask(Some(Self::mask(bounds)), |window| self.0.paint(window, cx));
    }
}

// ── what a value is, and how it runs ──────────────────────────────────

/// How a one-shot runs under each level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Draws, traces, write-ons, turns: Reduced ends them at once.
    Spatial,
    /// Colour at display rate: Reduced shortens it to ≤ [`QUICK`].
    Fade,
    /// Colour at 30 Hz (`slow`, `glow`): Reduced ≤ [`QUICK`], at display rate.
    Decay,
}

/// What an element remembers of a value between frames ([`Motion::change`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Tag {
    /// A call's trail glyph.
    Outcome,
    /// A prompt waits on the call (its warn bar).
    Waiting,
    /// A call finished (its lit bar settles).
    Finished,
    /// A worker block under its call (its rule draws).
    Block,
    /// A worker block's glyph.
    BlockGlyph,
    /// The prompt on the card.
    Card,
    /// The composer foot's glyph.
    Foot,
    /// A tray row's glyph.
    TrayGlyph,
    /// A tray row's watched bar.
    Watched,
    /// A streaming row's text length (the write head).
    Stream,
}

impl Tag {
    /// Keyed by the column's own ids (a transcript's item ids, which every
    /// transcript numbers from 1): another column's are other things.
    fn column(self) -> bool {
        matches!(self, Tag::Outcome | Tag::Waiting | Tag::Finished | Tag::Block | Tag::BlockGlyph | Tag::Stream)
    }
}

/// What a glyph does on the beat alone, painted by the beat layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Beat {
    /// A running glyph's scan row ([`crate::glyph::scan`]).
    Scan(crate::glyph::Glyph),
    /// `Needs` breathing since the prompt came ([`crate::glyph::breath`]).
    Breath(Instant),
}

/// A glyph drawn still in the app's frame whose beat the beat layer
/// paints: where it was painted (its box's origin, under its clip), and
/// in what.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Site {
    pub(crate) origin: Point<Pixels>,
    pub(crate) mask: ContentMask<Pixels>,
    pub(crate) mark: Hsla,
    pub(crate) ground: Hsla,
    /// The ink a scan's beam is lit toward ([`crate::glyph::beam`]).
    pub(crate) ink: Hsla,
    pub(crate) beat: Beat,
}

/// Who the one timer wakes: the app's view (a frame of its own) or only
/// the beat layer (the beat, over the cached frame).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Wake {
    App,
    Beat,
}

#[derive(Clone, Copy, Debug)]
struct Seen {
    value: u64,
    prev: u64,
    since: Option<Instant>,
    frame: u64,
}

/// A value retargeted mid-flight starts from where it is (a reversal
/// shortens), never from an end: the fold chevron's angle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Tween {
    from: f32,
    to: f32,
    at: Instant,
    dur: Duration,
}

impl Tween {
    pub(crate) fn rest(v: f32, now: Instant) -> Self {
        Tween { from: v, to: v, at: now, dur: Duration::ZERO }
    }

    /// Its value at `now` (`full`: the duration of a whole 0 → 1 travel).
    pub(crate) fn sample(&self, now: Instant, ease: &Easing) -> f32 {
        if self.dur.is_zero() {
            return self.to;
        }
        let t = (now.saturating_duration_since(self.at).as_secs_f32() / self.dur.as_secs_f32()).clamp(0., 1.);
        self.from + (self.to - self.from) * ease.sample(t)
    }

    pub(crate) fn running(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.at) < self.dur
    }

    /// Toward `to` from its value at `now`, over `full` × the distance.
    pub(crate) fn retarget(&mut self, to: f32, now: Instant, full: Duration, ease: &Easing) {
        if to == self.to {
            return;
        }
        let from = self.sample(now, ease);
        *self = Tween { from, to, at: now, dur: full.mul_f32((to - from).abs().min(1.)) };
    }
}

/// A sprung paint offset (`Flip`): a row that changed place starts where
/// it was and settles on its new place, critically damped (no overshoot).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Slide {
    target: f32,
    x0: f32,
    v0: f32,
    at: Instant,
    frame: u64,
}

impl Slide {
    fn omega() -> f32 {
        std::f32::consts::TAU / SNAP_RESPONSE
    }

    /// Offset and velocity at `now`.
    pub(crate) fn state(&self, now: Instant) -> (f32, f32) {
        let (w, t) = (Self::omega(), now.saturating_duration_since(self.at).as_secs_f32());
        let e = (-w * t).exp();
        let c = self.v0 + w * self.x0;
        ((self.x0 + c * t) * e, (self.v0 - w * c * t) * e)
    }

    pub(crate) fn settled(&self, now: Instant) -> bool {
        let (x, v) = self.state(now);
        x.abs() < SNAP_EPSILON && v.abs() < SNAP_EPSILON * Self::omega()
    }

    /// A new place: the offset keeps the row where it is (capped at
    /// [`SLIDE_CAP`] on its true side) and its velocity.
    pub(crate) fn retarget(&mut self, target: f32, now: Instant) {
        if target == self.target {
            return;
        }
        let (x, v) = self.state(now);
        let x = (x + self.target - target).clamp(-SLIDE_CAP, SLIDE_CAP);
        *self = Slide { target, x0: x, v0: v, at: now, frame: self.frame };
    }
}

/// One window's motion: the frame's clock, what the frame asked for, and
/// what elements remember between frames (kept only for what was drawn in
/// the last frame: nothing grows with history).
pub(crate) struct Motion {
    level: Cell<Level>,
    /// The palette's ink, as of the frame ([`crate::glyph::beam`]).
    ink: Cell<Hsla>,
    ambient: Cell<bool>,
    visible: Cell<bool>,
    now: Cell<Instant>,
    frame: Cell<u64>,
    in_frame: Cell<bool>,
    /// The app epoch the beat is phase-locked to.
    epoch: Instant,
    /// A display-rate value is moving: the next frame.
    display: Cell<bool>,
    /// The earliest timed change (a 30 Hz tick, a beat, a second).
    next: Cell<Option<Instant>>,
    seen: RefCell<HashMap<(Tag, u64), Seen>>,
    births: RefCell<HashMap<u64, Instant>>,
    slides: RefCell<HashMap<u64, Slide>>,
    turns: RefCell<HashMap<u64, (Tween, u64)>>,
    /// Rows revealing or growing ([`Motion::grow`]).
    grows: RefCell<HashMap<u64, Growing>>,
    /// Rows born that will not reveal: first drawn off screen, in the
    /// list's overdraw ([`Motion::unpainted`]).
    quiet: RefCell<std::collections::HashSet<u64>>,
    /// The one timer.
    timer: RefCell<Option<Task<()>>>,
    /// The instant it is armed for, and whom it wakes (cleared as it fires).
    armed: Rc<Cell<Option<(Instant, Wake)>>>,
    /// The instant the timer last fired for ([`Motion::tick`]).
    woke: Rc<Cell<Option<Instant>>>,
    /// A beat layer paints the beat ([`crate::beat::BeatLayer`]); without
    /// one the app's frame draws it (and each beat is the app's frame).
    layer: Cell<bool>,
    /// Glyphs whose beat the layer paints, recorded by the app's last paint.
    sites: RefCell<Vec<Site>>,
    /// The next beat the layer's sites change on.
    beat_next: Cell<Option<Instant>>,
    /// Frames the beat layer painted (tests, the bench).
    pub(crate) layer_paints: Cell<u64>,
    /// Timers armed so far (tests: one per change of deadline).
    pub(crate) arms: Cell<u64>,
    /// Handoff out: you sent (`false`) or steered a worker (`true`).
    pub(crate) sent: Cell<Option<(Instant, bool)>>,
    /// A fold or unfold: the clicked run's id, when (the cascade's origin).
    pub(crate) fold: Cell<Option<(u64, Instant)>>,
    /// Whose state is remembered: the session's (id and attach) and the
    /// column's (the session's thread or a worker's view) ([`Motion::rescope`]).
    scope: Cell<(u64, u64)>,
    /// Tests move the clock with the executor's ([`Motion::clock`]): the
    /// clock's offset from the wall clock, in signed nanoseconds.
    #[cfg(test)]
    skew: Cell<i128>,
    /// Tests: the clock stands at this instant until set ([`Motion::pin_clock`]).
    #[cfg(test)]
    pin: Cell<Option<Instant>>,
    /// Tests: the OS in low power ([`Motion::low_power`]).
    #[cfg(test)]
    pub(crate) force_low_power: Cell<bool>,
    /// Tests: the beat each layer paint with sites drew, in order.
    #[cfg(test)]
    pub(crate) painted: RefCell<Vec<u32>>,
}

impl Default for Motion {
    fn default() -> Self {
        Motion::new(Instant::now())
    }
}

impl Motion {
    pub(crate) fn new(epoch: Instant) -> Self {
        Motion {
            level: Cell::new(Level::Full),
            ink: Cell::new(Hsla::default()),
            ambient: Cell::new(true),
            visible: Cell::new(true),
            now: Cell::new(epoch),
            frame: Cell::new(0),
            in_frame: Cell::new(false),
            epoch,
            display: Cell::new(false),
            next: Cell::new(None),
            seen: RefCell::default(),
            births: RefCell::default(),
            slides: RefCell::default(),
            turns: RefCell::default(),
            grows: RefCell::default(),
            quiet: RefCell::default(),
            timer: RefCell::default(),
            armed: Rc::default(),
            woke: Rc::default(),
            layer: Cell::new(false),
            sites: RefCell::default(),
            beat_next: Cell::new(None),
            layer_paints: Cell::new(0),
            arms: Cell::new(0),
            sent: Cell::new(None),
            fold: Cell::new(None),
            scope: Cell::new((0, 0)),
            #[cfg(test)]
            skew: Cell::new(0),
            #[cfg(test)]
            pin: Cell::new(None),
            #[cfg(test)]
            force_low_power: Cell::new(false),
            #[cfg(test)]
            painted: RefCell::default(),
        }
    }

    /// Who is on screen this frame: `session` (its id and attach) and
    /// `column` (its thread or a worker's view). What was remembered under
    /// another is forgotten, so nothing compares one's ids with another's:
    /// a switch, a reconnect, a worker's view in or out come up at rest
    /// (motion.md §8). The tray, the foot and the card are the session's;
    /// the transcript's tags, turns and births are the column's.
    pub(crate) fn rescope(&self, session: u64, column: u64) {
        let (s0, c0) = self.scope.get();
        if s0 == session && c0 == column {
            return;
        }
        self.scope.set((session, column));
        if s0 != session {
            self.seen.borrow_mut().clear();
            self.slides.borrow_mut().clear();
            self.sent.set(None);
        } else {
            self.seen.borrow_mut().retain(|k, _| !k.0.column());
        }
        self.turns.borrow_mut().clear();
        self.grows.borrow_mut().clear();
        self.quiet.borrow_mut().clear();
        self.births.borrow_mut().clear();
        self.fold.set(None);
    }

    /// Fluid motion the agent causes plays (fluid.md §5): Full, and not in
    /// low power, which stops it (the user's scroll asks [`Motion::level`]).
    pub(crate) fn fluid(&self) -> bool {
        self.ambient.get()
    }

    /// Row `key`'s height this frame (fluid.md §3.3, §3.4), or none: drawn
    /// as is (Reduced, Off, low power, history, at rest).
    /// - `born` (it arrived while watched): its height reveals 0 → its own
    ///   on `snap`, clipped. Its first frame is 0 px tall (measured, not
    ///   shown), so the reveal runs to its true height from the start.
    ///   Inside, the content travels [`ARRIVE`] → 0 (`flick`) and lifts
    ///   [`LIFT`] → 1 (`quick`, `out`).
    /// - `streaming`: a height that grows (a wrapped line) glides there on
    ///   `flick` (an arrival's reveal on `snap`), never more than `line`
    ///   short of its true height once it has arrived; it shrinks at once.
    ///   The height is the last frame's measure: a chunk of more than a
    ///   line shows one frame (7 ms at 144 Hz) at the height before it.
    ///
    /// Settled, a row that no longer streams is unwrapped (none): a row is
    /// wrapped only while it moves or streams. `ix`: its row in the list.
    pub(crate) fn grow(&self, key: u64, ix: usize, born: Option<Instant>, streaming: bool, line: f32) -> Option<Grow> {
        if !self.fluid() {
            return None;
        }
        let (frame, now) = (self.frame.get(), self.now());
        let mut grows = self.grows.borrow_mut();
        let fresh = born.is_some_and(|b| now.saturating_duration_since(b) < QUICK) && !self.quiet.borrow().contains(&key);
        let g = match grows.get_mut(&key) {
            Some(g) if g.frame + 1 >= frame => g,
            _ if fresh || streaming => {
                let g = Growing {
                    ix,
                    shown: f32::NAN,
                    spring: Spring::default(),
                    arriving: fresh,
                    born: born.filter(|_| fresh),
                    at: now,
                    frame,
                    sink: Rc::new(Cell::new(f32::NAN)),
                };
                grows.insert(key, g);
                grows.get_mut(&key)?
            }
            _ => return None,
        };
        let dt = now.saturating_duration_since(g.at).as_secs_f32().min(0.05);
        (g.at, g.frame, g.ix) = (now, frame, ix);
        let natural = g.sink.get();
        let since = g.born.map_or(1., |b| now.checked_duration_since(b).map_or(0., |d| d.as_secs_f32()));
        let (travel, strength) = if g.arriving {
            let q = (since / QUICK.as_secs_f32()).clamp(0., 1.);
            (Spring::from_rest(ARRIVE, FLICK_RESPONSE, since), LIFT + (1. - LIFT) * ease::OUT.sample(q))
        } else {
            (0., 1.)
        };
        let content_moves = g.arriving && (travel >= FLUID_EPSILON || strength < 1.);
        if natural.is_nan() {
            // Not measured yet: an arrival is 0 px tall; a streaming row its own height.
            self.want_frame();
            return Some(Grow { key, shown: g.arriving.then_some(0.), sink: g.sink.clone(), travel, strength });
        }
        if g.shown.is_nan() {
            g.shown = if g.arriving { 0. } else { natural };
        }
        if g.born.is_none_or(|b| b <= now) {
            if natural < g.shown {
                (g.shown, g.spring) = (natural, Spring::default());
            } else {
                let response = if g.arriving { SNAP_RESPONSE } else { FLICK_RESPONSE };
                g.spring.x = g.shown - natural;
                g.spring.step(response, dt);
                if streaming && !g.arriving {
                    // The newest line is never more than a line away (an
                    // arrival reveals all of itself on `snap` first).
                    g.spring.x = g.spring.x.max(-line);
                }
                g.shown = natural + g.spring.x;
                if g.spring.settled(response) {
                    (g.shown, g.spring) = (natural, Spring::default());
                }
            }
        }
        // Arrived: its content at rest, and (unless it streams on) all of it
        // shown. A streaming arrival's new lines then glide, a line short at most.
        if g.arriving && !content_moves && (streaming || g.shown == natural) {
            g.arriving = false;
        }
        if g.shown == natural && !content_moves {
            if !streaming {
                grows.remove(&key);
                return None;
            }
        } else {
            self.want_frame();
        }
        Some(Grow { key, shown: Some(g.shown), sink: g.sink.clone(), travel, strength })
    }

    /// After the list's layout: arrivals it drew this frame but did not
    /// paint (laid out in its overdraw, off screen) have no measure, and
    /// GPUI keeps the 0 px they were drawn at. They don't reveal: they come
    /// back at their height, and their rows (returned) are measured again.
    pub(crate) fn unpainted(&self) -> Vec<usize> {
        let frame = self.frame.get();
        let mut out = Vec::new();
        self.grows.borrow_mut().retain(|key, g| {
            let off = g.frame == frame && g.arriving && g.sink.get().is_nan();
            if off {
                self.quiet.borrow_mut().insert(*key);
                out.push(g.ix);
            }
            !off
        });
        out
    }

    /// Rows revealing or growing now (tests).
    #[cfg(test)]
    pub(crate) fn growing(&self) -> usize {
        self.grows.borrow().len()
    }

    /// Row `key` is revealing as it arrives (tests).
    #[cfg(test)]
    pub(crate) fn arriving(&self, key: u64) -> bool {
        self.grows.borrow().get(&key).is_some_and(|g| g.arriving)
    }

    /// Row `key`'s drawn and measured heights (tests).
    #[cfg(test)]
    pub(crate) fn grow_of(&self, key: u64) -> Option<(f32, f32)> {
        self.grows.borrow().get(&key).map(|g| (g.shown, g.sink.get()))
    }

    pub(crate) fn level(&self) -> Level {
        self.level.get()
    }

    /// The palette's ink, set as a frame begins (a scan's beam).
    pub(crate) fn set_ink(&self, ink: Hsla) {
        self.ink.set(ink);
    }

    pub(crate) fn ink(&self) -> Hsla {
        self.ink.get()
    }

    /// Motion is on at all (not [`Level::Off`]).
    pub(crate) fn on(&self) -> bool {
        self.level.get() != Level::Off
    }

    /// The frame's clock (every element of a frame samples the same instant).
    pub(crate) fn now(&self) -> Instant {
        if self.in_frame.get() { self.now.get() } else { self.clock() }
    }

    /// A frame's instant: the clock, or the instant the timer fired for if
    /// it fired a hair early (a coarse OS timer): the beat it was armed for
    /// is the one drawn, so an early wake is not a second present of the
    /// beat before (Shady N2). Once that instant is past, the clock.
    pub(crate) fn tick(&self) -> Instant {
        let now = self.clock();
        self.woke.get().map_or(now, |due| due.max(now))
    }

    /// The OS is in low power: no ambient (§7).
    pub(crate) fn low_power(&self, cx: &App) -> bool {
        #[cfg(test)]
        if self.force_low_power.get() {
            return true;
        }
        cx.power_mode() == gpui_kit::PowerMode::LowPower
    }

    /// The time (tests: moved on with the test executor's clock).
    pub(crate) fn clock(&self) -> Instant {
        #[cfg(test)]
        if let Some(at) = self.pin.get() {
            return at;
        }
        let now = Instant::now();
        #[cfg(test)]
        let now = {
            let (s, d) = (self.skew.get(), Duration::from_nanos(self.skew.get().unsigned_abs() as u64));
            if s >= 0 { now + d } else { now - d }
        };
        now
    }

    /// Tests: the clock reads `at` now (then runs on with the wall clock).
    /// A test's beat sets it a millisecond at a time with the executor's,
    /// so a slow runner's wall time doesn't run the beat ahead of the
    /// executor's timers.
    #[cfg(test)]
    pub(crate) fn set_clock(&self, at: Instant) {
        if self.pin.get().is_some() {
            return self.pin.set(Some(at));
        }
        let now = Instant::now();
        let s = if at >= now { at.duration_since(now).as_nanos() as i128 } else { -(now.duration_since(at).as_nanos() as i128) };
        self.skew.set(s);
    }

    /// Tests: the clock stops where it is and moves only when set, with the
    /// executor's ([`Motion::set_clock`]): what a test sees doesn't depend on
    /// how long its runner took between two steps.
    #[cfg(test)]
    pub(crate) fn pin_clock(&self) {
        self.pin.set(Some(self.clock()));
    }

    /// Tests: the last frame's instant (what its timer was armed from).
    #[cfg(test)]
    pub(crate) fn frame_now(&self) -> Instant {
        self.now.get()
    }

    /// A frame starts (the root's render): the clock, the level, and what
    /// was not drawn last frame is forgotten (it comes back at rest).
    pub(crate) fn begin_frame(&self, level: Level, low_power: bool, now: Instant) {
        self.level.set(level);
        self.ambient.set(level == Level::Full && !low_power);
        self.now.set(now);
        let frame = self.frame.get() + 1;
        self.frame.set(frame);
        self.in_frame.set(true);
        self.display.set(false);
        self.next.set(None);
        self.sites.borrow_mut().clear();
        let alive = |f: u64| f + 1 >= frame;
        self.seen.borrow_mut().retain(|_, s| alive(s.frame));
        self.slides.borrow_mut().retain(|_, s| alive(s.frame));
        self.turns.borrow_mut().retain(|_, t| alive(t.1));
        self.grows.borrow_mut().retain(|_, g| alive(g.frame));
        let cut = BASE + STAGGER_CAP + SIG;
        self.births.borrow_mut().retain(|_, at| now.saturating_duration_since(*at) < cut);
        let births = self.births.borrow();
        self.quiet.borrow_mut().retain(|k| births.contains_key(k));
    }

    /// The window is shown or hidden: hidden, the timer stops (visible
    /// again, the next frame arms it).
    pub(crate) fn set_visible(&self, visible: bool) {
        self.visible.set(visible);
        if !visible {
            self.disarm();
        }
    }

    /// A beat layer paints the beat from now on ([`crate::beat::BeatLayer`]).
    pub(crate) fn set_layer(&self) {
        self.layer.set(true);
    }

    /// The beat goes to the beat layer this frame: ambient runs, and a
    /// layer is there to paint it.
    pub(crate) fn layered(&self) -> bool {
        self.layer.get() && self.ambient.get()
    }

    /// AppView frames so far (each one a render, layout and prepaint).
    pub(crate) fn frames(&self) -> u64 {
        self.frame.get()
    }

    /// A glyph drawn still was painted here: the layer paints its beat.
    pub(crate) fn site(&self, site: Site) {
        self.sites.borrow_mut().push(site);
    }

    #[cfg(test)]
    pub(crate) fn sites(&self) -> Vec<Site> {
        self.sites.borrow().clone()
    }

    /// Glyphs the beat layer paints (the bench).
    pub(crate) fn site_count(&self) -> usize {
        self.sites.borrow().len()
    }

    fn disarm(&self) {
        self.timer.borrow_mut().take();
        self.armed.set(None);
    }

    // ── demand ────────────────────────────────────────────────────────

    /// The next frame, please (a display-rate value is moving).
    pub(crate) fn want_frame(&self) {
        if self.on() {
            self.display.set(true);
        }
    }

    /// A change due at `at` (a timer, unless a frame comes first).
    pub(crate) fn want_at(&self, at: Instant) {
        if self.on() && self.next.get().is_none_or(|n| at < n) {
            self.next.set(Some(at));
        }
    }

    /// The next 30 Hz tick, on the shared grid (phase-locked to the
    /// epoch, as the beat is): N decays cost one timer and 30 frames/s,
    /// whenever each started.
    fn want_tick(&self) {
        let ticks = self.now().saturating_duration_since(self.epoch).as_nanos() / DECAY_TICK.as_nanos() + 1;
        self.want_at(self.epoch + Duration::from_nanos((DECAY_TICK.as_nanos() * ticks) as u64));
    }

    /// The beat (§7): its index, and the element changes again in `every`
    /// beats. `None` when ambient is off (Reduced, Off, low power): drawn still.
    pub(crate) fn beat(&self, every: u32) -> Option<u32> {
        if !self.ambient.get() {
            return None;
        }
        let since = self.now().saturating_duration_since(self.epoch);
        let beat = (since.as_nanos() / BEAT.as_nanos()) as u32;
        let next = (beat / every.max(1) + 1) * every.max(1);
        self.want_at(self.epoch + BEAT * next);
        Some(beat)
    }

    /// Beats since `since` (a breath's start), on the shared phase.
    pub(crate) fn beats_since(&self, since: Instant) -> u32 {
        let b = |t: Instant| (t.saturating_duration_since(self.epoch).as_nanos() / BEAT.as_nanos()) as u32;
        b(self.now()).saturating_sub(b(since))
    }

    /// A clock (a fuse's step; a running elapsed time is [`Self::tick_clock`])
    /// changes at `at`: under every level but Off (information, not
    /// motion). While the heartbeat runs it changes on the beat that
    /// crosses `at` (§7.1: the heartbeat's frame, no extra one); without
    /// it, at `at` itself.
    pub(crate) fn tick_at(&self, at: Instant) {
        self.want_at(if self.ambient.get() { self.on_grid(at, BEAT) } else { at });
    }

    /// A running clock's next second at `at`: every clock changes on the
    /// one second of the window's grid that crosses it (8 beats): N clocks,
    /// whatever their phase, cost one app frame a second (while the
    /// heartbeat runs, the rest of the beats only the beat layer). Each
    /// shows its own whole seconds as of that frame (at most one behind
    /// for less than a second). At every level but Off, and in low power,
    /// where frames matter most (Shady N5).
    pub(crate) fn tick_clock(&self, at: Instant) {
        self.want_at(self.on_grid(at, BEAT * 8));
    }

    /// The first instant of the window's `grid` at or after `at`.
    fn on_grid(&self, at: Instant, grid: Duration) -> Instant {
        let n = at.saturating_duration_since(self.epoch).as_nanos().div_ceil(grid.as_nanos());
        self.epoch + Duration::from_nanos((grid.as_nanos() * n) as u64)
    }

    // ── one-shots ─────────────────────────────────────────────────────

    /// Progress (0..1, eased) of a one-shot that started at `since` (none:
    /// at rest, 1), after `delay`, over `dur`. Asks for its frames while it runs.
    pub(crate) fn shot(&self, since: Option<Instant>, delay: Duration, dur: Duration, ease: &Easing, kind: Kind) -> f32 {
        let Some(since) = since else { return 1. };
        let dur = match (self.level.get(), kind) {
            (Level::Off, _) | (Level::Reduced, Kind::Spatial) => return 1.,
            (Level::Reduced, _) => dur.min(QUICK),
            (Level::Full, _) => dur,
        };
        let start = since + delay;
        let now = self.now();
        if dur.is_zero() || now >= start + dur {
            return 1.;
        }
        if now < start {
            // A staggered start: nothing to draw until it comes.
            self.want_at(start);
            return 0.;
        }
        match (self.level.get(), kind) {
            (Level::Full, Kind::Decay) => self.want_tick(),
            _ => self.want_frame(),
        }
        ease.sample(now.saturating_duration_since(start).as_secs_f32() / dur.as_secs_f32())
    }

    /// `draw` (§2.2): a bar's drawn fraction.
    pub(crate) fn draw(&self, since: Option<Instant>, delay: Duration, dur: Duration) -> f32 {
        self.shot(since, delay, dur, &ease::OUT, Kind::Spatial)
    }

    /// `write_on`: a glyph's drawn fraction (its strokes cut at it).
    pub(crate) fn write_on(&self, since: Option<Instant>) -> f32 {
        self.shot(since, Duration::ZERO, QUICK, &ease::OUT, Kind::Spatial)
    }

    /// `decay`: light switched on at `since` going off: 1 (lit) → 0, held
    /// lit for `hold` first.
    pub(crate) fn decay(&self, since: Option<Instant>, hold: Duration, dur: Duration) -> f32 {
        let Some(since) = since else { return 0. };
        if !self.on() {
            return 0.;
        }
        let now = self.now();
        if now < since + hold {
            self.want_at(since + hold);
            return 1.;
        }
        if self.level.get() == Level::Reduced && hold > Duration::ZERO {
            // Reduced: off at once, no decay.
            return 0.;
        }
        1. - self.shot(Some(since + hold), Duration::ZERO, dur, &ease::OUT, Kind::Decay)
    }

    /// Handoff in's light as it travels (motion.md §5.2, §10 amendment 17):
    /// 1 (lit) from `at`, decaying to 0 (the bar) over the last [`GLOW`] of
    /// its [`SIG`]: it lands as the bar, never switched off.
    pub(crate) fn handoff_light(&self, at: Option<Instant>) -> f32 {
        self.decay(at, SIG - GLOW, GLOW)
    }

    /// Handoff's light (motion.md §5.2): dark before `at`, lit at `at`,
    /// decaying over `glow` (Reduced: a 120 ms flash).
    pub(crate) fn flash(&self, at: Option<Instant>) -> f32 {
        let Some(at) = at.filter(|_| self.on()) else { return 0. };
        if self.now() < at {
            self.want_at(at);
            return 0.;
        }
        1. - self.shot(Some(at), Duration::ZERO, GLOW, &ease::OUT, Kind::Decay)
    }

    // ── what elements remember ────────────────────────────────────────

    /// Item `id` arrived while watched (`at`: a burst's staggered start).
    pub(crate) fn born(&self, id: u64, at: Instant) {
        if self.on() {
            self.births.borrow_mut().insert(id, at);
        }
    }

    pub(crate) fn birth(&self, id: u64) -> Option<Instant> {
        self.births.borrow().get(&id).copied()
    }

    /// `value` of `(tag, key)` this frame: when it last changed while
    /// watched, and what it was before. A value seen for the first time, or
    /// not drawn last frame (scrolled back in, a session switch), is at
    /// rest: history never animates. `born`: it arrived just now, from nothing.
    pub(crate) fn change(&self, tag: Tag, key: u64, value: u64, born: Option<Instant>) -> Option<(Instant, u64)> {
        let frame = self.frame.get();
        let mut seen = self.seen.borrow_mut();
        let s = seen.entry((tag, key)).or_insert(Seen { value, prev: u64::MAX, since: born, frame });
        if s.value != value {
            if s.frame + 1 >= frame && self.on() {
                (s.prev, s.since) = (s.value, Some(self.now()));
            } else {
                (s.prev, s.since) = (value, None);
            }
            s.value = value;
        }
        s.frame = frame;
        s.since.map(|t| (t, s.prev))
    }

    /// A row at `y` (its place in its list, keyed `key`): its paint offset.
    pub(crate) fn slide(&self, key: u64, y: f32) -> f32 {
        let (frame, now) = (self.frame.get(), self.now());
        let mut slides = self.slides.borrow_mut();
        let s = slides.entry(key).or_insert(Slide { target: y, x0: 0., v0: 0., at: now, frame });
        if s.frame + 1 < frame || self.level.get() != Level::Full {
            *s = Slide { target: y, x0: 0., v0: 0., at: now, frame };
        }
        s.retarget(y, now);
        s.frame = frame;
        if s.settled(now) {
            return 0.;
        }
        self.want_frame();
        s.state(now).0
    }

    /// A chevron's turn (0 closed … 1 open), from its angle now.
    pub(crate) fn turn(&self, key: u64, open: bool) -> f32 {
        let (frame, now, to) = (self.frame.get(), self.now(), if open { 1. } else { 0. });
        let mut turns = self.turns.borrow_mut();
        let t = turns.entry(key).or_insert((Tween::rest(to, now), frame));
        if t.1 + 1 < frame || self.level.get() != Level::Full {
            t.0 = Tween::rest(to, now);
        }
        t.0.retarget(to, now, QUICK, &ease::INOUT);
        t.1 = frame;
        if t.0.running(now) {
            self.want_frame();
        }
        t.0.sample(now, &ease::INOUT)
    }

    // ── after the frame ───────────────────────────────────────────────

    /// The app's frame is built (painted last): ask for the next frame,
    /// or (no beat layer) arm the one timer to the earliest change, or
    /// nothing. With a layer, it paints next and arms the timer.
    pub(crate) fn end_frame(&self, window: &mut Window, cx: &mut App, app: EntityId) {
        self.in_frame.set(false);
        if !self.on() || !self.visible.get() {
            return self.disarm();
        }
        if self.display.get() {
            // The app's view (the current one) draws the next frame.
            window.request_animation_frame();
            return;
        }
        if !self.layer.get() {
            self.beat_next.set(None);
            self.arm(cx, app, app);
        }
    }

    /// The beat layer's paint, over the app's frame (cached or new): each
    /// recorded glyph's beat now, on one beat (they scan in lockstep), then
    /// the one timer, to the earliest of the app's change and the next beat.
    pub(crate) fn paint_beats(&self, window: &mut Window, cx: &mut App, app: EntityId, layer: EntityId) {
        self.layer_paints.set(self.layer_paints.get() + 1);
        self.beat_next.set(None);
        if !self.on() || !self.visible.get() {
            return self.disarm();
        }
        // Low power: no ambient (§7). GPUI stores a power change without a
        // refresh, so the cached frame's sites would beat on; the app draws
        // instead, sees low power, and records none (Shady N1). Notified
        // after this frame (a notify while painting draws nothing).
        if !self.sites.borrow().is_empty() && self.low_power(cx) {
            cx.defer(move |cx| cx.notify(app));
            return self.disarm();
        }
        let since = self.tick().saturating_duration_since(self.epoch);
        let beat = (since.as_nanos() / BEAT.as_nanos()) as u32;
        let mut wants = false;
        for s in self.sites.borrow().iter() {
            let quad = |x: f32, y: f32, w: f32, h: f32, c: Hsla, window: &mut Window| {
                window.with_content_mask(Some(s.mask), |window| {
                    window.paint_quad(fill(Bounds::new(s.origin + point(px(x), px(y)), size(px(w), px(h))), c));
                });
            };
            match s.beat {
                Beat::Scan(g) => {
                    wants = true;
                    for (x, y, w, c) in crate::glyph::beam_rows(g, beat, s.mark, s.ink) {
                        quad(x, y, w, 1., c, window);
                    }
                }
                Beat::Breath(at) => {
                    let step = beat.saturating_sub((at.saturating_duration_since(self.epoch).as_nanos() / BEAT.as_nanos()) as u32);
                    if step < BREATH_BEATS * BREATHS {
                        wants = true;
                        let (x, y, w) = crate::glyph::BREATH_CELL;
                        quad(x, y, w, w, crate::glyph::breath_cell(step, s.mark, s.ground), window);
                    }
                }
            }
        }
        if wants {
            #[cfg(test)]
            self.painted.borrow_mut().push(beat);
            self.beat_next.set(Some(self.epoch + BEAT * (beat + 1)));
        }
        if !self.display.get() {
            self.arm(cx, app, layer);
        }
    }

    /// Arms the one timer to the earliest change: the app's (it draws a
    /// frame, the layer with it) or the beat's (only the layer).
    fn arm(&self, cx: &mut App, app: EntityId, layer: EntityId) {
        let target = match (self.next.get(), self.beat_next.get()) {
            (Some(a), Some(b)) if b < a => Some((b, Wake::Beat)),
            (Some(a), _) => Some((a, Wake::App)),
            (None, Some(b)) => Some((b, Wake::Beat)),
            (None, None) => None,
        };
        let Some((at, wake)) = target else { return self.disarm() };
        if self.armed.get() == Some((at, wake)) {
            return;
        }
        self.arms.set(self.arms.get() + 1);
        self.armed.set(Some((at, wake)));
        let (armed, woke, who) = (self.armed.clone(), self.woke.clone(), if wake == Wake::App { app } else { layer });
        let wait = at.saturating_duration_since(self.clock());
        let task = cx.spawn(async move |cx| {
            cx.background_executor().timer(wait).await;
            armed.set(None);
            woke.set(Some(at));
            cx.update(|cx| cx.notify(who));
        });
        *self.timer.borrow_mut() = Some(task);
    }

    /// One-shots that started on this frame (tests: a switch starts none).
    /// The watched bar's draw is the one a view opening here plays.
    #[cfg(test)]
    pub(crate) fn fresh(&self) -> Vec<(Tag, u64)> {
        let now = self.now.get();
        let mut v: Vec<(Tag, u64)> =
            self.seen.borrow().iter().filter(|(k, s)| k.0 != Tag::Watched && s.since == Some(now)).map(|(k, _)| *k).collect();
        v.extend(self.births.borrow().iter().filter(|(_, at)| **at >= now).map(|(id, _)| (Tag::Stream, *id)));
        v
    }

    /// The beat the last frame drew (tests).
    #[cfg(test)]
    pub(crate) fn frame_beat(&self) -> u32 {
        (self.now.get().saturating_duration_since(self.epoch).as_nanos() / BEAT.as_nanos()) as u32
    }

    /// Every keyed value that changed (animating) after `t` (tests).
    #[cfg(test)]
    pub(crate) fn changed_since(&self, t: Instant) -> Vec<(Tag, u64)> {
        self.seen.borrow().iter().filter(|(_, s)| s.since.is_some_and(|at| at > t)).map(|(k, _)| *k).collect()
    }

    /// What the last frame asked for: a frame, a timer at, nothing (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn pending(&self) -> (bool, Option<Instant>) {
        (self.display.get(), self.armed.get().map(|(t, _)| t))
    }

    /// Whom the armed timer wakes (tests).
    #[cfg(test)]
    pub(crate) fn wakes(&self) -> Option<Wake> {
        self.armed.get().map(|(_, w)| w)
    }

    /// The scheduler, as `end_frame` would leave it, without a window (tests).
    #[cfg(test)]
    pub(crate) fn demand(&self) -> (bool, Option<Instant>) {
        self.in_frame.set(false);
        (self.display.get(), self.next.get())
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings in gpui's `test` attribute through `gpui_kit::*`.
    use std::time::{Duration, Instant};

    use gpui_kit::{Bounds, point, px, size};

    use super::{
        BASE, BEAT, GLOW, HEAD_HOLD, Kind, Level, Lock, Motion, QUICK, SLIDE_CAP, SLOW, STAGGER, STAGGER_CAP, Setting, Slide, Tag, Tween,
        ease, exit, stagger,
    };

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Off: nothing samples anything but the end state, nothing asks for a
    /// frame or a timer, nothing is remembered as changed.
    #[test]
    fn off_is_static() {
        let t0 = Instant::now();
        let m = Motion::new(t0);
        m.begin_frame(Level::Off, false, t0);
        m.born(7, t0);
        assert_eq!(m.birth(7), None);
        assert_eq!(m.draw(Some(t0), Duration::ZERO, BASE), 1.);
        assert_eq!(m.write_on(Some(t0)), 1.);
        assert_eq!(m.decay(Some(t0), HEAD_HOLD, GLOW), 0.);
        assert_eq!(m.beat(1), None);
        assert_eq!(m.change(Tag::Outcome, 1, 1, None), None);
        m.begin_frame(Level::Off, false, t0 + ms(10));
        assert_eq!(m.change(Tag::Outcome, 1, 2, None), None, "a change under Off is at rest");
        assert_eq!(m.slide(1, 0.), 0.);
        assert_eq!(m.slide(1, 40.), 0.);
        assert_eq!(m.turn(1, true), 1.);
        m.tick_at(t0 + ms(1000));
        assert_eq!(m.demand(), (false, None));
    }

    /// Reduced: spatial motion ends at once, colour runs ≤ quick, no ambient.
    #[test]
    fn reduced_ends_spatial_and_shortens_colour() {
        let t0 = Instant::now();
        let m = Motion::new(t0);
        m.begin_frame(Level::Reduced, false, t0 + ms(10));
        assert_eq!(m.draw(Some(t0), Duration::ZERO, BASE), 1.);
        assert_eq!(m.beat(1), None);
        m.begin_frame(Level::Reduced, false, t0 + ms(130));
        assert_eq!(m.shot(Some(t0), Duration::ZERO, SLOW, &ease::OUT, Kind::Decay), 1., "slow is quick under Reduced");
        // Clocks still tick.
        m.tick_at(t0 + ms(1000));
        assert_eq!(m.demand(), (false, Some(t0 + ms(1000))));
    }

    /// After every one-shot is over, a frame asks for nothing.
    #[test]
    fn idle_requests_no_frames() {
        let t0 = Instant::now();
        let m = Motion::new(t0);
        m.begin_frame(Level::Full, false, t0 + ms(10));
        m.draw(Some(t0), Duration::ZERO, BASE);
        assert!(m.demand().0, "a bar mid-draw wants the next frame");
        m.begin_frame(Level::Full, false, t0 + ms(2000));
        assert_eq!(m.draw(Some(t0), stagger(9), BASE), 1.);
        assert_eq!(m.write_on(Some(t0)), 1.);
        assert_eq!(m.decay(Some(t0), HEAD_HOLD, GLOW), 0.);
        assert_eq!(m.shot(Some(t0), ms(60), GLOW, &ease::OUT, Kind::Decay), 1.);
        assert_eq!(m.turn(3, true), 1.);
        assert_eq!(m.demand(), (false, None));
    }

    /// 25 running glyphs: one deadline, the next beat; 30 Hz decays share
    /// the tick; a clock alone wakes once a second.
    #[test]
    fn heartbeat_is_one_timer() {
        let t0 = Instant::now();
        let m = Motion::new(t0);
        m.begin_frame(Level::Full, false, t0 + ms(130));
        let beats: Vec<_> = (0..25).map(|_| m.beat(1)).collect();
        assert!(beats.iter().all(|b| *b == Some(1)), "lockstep: {beats:?}");
        assert_eq!(m.demand(), (false, Some(t0 + BEAT * 2)));
        m.begin_frame(Level::Full, false, t0 + ms(130));
        m.tick_at(t0 + ms(1000));
        assert_eq!(m.demand(), (false, Some(t0 + ms(1000))), "a clock alone: 1 frame/s");
        // Three decays started 10 ms apart (a turn's end: the write head,
        // two lit bars): one 30 Hz grid, not three.
        let m = Motion::new(t0);
        let starts = [t0 + ms(5), t0 + ms(15), t0 + ms(25)];
        let (mut now, mut frames) = (t0 + ms(30), 0);
        while now < t0 + ms(330) {
            m.begin_frame(Level::Full, false, now);
            for s in starts {
                m.decay(Some(s), Duration::ZERO, SLOW);
            }
            let (display, next) = m.demand();
            assert!(!display, "a decay rides the 30 Hz tick, not display frames");
            let next = next.expect("decaying: a tick");
            assert_eq!(next.duration_since(t0).as_nanos() % super::DECAY_TICK.as_nanos(), 0, "on the shared grid");
            (now, frames) = (next, frames + 1);
        }
        assert_eq!(frames, 10, "the frame at 30 ms, then 300 ms at 30 Hz: whatever the starts");
    }

    /// Hidden or low power: no ambient demand.
    #[test]
    fn heartbeat_stops_low_power() {
        let t0 = Instant::now();
        let m = Motion::new(t0);
        m.begin_frame(Level::Full, true, t0);
        assert_eq!(m.beat(1), None);
        assert_eq!(m.demand(), (false, None));
    }

    /// A target changed mid-flight: the sampled value goes on from where it
    /// was (no jump), for the chevron's turn and for a row's slide.
    #[test]
    fn retarget_is_continuous() {
        let t0 = Instant::now();
        let mut tw = Tween::rest(0., t0);
        tw.retarget(1., t0, QUICK, &ease::INOUT);
        let mid = t0 + ms(50);
        let before = tw.sample(mid, &ease::INOUT);
        assert!(before > 0.05 && before < 0.95, "{before}");
        tw.retarget(0., mid, QUICK, &ease::INOUT);
        assert!((tw.sample(mid, &ease::INOUT) - before).abs() < 1e-6, "no jump at the reversal");
        assert!(tw.dur < QUICK, "a reversal is shorter");
        for k in 1..=20 {
            let a = tw.sample(mid + ms(k * 4 - 4), &ease::INOUT);
            let b = tw.sample(mid + ms(k * 4), &ease::INOUT);
            assert!(b <= a + 1e-6, "monotonic back: {a} → {b}");
        }
        // A slide: the painted place (target + offset) never jumps, and the
        // offset never overshoots past zero.
        let mut s = Slide { target: 0., x0: 0., v0: 0., at: t0, frame: 0 };
        s.retarget(-24., t0);
        let place = |s: &Slide, t| s.target + s.state(t).0;
        assert!((place(&s, t0) - 0.).abs() < 1e-4, "starts where it was");
        let t1 = t0 + ms(60);
        let p1 = place(&s, t1);
        s.retarget(24., t1);
        assert!((place(&s, t1) - p1).abs() < 1e-4, "retarget keeps the place");
        let mut last = s.state(t1).0;
        for k in 1..100 {
            let x = s.state(t1 + ms(k * 5)).0;
            assert!(x.signum() == last.signum() || x.abs() < 0.5, "no overshoot");
            last = x;
        }
        assert!(s.settled(t1 + ms(600)));
        // A far move starts 48 px away on its true side.
        let mut f = Slide { target: 0., x0: 0., v0: 0., at: t0, frame: 0 };
        f.retarget(-300., t0);
        assert_eq!(f.state(t0).0, SLIDE_CAP);
    }

    /// History never animates; a change seen while watched does, from its
    /// previous value; one not drawn last frame comes back at rest.
    #[test]
    fn changes_animate_only_while_watched() {
        let t0 = Instant::now();
        let m = Motion::new(t0);
        m.begin_frame(Level::Full, false, t0);
        assert_eq!(m.change(Tag::Outcome, 1, 10, None), None, "first seen: at rest");
        m.begin_frame(Level::Full, false, t0 + ms(16));
        assert_eq!(m.change(Tag::Outcome, 1, 11, None), Some((t0 + ms(16), 10)));
        m.begin_frame(Level::Full, false, t0 + ms(32));
        assert_eq!(m.change(Tag::Outcome, 1, 11, None), Some((t0 + ms(16), 10)), "kept until it changes");
        // Not drawn for a frame: forgotten; back, at rest.
        m.begin_frame(Level::Full, false, t0 + ms(48));
        m.begin_frame(Level::Full, false, t0 + ms(64));
        assert_eq!(m.change(Tag::Outcome, 1, 12, None), None);
        // Born: from nothing.
        m.born(5, t0 + ms(64));
        assert_eq!(m.change(Tag::Outcome, 5, 1, m.birth(5)), Some((t0 + ms(64), u64::MAX)));
    }

    /// Finish gate fix 1: Handoff in's light is on at once and decays into
    /// the bar, each 30 fps step smaller than the last: it lands as the
    /// bar, never switched off.
    #[test]
    fn handoff_in_decays_into_the_bar() {
        let t0 = Instant::now();
        let m = Motion::new(t0);
        // At 30 fps, as a capture sees it.
        let at = |t: u64| {
            m.begin_frame(Level::Full, false, t0 + ms(t));
            m.handoff_light(Some(t0))
        };
        assert_eq!(at(0), 1., "on at once");
        assert_eq!(at(99), 1., "lit while it starts down");
        let k: Vec<f32> = (0..=20).map(|f| at(f * 100 / 3)).collect();
        assert!(k.windows(2).all(|w| w[1] <= w[0]), "it only decays: {k:?}");
        let steps: Vec<f32> = k.windows(2).map(|w| w[0] - w[1]).filter(|d| *d > 0.).collect();
        // (The first step is a part one: the decay starts between frames.)
        assert!(steps[1..].windows(2).all(|w| w[1] <= w[0] + 1e-6), "each step smaller than the last (no jump at the end): {steps:?}");
        assert!(*steps.last().unwrap() < 0.02, "its last step is a whisker: {steps:?}");
        assert_eq!(at(600), 0., "the bar at sig");
        assert_eq!(m.handoff_light(None), 0.);
    }

    /// Every running glyph samples the same beat in a frame, though the
    /// wall clock crosses a beat while the frame's rows are built (they
    /// read the frame's clock, not the time each is built at), and a row
    /// born mid-cycle joins the phase (it does not start at 0).
    #[test]
    fn fresh_glyph_rows_scan_in_lockstep() {
        // The epoch put so that the next beat is 2 ms after the frame's
        // instant (one instant for both: a slow runner can't cross it).
        let t0 = Instant::now();
        let m = Motion::new(t0 - (BEAT * 3 - ms(2)));
        m.begin_frame(Level::Full, false, t0);
        let old = m.beat(1);
        std::thread::sleep(ms(6));
        m.born(9, Instant::now());
        let fresh = m.beat(1);
        assert_eq!(old, Some(2));
        assert_eq!(fresh, old, "a row built after the wall clock crossed the beat scans with the rest");
        assert_eq!(crate::glyph::scan_row(old.unwrap()), crate::glyph::scan_row(fresh.unwrap()));
        // The next frame: the next beat, for both.
        m.begin_frame(Level::Full, false, t0 + ms(6));
        assert_eq!((m.beat(1), m.beat(1)), (Some(3), Some(3)));
    }

    /// The `roles_check` step (motion.md §10): every afterglow mix of
    /// §6.6 (`mix(role, ink, k)`, k 1 → 0.66 → 0.33 → 0) holds its text
    /// floor on the chrome, in every theme and accent; and the drain (70 %
    /// toward the grey of a role's own luminance, mixed in linear light)
    /// keeps every contrast, by construction (asserted).
    #[test]
    fn afterglow_mixes_and_the_drain_keep_their_floors() {
        use crate::theme::{contrast, luminance, mix, resolve};
        let lin = |v: u32| {
            let v = f64::from(v) / 255.;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        let enc = |l: f64| {
            let v = if l <= 0.003_130_8 { l * 12.92 } else { 1.055 * l.powf(1. / 2.4) - 0.055 };
            (v * 255.).round().clamp(0., 255.) as u32
        };
        let drain = |c: u32| {
            let y = luminance(c);
            let ch = |s: u32| enc(lin((c >> s) & 255) * 0.3 + y * 0.7);
            (ch(16) << 16) | (ch(8) << 8) | ch(0)
        };
        let (mut low, mut worst) = (f64::MAX, 0f64);
        for (name, raw) in crate::palettes::THEMES {
            for accent in [None, Some(0xe8132f), Some(0xf4b400), Some(0x2d7bff), Some(0x35f2ff)] {
                let r = resolve(&raw, accent).0;
                for (role, what) in [(r.cost_ch, "cost_ch"), (r.dim_ch, "dim_ch"), (r.err_ch, "err_ch"), (r.ok_ch, "ok_ch")] {
                    for k in [1., 0.66, 0.33, 0.] {
                        let c = contrast(mix(role, r.ink, k), r.chrome);
                        low = low.min(c);
                        assert!(c >= 4.5, "{name} {accent:?}: {what} afterglow at {k}: {c:.2}");
                    }
                }
                for role in [
                    r.run,
                    r.warn,
                    r.err,
                    r.ok,
                    r.you,
                    r.ready,
                    r.run_ch,
                    r.warn_ch,
                    r.err_ch,
                    r.ok_ch,
                    r.ready_ch,
                    r.you_mark,
                    r.warn_mark,
                    r.run_mark,
                ] {
                    for ground in [r.canvas, r.chrome, r.tool, r.user] {
                        worst = worst.max((contrast(drain(role), ground) - contrast(role, ground)).abs());
                    }
                }
            }
        }
        assert!(low >= 4.5, "{low}");
        assert!(worst < 0.06, "the drain moved a contrast by {worst:.3}");
    }

    /// The `roles_check` step for the soul (amendments 14, 15): the scan's
    /// beam and its afterglow are a running glyph's light, so they hold
    /// the mark floor (3:1) on every ground a running glyph is drawn on;
    /// a new bar, drawn hot, holds it on the run surface, for every
    /// tool's hue and thinking's; in every theme and accent. The bloom is
    /// the lit bar's glow beside it (decoration: the bar is the state).
    #[test]
    fn the_beam_and_the_head_keep_the_mark_floor() {
        use crate::glyph::{BEAM, GLOW};
        use crate::theme::{contrast, mix, resolve};
        let mut low = f64::MAX;
        for (name, raw) in crate::palettes::THEMES {
            for accent in [None, Some(0xe8132f), Some(0xf4b400), Some(0x2d7bff), Some(0x35f2ff)] {
                let r = resolve(&raw, accent).0;
                let marks = [
                    (r.run_mark, vec![r.canvas, r.tool, r.tool_out, r.user], "run_mark"),
                    (r.run_mark_ch, vec![r.chrome], "run_mark_ch"),
                    (r.run_mark_k, vec![r.card, r.card_hi], "run_mark_k"),
                ];
                for (mark, grounds, what) in marks {
                    for k in [BEAM, GLOW] {
                        let c = mix(mark, r.ink, f64::from(k));
                        for g in &grounds {
                            let x = contrast(c, *g);
                            low = low.min(x);
                            assert!(x >= 3., "{name} {accent:?}: {what}'s beam at {k}: {x:.2}");
                        }
                    }
                }
                for hue in [r.t_bash, r.t_read, r.t_write, r.t_edit, r.t_grep, r.t_find, r.t_ls, r.t_sub, r.t_ext, r.t_gen, r.think] {
                    let lit = mix(hue, r.ink, 0.30);
                    for bar in [hue, lit] {
                        let x = contrast(mix(bar, r.ink, f64::from(crate::turns::HEAD)), r.tool);
                        low = low.min(x);
                        assert!(x >= 3., "{name} {accent:?}: a bar's head: {x:.2}");
                    }
                }
            }
        }
        assert!(low >= 3., "{low}");
    }

    #[test]
    fn the_setting_resolves_locks_first() {
        assert_eq!(
            Setting::resolve(Some("full"), Some("1"), Some("reduced")).0,
            Setting { level: Some(Level::Off), lock: Some(Lock::NoBootFx) }
        );
        assert_eq!(
            Setting::resolve(Some("reduced"), Some("0"), Some("off")).0,
            Setting { level: Some(Level::Reduced), lock: Some(Lock::Flag) }
        );
        assert_eq!(Setting::resolve(None, None, Some("off")).0, Setting { level: Some(Level::Off), lock: None });
        assert_eq!(Setting::resolve(None, Some(""), None).0, Setting::default());
        assert_eq!(Setting::resolve(None, Some("false"), None).0, Setting::default(), "the TUI's switch: 1 only");
        let (s, warn) = Setting::resolve(Some("fast"), None, Some("reduced"));
        assert_eq!(s.level, Some(Level::Reduced));
        assert!(warn.is_some());
        assert_eq!(exit(QUICK), ms(90));
        assert_eq!(stagger(1), STAGGER);
        assert_eq!(stagger(5), STAGGER_CAP);
        assert_eq!(stagger(9), STAGGER_CAP);
        assert_eq!(BEAT * 8, Duration::from_secs(1));
    }

    /// Law 3 and §3.3: a row born now is 0 px tall on its first frame (its
    /// text measured, not shown) at no less than [`super::LIFT`], reveals
    /// to its measured height without overshoot, then is unwrapped.
    #[test]
    fn first_paint_text_at_least_lift() {
        let t0 = Instant::now();
        let m = Motion::new(t0);
        m.begin_frame(Level::Full, false, t0);
        let g = m.grow(3, 0, Some(t0), false, 20.).expect("an arrival grows");
        assert_eq!(g.shown, Some(0.));
        assert!(g.strength >= super::LIFT && g.strength < 1.);
        assert!(g.travel > 7.9);
        g.sink.set(120.);
        let mut last = 0.;
        let mut unwrapped = None;
        for k in 1..80u64 {
            m.begin_frame(Level::Full, false, t0 + Duration::from_micros(6944 * k));
            match m.grow(3, 0, Some(t0), false, 20.) {
                Some(g) => {
                    let h = g.shown.expect("clipped while revealing");
                    assert!(h >= last && h <= 120., "frame {k}: {h} after {last}");
                    assert!(g.strength >= super::LIFT);
                    last = h;
                }
                None => {
                    unwrapped = Some(k);
                    break;
                }
            }
        }
        let k = unwrapped.expect("unwrapped once settled");
        assert!(k > 20 && k < 60, "settled on snap: frame {k}");
        assert_eq!(m.growing(), 0);
        // Words: the stream curve starts at the floor and never goes under it.
        let e = super::stream_curve();
        assert!((e.sample(0.) - super::LIFT).abs() < 1e-4);
        assert!((e.sample(0.35) - 0.88).abs() < 1e-4);
        assert!((0..=100).all(|k| e.sample(k as f32 / 100.) >= super::LIFT - 1e-4));
        assert_eq!(super::text_motion().stream_fade(), super::STREAM_FADE);
    }

    /// Reduced, Off and low power: nothing grows (rows as today); history
    /// (no birth) never does.
    #[test]
    fn grow_is_full_and_live_only() {
        let t0 = Instant::now();
        for (level, low) in [(Level::Reduced, false), (Level::Off, false), (Level::Full, true)] {
            let m = Motion::new(t0);
            m.begin_frame(level, low, t0);
            assert!(m.grow(1, 0, Some(t0), true, 20.).is_none(), "{level:?} low power {low}");
        }
        let m = Motion::new(t0);
        m.begin_frame(Level::Full, false, t0);
        assert!(m.grow(1, 0, None, false, 20.).is_none(), "history");
        m.begin_frame(Level::Full, false, t0 + ms(500));
        assert!(m.grow(2, 0, Some(t0), false, 20.).is_none(), "born long ago, first drawn now: as is");
    }

    /// A revealing row is clipped at its top and bottom only: its gutter
    /// (the hanging label, the write head) is not (Shady H1).
    #[test]
    fn a_reveal_clips_y_only() {
        let b = Bounds::new(point(px(300.), px(100.)), size(px(600.), px(40.)));
        let m = super::ClipY::mask(b).bounds;
        assert_eq!((m.top(), m.bottom()), (b.top(), b.bottom()));
        assert!(m.left() < px(0.) && m.right() > px(10_000.), "{m:?}");
    }

    /// An arrival laid out off screen (the list's overdraw: drawn, never
    /// painted) doesn't reveal: its row is measured again, at its height
    /// (Shady M1).
    #[test]
    fn an_unpainted_arrival_comes_back_at_rest() {
        let t0 = Instant::now();
        let m = Motion::new(t0);
        m.begin_frame(Level::Full, false, t0);
        m.born(5, t0);
        let g = m.grow(5, 7, Some(t0), false, 20.).expect("an arrival");
        assert_eq!(g.shown, Some(0.));
        assert_eq!(m.unpainted(), vec![7], "drawn, not painted: measured again");
        m.begin_frame(Level::Full, false, t0 + ms(7));
        assert!(m.grow(5, 7, Some(t0), false, 20.).is_none(), "at rest");
        // One painted (its measure came) is not touched.
        m.born(6, t0 + ms(7));
        let g = m.grow(6, 8, Some(t0 + ms(7)), false, 20.).expect("an arrival");
        g.sink.set(40.);
        assert!(m.unpainted().is_empty());
        assert_eq!(m.growing(), 1);
    }

    /// §3.4: a streaming row's new line glides in on `flick`, never more
    /// than a line short; a shrink is at once.
    #[test]
    fn stream_line_growth_glides_within_a_line() {
        let t0 = Instant::now();
        let m = Motion::new(t0);
        m.begin_frame(Level::Full, false, t0);
        let g = m.grow(9, 0, None, true, 24.).expect("streaming rows are tracked");
        assert_eq!(g.shown, None, "unmeasured: its own height");
        g.sink.set(100.);
        m.begin_frame(Level::Full, false, t0 + ms(7));
        assert_eq!(m.grow(9, 0, None, true, 24.).and_then(|g| g.shown), Some(100.));
        g.sink.set(172.);
        let mut prev = 100.;
        for k in 2..60u64 {
            m.begin_frame(Level::Full, false, t0 + Duration::from_micros(6944 * k));
            let h = m.grow(9, 0, None, true, 24.).and_then(|g| g.shown).expect("still streaming");
            assert!(h >= 172. - 24. - 1e-3, "trails by at most a line: {h}");
            assert!(h >= prev);
            prev = h;
        }
        assert_eq!(prev, 172.);
        // A chunk of five lines at once (Shady L1): a line short at most,
        // from the frame its measure comes.
        g.sink.set(172. + 5. * 24.);
        for k in 60..120u64 {
            m.begin_frame(Level::Full, false, t0 + Duration::from_micros(6944 * k));
            let h = m.grow(9, 0, None, true, 24.).and_then(|g| g.shown).expect("still streaming");
            assert!(h >= 292. - 24. - 1e-3, "trails by at most a line: {h}");
        }
        g.sink.set(150.);
        m.begin_frame(Level::Full, false, t0 + ms(900));
        assert_eq!(m.grow(9, 0, None, true, 24.).and_then(|g| g.shown), Some(150.));
        // Done streaming and at rest: unwrapped.
        m.begin_frame(Level::Full, false, t0 + ms(907));
        assert!(m.grow(9, 0, None, false, 24.).is_none());
    }
}
