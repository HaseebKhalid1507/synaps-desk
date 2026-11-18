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

/// How early an OS timer may fire and still be the frame it was armed for
/// ([`Motion::tick`]): a coarse timer's slack (Windows' 15.6 ms), with room.
pub(crate) const EARLY: Duration = Duration::from_millis(20);

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
/// The shade sweep's step (polish.md P2, the one new token): its comet's
/// band, 12 px, moved one band per [`DECAY_TICK`].
pub(crate) const SWEEP_STEP: f32 = 12.;
/// The comet's four opaque bands, head first: the TUI's `█ ▓ ▒ ░` as
/// `mix(ground, mark, level)`. Never a gradient.
pub(crate) const SWEEP_LEVELS: [f32; 4] = [1., 0.66, 0.4, 0.2];
/// The write-on waits this long, then draws over [`BASE`] (polish.md P4).
pub(crate) const WRITE_DELAY: Duration = Duration::from_millis(40);
/// The star hop (polish.md P6): an impulse `v` px/s on a critically damped
/// spring of `glide`'s ω: up 4 px at 48 ms, back by ~300 ms, never past rest.
pub(crate) const HOP_V: f32 = 227.;
/// A roll is over within this much of its line (well under half a pixel).
const ROLL_EPSILON: f32 = 0.02;
pub(crate) const BREATHS: u32 = 3;
/// H2's state-change turn (indicators/proposal.md §6): every change of a
/// live indicator's state turns its grid a quarter onto the new state
/// ([`crate::lattice::turning`]); `false` is H1, the new state at once
/// (the fallback, kept switchable: [`Motion::turn_on_change`]).
pub(crate) const TURN_ON_CHANGE: bool = true;
/// [`TURN_ON_CHANGE`], or `SYNAPS_DESK_TURN=0` (H1) for a look at the
/// fallback (evidence).
fn turn_on_change() -> bool {
    match std::env::var("SYNAPS_DESK_TURN").ok().as_deref() {
        Some("0") => false,
        Some("1") => true,
        _ => TURN_ON_CHANGE,
    }
}

/// `flick` as a duration (its spring's response): a one-shot's length
/// where the proposal names the spring.
pub(crate) const FLICK: Duration = Duration::from_millis(160);

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
    /// A mark's strokes writing on (polish.md P4): near linear, easing out
    /// at the end: its time is spent on the path, so the stroke is seen.
    pub(crate) const WRITE: Easing = Easing::CubicBezier { x1: 0.3, y1: 0.3, x2: 0.6, y2: 1. };
    /// Light handed over evenly (a write-on's old glyph going out).
    pub(crate) const LINEAR: Easing = Easing::CubicBezier { x1: 0., y1: 0., x2: 1., y2: 1. };
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
/// `sheet`: the largest surfaces (panels, the dock, the authorize card's
/// collapse: polish.md P13), 426 ms to 0.5 px.
pub(crate) const SHEET_RESPONSE: f32 = 0.36;
/// An arriving block's or word's first strength: never lower (law 3).
pub(crate) const LIFT: f32 = 0.7;
/// A block or row arriving in a list travels this far, from below.
pub(crate) const ARRIVE: f32 = 8.;
/// A view (session, tab, worker) arriving travels this far, in its direction.
pub(crate) const SHIFT: f32 = 12.;
/// Chrome key `id` in the key space `space` (its top 16 bits): a row's
/// hashed key never reaches into another space.
pub(crate) const fn key(space: u64, id: u64) -> u64 {
    space | (id & 0xFFFF_FFFF_FFFF)
}

/// A hover fill comes on over this (fluid.md §3.9)…
pub(crate) const HOVER_IN: Duration = Duration::from_millis(60);
/// …and goes off over this: sweeping down the rail leaves a trail.
pub(crate) const HOVER_OUT: Duration = Duration::from_millis(160);
/// A press holds a control's content this far down (no div scale in GPUI:
/// a 1 px travel, fluid.md §3.9).
pub(crate) const PRESS: f32 = 1.;
/// An overlay rises this far from its anchor side as it enters (`rise`,
/// fluid.md §2.3: 6, was motion.md's 4).
pub(crate) const RISE_FLUID: f32 = 6.;
/// An overlay leaving sinks this far back toward its anchor.
pub(crate) const SINK: f32 = 2.;
/// One overlay replacing another this soon doesn't re-enter (it follows).
pub(crate) const REPLACE: Duration = Duration::from_millis(300);
/// A switch this soon after the last is instant (fluid.md §3.6).
pub(crate) const RAPID: Duration = Duration::from_millis(150);
/// A streamed word reaches full strength over this.
pub(crate) const STREAM_FADE: Duration = Duration::from_millis(180);
/// Each further word of one update starts this much later.
pub(crate) const STREAM_STAGGER: Duration = Duration::from_millis(12);
/// Positions settle within this (px).
const FLUID_EPSILON: f32 = 0.5;
/// [`Motion::spring_px`]: a change due within a 144 Hz frame is the next
/// frame's; one due later, a timer's.
const PX_FRAME: Duration = Duration::from_micros(6944);
/// A press's release has settled (its fill and travel, as progress).
const PRESS_EPSILON: f32 = 0.002;

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

/// Tests: a size-0 mark that logs `what` when it paints, in paint order
/// ([`Motion::paint_log`]); outside tests, nothing.
pub(crate) fn probe(fx: &Rc<Motion>, what: &'static str) -> Option<AnyElement> {
    #[cfg(test)]
    {
        let fx = fx.clone();
        Some(canvas(|_, _, _| {}, move |_, _, _, _| fx.painted(what)).absolute().size_0().into_any_element())
    }
    #[cfg(not(test))]
    {
        let _ = (fx, what);
        None
    }
}

/// The one sanctioned opacity (fluid.md §3.6, law 3): something arriving
/// lifts from [`LIFT`] to full, never lower. Opaque grounds only.
pub(crate) fn lift<E: Styled>(el: E, strength: f32) -> E {
    lift_from(el, strength, LIFT)
}

/// [`lift`] from `floor`: [`LIFT`], or below it only for content on its own
/// opaque card (a card row's, cards4/proposal.md §8 #1, #13: the card's
/// slice is drawn at full strength round it).
#[allow(clippy::disallowed_methods)]
pub(crate) fn lift_from<E: Styled>(el: E, strength: f32, floor: f32) -> E {
    if strength >= 1. { el } else { el.opacity(strength.max(floor.clamp(0., LIFT))) }
}

/// The overlay's strength (fluid.md §3.10): transient chrome appearing
/// under the pointer on the overlay box (opaque `card`) may start at 0.
/// The only other sanctioned opacity; never a state.
#[allow(clippy::disallowed_methods)]
pub(crate) fn appear<E: Styled>(el: E, strength: f32) -> E {
    if strength >= 1. { el } else { el.opacity(strength.max(0.)) }
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
    /// The lowest the content's strength is drawn at: [`LIFT`], or 0 for a
    /// card row's content on its opaque card (cards4/proposal.md §8 #1).
    pub(crate) floor: f32,
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

/// A body's reveal ([`Motion::reveal`]): open or not, since when (none:
/// at rest), the fraction shown when it turned, and its measured height.
#[derive(Clone, Debug)]
struct Reveal {
    open: bool,
    at: Option<Instant>,
    from: f32,
    frame: u64,
    sink: Rc<Cell<f32>>,
    /// What the body holds (its call's state): open and at rest, a change
    /// of it glides the body's height from the one it had.
    generation: u64,
    resize: Option<(f32, Instant)>,
}

impl Reveal {
    fn target(&self) -> f32 {
        if self.open { 1. } else { 0. }
    }

    /// The fraction shown at `now`, and whether it has settled.
    fn at(&self, now: Instant) -> (f32, bool) {
        let Some(at) = self.at else { return (self.target(), true) };
        let x = Spring::from_rest(1., GLIDE_RESPONSE, now.saturating_duration_since(at).as_secs_f32());
        let left = (self.from - self.target()) * x;
        let h = self.sink.get();
        let done = if h.is_nan() { x < 0.002 } else { (left * h).abs() < FLUID_EPSILON };
        (self.target() + left, done)
    }
}

/// Lines streaming into a body ([`Motion::streamed`]): how many it had, and
/// when each that came while watched arrived.
#[derive(Clone, Debug)]
struct Streamed {
    n: usize,
    frame: u64,
    born: Vec<(usize, Instant)>,
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
        // Whole pixels, every frame: a fractional height left in the
        // list's sums by a row that scrolls out of view mid-growth (never
        // measured again) puts its end an f32 ulp off (Shady #23 r2 H1:
        // which frames land mid-growth differs by runner).
        .when_some(g.shown, |d, h| d.h(px(h.round())))
        .child(lift_from(content, g.strength, g.floor))
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
        .child(ClipY(clip.into_any_element(), None))
        .children(label.map(|(l, top)| lift_from(l.top(px(top + g.travel)), g.strength, g.floor)))
        .children(head)
        .into_any_element()
}

/// `el` clipped to its own top and bottom only (x is left to the parents):
/// a box whose height is on a spring (the composer's growth, the dock).
pub(crate) fn clip_y(el: AnyElement) -> AnyElement {
    ClipY(el, None).into_any_element()
}

/// `el` laid out at its own height, drawn only down to `h` px under its
/// top (a section's body as it unfolds: its place in the list is its own).
pub(crate) fn clip_h(el: AnyElement, h: f32) -> AnyElement {
    ClipY(el, Some(h)).into_any_element()
}

/// Its child, clipped to its own top and bottom and nothing else: a
/// revealing row's gutter (the hanging labels, the write head, left of the
/// row's box) stays drawn. GPUI's `overflow_*` clip x as well, whichever
/// axis they hide. The list's own mask still bounds x.
struct ClipY(AnyElement, Option<f32>);

impl ClipY {
    fn mask(b: Bounds<Pixels>) -> ContentMask<Pixels> {
        let wide = px(1.0e6);
        ContentMask { bounds: Bounds::from_corners(point(b.left() - wide, b.top()), point(b.right() + wide, b.bottom())) }
    }

    /// [`ClipY::mask`], down to its limit if it has one.
    fn mask_of(&self, b: Bounds<Pixels>) -> ContentMask<Pixels> {
        let mut m = Self::mask(b);
        if let Some(h) = self.1 {
            m.bounds.size.height = m.bounds.size.height.min(px(h.max(0.)));
        }
        m
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
        let mask = self.mask_of(bounds);
        window.with_content_mask(Some(mask), |window| {
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
        let mask = self.mask_of(bounds);
        window.with_content_mask(Some(mask), |window| self.0.paint(window, cx));
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

/// The link's signatures (§5.1 Scan, §5.3 Glitch), never offline. Only a
/// real loss plays them: a daemon reload is a planned handover (its
/// sessions come back), so a link that goes down for one is given
/// [`RELOAD_GRACE`], restarted by each new reload's goodbye (a storm); any
/// other drop [`LINK_GRACE`]. Back within it, nothing happened. The first
/// hello after launch scans, however long the link took to come up (the
/// window starts at `Connecting`). Past it, the link is lost: the tear (at most once per
/// [`FLASH_BUDGET`]), then the palette drains, held while down. Back after
/// a loss, it restores and scans (the Scan also at most once per budget).
/// The drain is a [`Tween`]: a drop mid-restore or a return mid-drain
/// turns from where it is, never from an end.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Link {
    /// Up last frame (none: not seen yet).
    pub up: Option<bool>,
    /// Since when it is down, and for which reload (the model's count of
    /// reload goodbyes; none: not a reload). A new goodbye restarts it.
    pub down: Option<(Instant, Option<u64>)>,
    /// It has been up since launch (so the next up isn't the first hello).
    pub ever_up: bool,
    /// The loss was seen (past the grace).
    pub lost: bool,
    pub scan: Option<Instant>,
    /// The tear that is playing (the drop's time, for its seed).
    pub glitch: Option<Instant>,
    /// The last tear and the last Scan (the budget).
    pub tore: Option<Instant>,
    pub scanned: Option<Instant>,
    /// How drained the palette is (0..1).
    pub drain: Option<Tween>,
}

/// How long a non-reload drop may last before it is a loss.
pub(crate) const LINK_GRACE: Duration = Duration::from_millis(1000);
/// How long a reload may take before it is a loss, from its last goodbye:
/// longer than the client's longest honest wait in one (it retries every
/// `synaps_client::RELOAD_RETRY` for `RELOAD_WINDOW`, but first waits out
/// the daemon's `retry_after`, capped at 10 s), plus a boot.
pub(crate) const RELOAD_GRACE: Duration = Duration::from_secs(15);
/// At most one tear, and one Scan, per this.
pub(crate) const FLASH_BUDGET: Duration = Duration::from_secs(5);

/// A worker tab opened from a row by the pointer (§6.2): the tracer, a
/// 2 × 24 px line in `you_bar` from where it was opened to the new tab's
/// top edge over `base` `out`; then the tab's bar draws left to right over
/// `quick`. Full only.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Tracer {
    pub tab: u64,
    pub from: gpui_kit::Point<gpui_kit::Pixels>,
    pub at: Instant,
    /// The tab's top-left corner in the window, once the strip laid it out.
    pub to: Option<gpui_kit::Point<gpui_kit::Pixels>>,
}

/// The tracer's line: its length.
pub(crate) const TRACER_LEN: f32 = 24.;

/// The tear's two frames (§5.3): 60 ms, then 40.
pub(crate) const TEAR: [Duration; 2] = [Duration::from_millis(60), Duration::from_millis(40)];

impl Link {
    /// The connection is `up` this frame (`reload`: it went down for the
    /// daemon reload with this count), at `now`, under `fx`'s level.
    ///
    /// Off plays nothing (no tear, drain or Scan) but still keeps the
    /// state: the grace and the loss (what the words and the rail say),
    /// and the first hello spent. A switch to Full mid-loss drains, no
    /// tear.
    pub(crate) fn see(&mut self, up: bool, reload: Option<u64>, fx: &Motion) {
        let (now, level) = (fx.now(), fx.level());
        let off = level == Level::Off;
        if off {
            (self.scan, self.glitch, self.drain) = (None, None, None);
        }
        let budget = |at: Option<Instant>| at.is_none_or(|t| now.saturating_duration_since(t) >= FLASH_BUDGET);
        match (self.up.replace(up), up) {
            // The first hello scans, from `Connecting` or before the first
            // frame (outside the budget: a return after a loss soon after
            // launch scans too).
            (_, true) if !self.ever_up => {
                self.ever_up = true;
                self.down = None;
                if level == Level::Full {
                    self.scan = Some(now);
                }
            }
            (Some(false), true) => {
                self.down = None;
                if std::mem::take(&mut self.lost) {
                    self.glitch = None;
                    if !off {
                        self.retarget(0., fx);
                    }
                    if level == Level::Full && budget(self.scanned) {
                        (self.scan, self.scanned) = (Some(now), Some(now));
                    }
                }
            }
            (Some(true), false) => {
                self.down = Some((now, reload));
                self.scan = None;
            }
            // Down still, and a new reload's goodbye came (the client was
            // back for a moment between two frames): its grace starts now.
            (_, false) if reload.is_some() && self.down.is_some_and(|(_, r)| r != reload) => {
                self.down = Some((now, reload));
            }
            _ => {}
        }
        if self.lost && !off && self.drain.is_none() {
            // Lost under Off, now on: drained, held (no tear).
            self.retarget(1., fx);
        }
        let Some((since, reload)) = self.down.filter(|_| !self.lost && self.up == Some(false)) else { return };
        let grace = if reload.is_some() { RELOAD_GRACE } else { LINK_GRACE };
        if now.saturating_duration_since(since) < grace {
            fx.want_at(since + grace);
            return;
        }
        self.lost = true;
        if off {
            return;
        }
        let tear = level == Level::Full && budget(self.tore);
        if tear {
            (self.glitch, self.tore) = (Some(now), Some(now));
        }
        self.retarget(1., fx);
        if tear && let Some(d) = self.drain.as_mut() {
            // The drain waits for the tear.
            d.delay(TEAR[0] + TEAR[1]);
        }
    }

    /// When the loss was seen (the end of its grace), while lost.
    pub(crate) fn lost_at(&self) -> Option<Instant> {
        let (since, reload) = self.down.filter(|_| self.lost)?;
        Some(since + if reload.is_some() { RELOAD_GRACE } else { LINK_GRACE })
    }

    fn retarget(&mut self, to: f32, fx: &Motion) {
        let now = fx.now();
        let dur = if fx.level() == Level::Full { SLOW } else { QUICK };
        self.drain.get_or_insert(Tween::rest(0., now)).retarget(to, now, dur, &ease::INOUT);
    }

    /// The tear's frame now (0, 1), if one is on: Full only.
    pub(crate) fn tear(&mut self, fx: &Motion) -> Option<usize> {
        let g = self.glitch.filter(|_| fx.level() == Level::Full)?;
        let gone = fx.now().saturating_duration_since(g);
        if gone < TEAR[0] {
            fx.want_at(g + TEAR[0]);
            Some(0)
        } else if gone < TEAR[0] + TEAR[1] {
            fx.want_at(g + TEAR[0] + TEAR[1]);
            Some(1)
        } else {
            self.glitch = None;
            None
        }
    }

    /// How drained the palette is (0..1): from the end of the tear over
    /// `slow` `inout` (Reduced: no tear, `quick`), held while down; back
    /// over the same once up again, from wherever it was.
    pub(crate) fn drain(&mut self, fx: &Motion) -> f32 {
        if fx.level() == Level::Off {
            self.drain = None;
            return 0.;
        }
        let now = fx.now();
        let Some(d) = self.drain else { return 0. };
        let k = d.sample(now, &ease::INOUT);
        if d.running(now) {
            match d.starts() {
                Some(at) if at > now => fx.want_at(at),
                _ => fx.want_frame(),
            }
        } else if k <= 0. {
            self.drain = None;
        }
        k
    }

    /// The Scan's line, 0..1 down the window (`sig` `out`), while it runs.
    pub(crate) fn scan(&mut self, fx: &Motion) -> Option<f32> {
        let t = fx.shot(self.scan, Duration::ZERO, SIG, &ease::OUT, Kind::Spatial);
        if t >= 1. {
            self.scan = None;
            return None;
        }
        Some(t)
    }
}

/// What an element remembers of a value between frames ([`Motion::change`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Tag {
    /// A call's trail glyph.
    Outcome,
    /// A prompt waits on the call (its gutter glyph breathes).
    Waiting,
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
    /// A tab needs you (its top bar draws).
    TabNeeds,
    /// A rail row needs you (its bar draws).
    RowNeeds,
    /// A rail row's favourite (its star hops, polish.md P6).
    Star,
}

impl Tag {
    /// Keyed by the column's own ids (a transcript's item ids, which every
    /// transcript numbers from 1): another column's are other things.
    fn column(self) -> bool {
        matches!(self, Tag::Outcome | Tag::Waiting | Tag::Block | Tag::BlockGlyph | Tag::Stream)
    }
}

/// What a glyph does on the beat alone, painted by the beat layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Beat {
    /// A 10 px live glyph's lattice (the rail, the tabs, a chip; was the
    /// braille's frame, polish.md P1): [`crate::glyph::spin`].
    Spin(crate::glyph::Glyph),
    /// `Needs` breathing since the prompt came ([`crate::glyph::breath`]).
    Breath(Instant),
    /// A 16 px lattice (a card's, the composer's cell), its pose on the
    /// beat ([`crate::lattice::pose`]).
    Lattice(crate::lattice::Pat),
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
        now < self.at || now.saturating_duration_since(self.at) < self.dur
    }

    /// Its travel starts `by` later (it holds its start value until then).
    pub(crate) fn delay(&mut self, by: Duration) {
        self.at += by;
    }

    /// When its travel starts, if it has one.
    pub(crate) fn starts(&self) -> Option<Instant> {
        (!self.dur.is_zero()).then_some(self.at)
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

/// A chrome position on a critically damped spring ([`Motion::spring`]):
/// its displacement from `target` at `at`, and its velocity then.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Sprung {
    target: f32,
    x0: f32,
    v0: f32,
    at: Instant,
    frame: u64,
    response: f32,
}

impl Sprung {
    fn state(&self, now: Instant) -> (f32, f32) {
        let mut s = Spring { x: self.x0, v: self.v0 };
        s.step(self.response, now.saturating_duration_since(self.at).as_secs_f32());
        (s.x, s.v)
    }

    fn settled(&self, now: Instant) -> bool {
        let (x, v) = self.state(now);
        Spring { x, v }.settled(self.response)
    }
}

/// One window's motion: the frame's clock, what the frame asked for, and
/// what elements remember between frames (kept only for what was drawn in
/// the last frame: nothing grows with history).
pub(crate) struct Motion {
    level: Cell<Level>,
    /// The palette's ink, as of the frame (afterglows).
    ink: Cell<Hsla>,
    ambient: Cell<bool>,
    /// The window's scale, as of the frame ([`Self::snap`]).
    scale: Cell<f32>,
    /// Ambient as the level and power have it, held still or not
    /// ([`Self::words_beat`]).
    lively: Cell<bool>,
    /// The link was lost at this instant (as of the frame, R2-L1): every
    /// clock shows its count as of then and asks for no second.
    stale: Cell<Option<Instant>>,
    /// Tests: what painted this frame, in order (the paint-order probe,
    /// Shady #17 R3-M2: the settle pass over the composer's border).
    #[cfg(test)]
    pub(crate) paint_log: RefCell<Vec<&'static str>>,
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
    /// Chrome on springs (the tab indicator, the rail's selection, a
    /// segment's bar, the readout's y): the window's, never rescoped.
    springs: RefCell<HashMap<u64, Sprung>>,
    /// Each [`Motion::cell_slide`]'s cell, and the frame that drew it.
    cells: RefCell<HashMap<u64, (u64, u64)>>,
    /// A view pane is drawing, and something in it asked for a frame.
    pane: Cell<bool>,
    pane_wants: Cell<bool>,
    /// Hover fills on their way in or out ([`Motion::hover`]).
    hovers: RefCell<HashMap<u64, (Tween, u64)>>,
    /// The control held down (or pressed from the keyboard): its key, and
    /// when it was let go ([`Motion::pressed`]).
    press: Cell<Option<(u64, Option<Instant>)>>,
    /// Counts and lines as last drawn, rolling ([`Motion::roll`]).
    rolls: RefCell<HashMap<u64, Rolling>>,
    /// The count each rolling count last showed (its direction).
    roll_n: RefCell<HashMap<u64, u64>>,
    /// Numbers as last drawn, for their afterglow ([`Motion::glow`]):
    /// the window's, so a switch's new values glow (fluid.md §3.6).
    glows: RefCell<HashMap<u64, Seen>>,
    /// Rows revealing or growing ([`Motion::grow`]).
    grows: RefCell<HashMap<u64, Growing>>,
    /// Bodies opening and closing under their rows ([`Motion::reveal`]).
    reveals: RefCell<HashMap<u64, Reveal>>,
    /// Lines streaming into a body ([`Motion::streamed`]).
    streams: RefCell<HashMap<u64, Streamed>>,
    /// Rows born that will not reveal: first drawn off screen, in the
    /// list's overdraw ([`Motion::unpainted`]).
    quiet: RefCell<std::collections::HashSet<u64>>,
    /// The one timer.
    timer: RefCell<Option<Task<()>>>,
    /// The instant it is armed for, and whom it wakes (cleared as it fires).
    armed: Rc<Cell<Option<(Instant, Wake)>>>,
    /// The instant the timer last fired for ([`Motion::tick`]).
    woke: Rc<Cell<Option<(Instant, Wake)>>>,
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
    skew: Rc<Cell<i128>>,
    /// Tests: the clock stands at this instant until set ([`Motion::pin_clock`]).
    #[cfg(test)]
    pin: Rc<Cell<Option<Instant>>>,
    /// H2's turn on a change ([`TURN_ON_CHANGE`]), or H1 (`false`):
    /// `SYNAPS_DESK_TURN=0` for the fallback, and tests of both.
    pub(crate) turn_on_change: Cell<bool>,
    /// Tests: the OS in low power ([`Motion::low_power`]).
    #[cfg(test)]
    pub(crate) force_low_power: Cell<bool>,
    /// Tests: the beat each layer paint with sites drew, in order.
    #[cfg(test)]
    pub(crate) painted: RefCell<Vec<u32>>,
}

/// A count or a line as last drawn ([`Motion::roll`]).
#[derive(Clone, Debug)]
struct Rolling {
    value: SharedString,
    prev: SharedString,
    since: Option<Instant>,
    up: bool,
    frame: u64,
}

/// The star hop's offset `t` s after the impulse (polish.md P6):
/// `−v·t·e^(−ωt)`, ω = 2π / `glide`.
pub(crate) fn hop_at(t: f32) -> f32 {
    let w = std::f32::consts::TAU / GLIDE_RESPONSE;
    -HOP_V * t * (-w * t).exp()
}

/// The comet with its head's right edge at `head` px along a `w` px edge:
/// its bands clipped to it, each at `level(i)`.
pub(crate) fn band_at(w: f32, head: f32, level: impl Fn(usize) -> f32) -> impl Iterator<Item = (f32, f32, f32)> {
    (0..SWEEP_LEVELS.len()).filter_map(move |i| {
        let right = (head - SWEEP_STEP * i as f32).min(w);
        let left = (head - SWEEP_STEP * (i + 1) as f32).max(0.);
        (right > left).then(|| (left, right - left, level(i)))
    })
}

/// The settle pass (polish.md P3): the composer sweep's one last pass, its
/// head at `p` (0..1) of the edge and the comet, in `mark` on `ground`;
/// the app's frame draws it (Handoff in's own frames).
pub(crate) fn pass(fx: &Rc<Motion>, p: f32, mark: Hsla, ground: Hsla) -> AnyElement {
    #[cfg(test)]
    let fx = fx.clone();
    #[cfg(not(test))]
    let _ = fx;
    let line = canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            #[cfg(test)]
            fx.painted("settle-pass");
            let w = f32::from(b.size.width);
            // Its head one band in at 0 (the corner lit both ways on the
            // turn's last frame, as Handoff's 24 px light is), past the end
            // with its tail at 1.
            for (x, bw, level) in band_at(w, SWEEP_STEP + p * (w + SWEEP_STEP * 3.), |i| SWEEP_LEVELS[i]) {
                window.paint_quad(fill(Bounds::new(b.origin + point(px(x), px(0.)), size(px(bw), px(1.))), mix(ground, mark, level)));
            }
        },
    )
    .size_full();
    // On the composer's top border, which GPUI paints after its children:
    // deferred, the pass paints over it (the comet is the beat layer's,
    // painted over the frame). Deferred, it is not clipped by its
    // ancestors (GPUI's `deferred` draws with no content mask): none clips
    // the composer's edge (Shady #17 R3-L1).
    deferred(div().absolute().left_0().right_0().bottom_0().h(px(1.)).debug_selector(|| "settle-pass".into()).child(line))
        .into_any_element()
}

/// A rolling text (polish.md P5): `make(text)` as it is at rest; while it
/// rolls, the new text coming in and the one it left going out, a line
/// apart, clipped to the line ([`Roll`]). A count (`grow` false) rolls only
/// its number's changed digit cells (`12` → `13`: the `3`; `9` → `10`: the
/// number): its words, and everything beside it, stay where they are at
/// rest, laid out by the same element. `grow`: a live line, which rolls
/// whole at its row's width.
pub(crate) fn rolled(fx: &Motion, key: u64, text: SharedString, up: bool, grow: bool, make: impl Fn(SharedString) -> Div) -> AnyElement {
    let Some((old, x, up)) = fx.roll(key, &text, up) else { return make(text).into_any_element() };
    let cells = if grow { None } else { changed_cells(&old, &text) };
    let tagged = |sel: String, t: SharedString| div().debug_selector(move || sel).child(make(t)).into_any_element();
    let cut = |at: usize| make(SharedString::from(text[..at].to_string())).into_any_element();
    // The words after the cells changed too (`1 tool` → `2 tools`): the old
    // ones stay until the digits cross mid-roll (re-gate #17: never `1 tools`).
    let words = cells.and_then(|(_, b)| {
        let ob = old_cells_end(&old, &text, b)?;
        (old[ob..] != text[b..]).then(|| {
            (tagged(format!("rolled-words-{key:x}"), old.clone()), make(SharedString::from(old[..ob].to_string())).into_any_element())
        })
    });
    Roll {
        left: cells.filter(|c| c.0 > 0).map(|_| make(text.clone()).into_any_element()),
        measure: cells.map(|(a, b)| ((a > 0).then(|| cut(a)), cut(b))),
        words,
        rolling: tagged(format!("rolling-{key:x}"), text.clone()),
        old: tagged(format!("rolled-{key:x}"), old),
        new: make(text).into_any_element(),
        dir: if up { 1. } else { -1. },
        x,
        grow,
    }
    .into_any_element()
}

/// The changed digit cells of a count going `old` → `new` (polish.md P5),
/// as a byte range of `new`: its number's changed digits when both numbers
/// are as long (`19 calls` → `20 calls`: `20`; `12` → `13`: `3`), its whole
/// number when not (`9` → `10`); empty when the number is the same. `None`
/// (the whole text rolls) when either has no number or what comes before
/// it changed.
pub(crate) fn changed_cells(old: &str, new: &str) -> Option<(usize, usize)> {
    let ((oa, ob), (na, nb)) = (number_in(old)?, number_in(new)?);
    if old[..oa] != new[..na] {
        return None;
    }
    let (o, n) = (&old.as_bytes()[oa..ob], &new.as_bytes()[na..nb]);
    if o.len() != n.len() {
        return Some((na, nb));
    }
    let Some(first) = o.iter().zip(n).position(|(a, b)| a != b) else { return Some((na, na)) };
    let last = o.iter().zip(n).rposition(|(a, b)| a != b).unwrap_or(first);
    Some((na + first, na + last + 1))
}

/// Where `old`'s changed cells end (a byte index of `old`), `new`'s
/// ending at `b` ([`changed_cells`]): the same index when both numbers are
/// as long, else the end of `old`'s number. Only after `changed_cells`
/// returned `Some`, which proved the text before the numbers the same (this
/// doesn't check it).
fn old_cells_end(old: &str, new: &str, b: usize) -> Option<usize> {
    let (oa, ob) = number_in(old)?;
    let (na, nb) = number_in(new)?;
    Some(if ob - oa == nb - na { b } else { ob })
}

/// The first number in `s`, as a byte range.
fn number_in(s: &str) -> Option<(usize, usize)> {
    let a = s.find(|c: char| c.is_ascii_digit())?;
    Some((a, s[a..].find(|c: char| !c.is_ascii_digit()).map_or(s.len(), |n| a + n)))
}

/// [`rolled`]'s element. `new` is in the flow, laid out as at rest (so
/// nothing beside it moves while it rolls); the rest are copies drawn over
/// its box: `rolling` (the new text, `x` of a line from its place) and `old`
/// (the one it left, a line on), clipped to its top and bottom; for a
/// count, to its changed cells, `new` itself and `left` drawing the rest
/// in place. `measure`: the texts before and through the cells (laid out
/// only, for their x). `words`: the old text in place and the old text
/// through its cells (for its x), drawn after the cells instead of `new`'s
/// until the digits cross mid-roll, when the words after them changed.
/// Offsets in px, from the box's own height.
struct Roll {
    new: AnyElement,
    left: Option<AnyElement>,
    words: Option<(AnyElement, AnyElement)>,
    rolling: AnyElement,
    old: AnyElement,
    measure: Option<(Option<AnyElement>, AnyElement)>,
    dir: f32,
    x: f32,
    grow: bool,
}

impl IntoElement for Roll {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Roll {
    type RequestLayoutState = ();
    /// The cells' x span (none: the whole line rolls), and where the old
    /// words start.
    type PrepaintState = Option<(Pixels, Pixels, Pixels)>;

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
        (self.new.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<(Pixels, Pixels, Pixels)> {
        let fit = size(AvailableSpace::MinContent, AvailableSpace::MinContent);
        let wide = size(AvailableSpace::Definite(bounds.size.width), AvailableSpace::MinContent);
        let cut = self.measure.as_mut().map(|(a, b)| {
            let x0 = a.as_mut().map_or(px(0.), |a| a.layout_as_root(fit, window, cx).width);
            let x1 = b.layout_as_root(fit, window, cx).width.max(x0);
            (bounds.left() + x0, bounds.left() + x1)
        });
        self.new.prepaint(window, cx);
        if let Some(left) = self.left.as_mut() {
            left.prepaint_as_root(bounds.origin, wide, window, cx);
        }
        let mut ox1 = cut.map_or(px(0.), |c| c.1);
        if let Some((words, through)) = self.words.as_mut() {
            ox1 = ox1.max(bounds.left() + through.layout_as_root(fit, window, cx).width);
            words.prepaint_as_root(bounds.origin, fit, window, cx);
        }
        let cut = cut.map(|(x0, x1)| (x0, x1, ox1));
        let h = bounds.size.height;
        self.rolling.prepaint_as_root(bounds.origin + point(px(0.), h * (self.dir * self.x)), wide, window, cx);
        let old_fit = if self.grow { wide } else { fit };
        self.old.prepaint_as_root(bounds.origin + point(px(0.), h * (self.dir * (self.x - 1.))), old_fit, window, cx);
        cut
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        cut: &mut Option<(Pixels, Pixels, Pixels)>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let far = px(1.0e6);
        let band = |l: Pixels, r: Pixels| ContentMask { bounds: Bounds::from_corners(point(l, bounds.top()), point(r, bounds.bottom())) };
        let (x0, x1, ox1) = cut.unwrap_or((bounds.left() - far, bounds.right() + far, bounds.right() + far));
        if cut.is_some() {
            // The words after the cells: the old ones until the digits
            // cross mid-roll, then the new.
            let crossed = self.x < 0.5;
            match self.words.as_mut().filter(|_| !crossed) {
                Some((words, _)) => window.with_content_mask(Some(band(ox1, bounds.right() + far)), |window| words.paint(window, cx)),
                None => window.with_content_mask(Some(band(x1, bounds.right() + far)), |window| self.new.paint(window, cx)),
            }
            if let Some(left) = self.left.as_mut() {
                window.with_content_mask(Some(band(bounds.left() - far, x0)), |window| left.paint(window, cx));
            }
        }
        window.with_content_mask(Some(band(x0, x1)), |window| {
            self.rolling.paint(window, cx);
            self.old.paint(window, cx);
        });
    }
}

/// [`rolled`] for a count: up when it grew.
pub(crate) fn roll_count(fx: &Motion, key: u64, n: usize, text: SharedString, make: impl Fn(SharedString) -> Div) -> AnyElement {
    let up = fx.roll_was(key).is_none_or(|was| n as u64 >= was);
    fx.roll_saw(key, n as u64);
    rolled(fx, key, text, up, false, make)
}

/// What rolls (polish.md P5), each in its own key space: a run's `N
/// calls`, `+N more`, the NEEDS YOU count, the rail's chips. Never a clock,
/// cost, tokens or a %; nor a worker's tool count or live line (gate 4: a
/// roll is 30 Hz of app frames, and a live fan-out changes them every
/// fraction of a second; they come back as beat-layer paint).
pub(crate) const ROLL_CALLS: u64 = 0x7AD2 << 48;
pub(crate) const ROLL_MORE: u64 = 0x7AD3 << 48;
pub(crate) const ROLL_NEEDS: u64 = 0x7AD4 << 48;
pub(crate) const ROLL_RAIL: u64 = 0x7AD5 << 48;
/// Tests: a count with a plural (`AppView::roll_probe`).
#[cfg(test)]
pub(crate) const ROLL_PROBE: u64 = 0x7AD6 << 48;

impl Default for Motion {
    fn default() -> Self {
        // Tests: each window's clock is its own, so is its epoch.
        if cfg!(test) { Motion::new(Instant::now()) } else { Motion::new(epoch()) }
    }
}

/// The process's motion epoch: every 30 Hz decay tick (the app's and a
/// scrollbar's) is on its grid, so they land in the same frames.
pub(crate) fn epoch() -> Instant {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

/// The first 30 Hz tick on [`epoch`]'s grid after `now`.
pub(crate) fn next_tick(now: Instant) -> Instant {
    let ticks = now.saturating_duration_since(epoch()).as_nanos() / DECAY_TICK.as_nanos() + 1;
    epoch() + Duration::from_nanos((DECAY_TICK.as_nanos() * ticks) as u64)
}

impl Motion {
    pub(crate) fn new(epoch: Instant) -> Self {
        Motion {
            level: Cell::new(Level::Full),
            ink: Cell::new(Hsla::default()),
            ambient: Cell::new(true),
            scale: Cell::new(1.),
            lively: Cell::new(true),
            stale: Cell::new(None),
            #[cfg(test)]
            paint_log: Default::default(),
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
            springs: RefCell::default(),
            cells: RefCell::default(),
            hovers: RefCell::default(),
            pane: Cell::new(false),
            pane_wants: Cell::new(false),
            press: Cell::new(None),
            glows: RefCell::default(),
            rolls: RefCell::default(),
            roll_n: RefCell::default(),
            grows: RefCell::default(),
            reveals: RefCell::default(),
            streams: RefCell::default(),
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
            skew: Rc::default(),
            #[cfg(test)]
            pin: Rc::default(),
            #[cfg(test)]
            force_low_power: Cell::new(false),
            turn_on_change: Cell::new(turn_on_change()),
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
        self.rolls.borrow_mut().clear();
        self.grows.borrow_mut().clear();
        self.reveals.borrow_mut().clear();
        self.streams.borrow_mut().clear();
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
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn grow(&self, key: u64, ix: usize, born: Option<Instant>, streaming: bool, line: f32) -> Option<Grow> {
        self.grow_on(key, ix, born, streaming, line, SNAP_RESPONSE)
    }

    /// [`Motion::grow`], an arrival's height revealing on `response`'s
    /// spring (the authorize card's on `glide`, cards4/proposal.md §8 #8).
    pub(crate) fn grow_on(&self, key: u64, ix: usize, born: Option<Instant>, streaming: bool, line: f32, response: f32) -> Option<Grow> {
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
            return Some(Grow { key, shown: g.arriving.then_some(0.), sink: g.sink.clone(), travel, strength, floor: LIFT });
        }
        if g.shown.is_nan() {
            g.shown = if g.arriving { 0. } else { natural };
        }
        if g.born.is_none_or(|b| b <= now) {
            if natural < g.shown {
                (g.shown, g.spring) = (natural, Spring::default());
            } else {
                let response = if g.arriving { response } else { FLICK_RESPONSE };
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
        Some(Grow { key, shown: Some(g.shown), sink: g.sink.clone(), travel, strength, floor: LIFT })
    }

    /// A body under its row as it opens or closes (cards4/proposal.md §8
    /// #6, #12): the fraction of its height shown (the header never moves:
    /// the clip is the body's), on `glide`, and the cell its height is
    /// measured into. `None`: at rest (open: all of it; closed: none). A
    /// body seen for the first time (history, scrolled in, a switch) is at
    /// rest; Reduced, Off and low power: at once. `delay`: a turn that
    /// starts now waits this long (a success's streamed body folds @ +180).
    /// `generation`: what it holds (its call's state); open and at rest, a
    /// change of it (a failure's output in place of the streamed lines)
    /// glides its height from the one it had, never a jump.
    /// The frame's number (counts up a frame at a time).
    pub(crate) fn frame(&self) -> u64 {
        self.frame.get()
    }

    /// A body opening or closing (the design gate of #23, S1): whether it
    /// opens, how much of it is shown at `now`, its measured height, and
    /// whether it has landed. None: no body measured.
    pub(crate) fn body(&self, key: u64, now: Instant) -> Option<(bool, f32, f32, bool)> {
        let reveals = self.reveals.borrow();
        let r = reveals.get(&key)?;
        let h = r.sink.get();
        if h.is_nan() {
            return None;
        }
        let (shown, done) = r.at(now);
        Some((r.open, shown, h, done || r.at.is_none()))
    }

    pub(crate) fn reveal(&self, key: u64, open: bool, delay: Duration, generation: u64) -> (Option<f32>, Rc<Cell<f32>>) {
        let (frame, now) = (self.frame.get(), self.now());
        let fluid = self.fluid();
        let mut reveals = self.reveals.borrow_mut();
        let r = reveals.entry(key).or_insert_with(|| Reveal {
            open,
            at: None,
            from: 0.,
            frame,
            sink: Rc::new(Cell::new(f32::NAN)),
            generation,
            resize: None,
        });
        if r.frame + 1 < frame || !fluid {
            (r.open, r.at, r.resize) = (open, None, None);
        } else if r.open != open {
            let (shown, _) = r.at(now);
            (r.open, r.from, r.at, r.resize) = (open, shown, Some(now + delay), None);
        } else if r.generation != generation && open && r.at.is_none() && !r.sink.get().is_nan() {
            r.resize = Some((r.sink.get(), now));
        }
        (r.frame, r.generation) = (frame, generation);
        if let Some((from, at)) = r.resize {
            // The height it had, gliding to the one it has (measured a
            // frame late: the first frame holds the old).
            let to = r.sink.get();
            let x = Spring::from_rest(1., GLIDE_RESPONSE, now.saturating_duration_since(at).as_secs_f32());
            let h = to + (from - to) * x;
            // Its first frame measures the new height (`to` is the old one
            // until then): never done there.
            if now > at && ((h - to).abs() < FLUID_EPSILON || to <= 0.) {
                r.resize = None;
            } else {
                self.want_frame();
                return (Some(if to > 0. { (h / to).max(0.) } else { 1. }), r.sink.clone());
            }
        }
        let (shown, done) = r.at(now);
        if done {
            r.at = None;
            return (None, r.sink.clone());
        }
        self.want_frame();
        (Some(shown.clamp(0., 1.)), r.sink.clone())
    }

    /// Lines streaming into a body (cards4/proposal.md §8 #4): of the `n`
    /// it holds now, those that came while watched, as `(line, clip,
    /// strength)`: each line's clip 0 → 1 of its height on `flick`, its
    /// text from 70 % on [`STREAM_FADE`] ([`stream_curve`]), in arrival
    /// order, lines that came in one frame [`STREAM_STAGGER`] apart. Empty
    /// at rest: history, a body seen for the first time, Reduced, Off.
    pub(crate) fn streamed(&self, key: u64, n: usize) -> Vec<(usize, f32, f32)> {
        let (frame, now) = (self.frame.get(), self.now());
        let fluid = self.fluid();
        let mut streams = self.streams.borrow_mut();
        let s = streams.entry(key).or_insert_with(|| Streamed { n, frame, born: Vec::new() });
        if s.frame + 1 < frame || !fluid || n < s.n {
            s.born.clear();
        } else if n > s.n {
            let first = s.born.last().map_or(now, |b| b.1.max(now));
            s.born.extend((s.n..n).enumerate().map(|(k, i)| (i, first + STREAM_STAGGER * k.min(12) as u32)));
        }
        (s.n, s.frame) = (n, frame);
        let curve = stream_curve();
        s.born.retain(|(_, at)| {
            now < *at
                || Spring::from_rest(1., FLICK_RESPONSE, now.saturating_duration_since(*at).as_secs_f32()) > 0.01
                || now.saturating_duration_since(*at) < STREAM_FADE
        });
        if s.born.is_empty() {
            return Vec::new();
        }
        self.want_frame();
        s.born
            .iter()
            .map(|&(i, at)| {
                let t = now.checked_duration_since(at).map_or(-1., |d| d.as_secs_f32());
                if t < 0. {
                    return (i, 0., LIFT);
                }
                let clip = 1. - Spring::from_rest(1., FLICK_RESPONSE, t);
                (i, clip, curve.sample((t / STREAM_FADE.as_secs_f32()).min(1.)))
            })
            .collect()
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

    /// The palette's ink, set as a frame begins (afterglows).
    pub(crate) fn set_ink(&self, ink: Hsla) {
        self.ink.set(ink);
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
        self.woke.get().map_or(now, |(due, _)| due.max(now))
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
        self.clock_fn()()
    }

    /// The clock, to read later (a timer's task).
    fn clock_fn(&self) -> impl Fn() -> Instant + 'static {
        #[cfg(test)]
        let (pin, skew) = (self.pin.clone(), self.skew.clone());
        move || {
            #[cfg(test)]
            if let Some(at) = pin.get() {
                return at;
            }
            let now = Instant::now();
            #[cfg(test)]
            let now = {
                let (s, d) = (skew.get(), Duration::from_nanos(skew.get().unsigned_abs() as u64));
                if s >= 0 { now + d } else { now - d }
            };
            now
        }
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
        self.lively.set(self.ambient.get());
        self.stale.set(None);
        #[cfg(test)]
        self.paint_log.borrow_mut().clear();
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
        self.springs.borrow_mut().retain(|_, s| alive(s.frame));
        self.cells.borrow_mut().retain(|_, c| alive(c.1));
        self.hovers.borrow_mut().retain(|_, h| alive(h.1));
        self.glows.borrow_mut().retain(|_, s| alive(s.frame));
        self.rolls.borrow_mut().retain(|_, r| alive(r.frame));
        {
            let rolls = self.rolls.borrow();
            self.roll_n.borrow_mut().retain(|k, _| rolls.contains_key(k));
        }
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

    /// An opaque strip painted over the list after its glyphs (the sticky
    /// band, polish.md F8): the beat layer, painted after the whole frame,
    /// paints nothing under it. Every site recorded so far whose clip meets
    /// `cover` starts below it. The band spans the column from the view's
    /// top, so trimming the top is exact: the clip's part it also drops,
    /// in the gutters beside the band, holds no glyph. Sites recorded
    /// outside the list (the rail, the tabs) never meet it.
    pub(crate) fn cover_top(&self, cover: Bounds<Pixels>) {
        for s in self.sites.borrow_mut().iter_mut() {
            let m = &mut s.mask.bounds;
            if m.intersects(&cover) {
                let bottom = m.bottom();
                m.origin.y = cover.bottom().min(bottom);
                m.size.height = bottom - m.origin.y;
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn sites(&self) -> Vec<Site> {
        self.sites.borrow().clone()
    }

    /// Glyphs the beat layer paints (the bench).
    pub(crate) fn site_count(&self) -> usize {
        self.sites.borrow().len()
    }

    /// Lattices the beat layer paints (the bench: `indicators`).
    pub(crate) fn lattice_count(&self) -> usize {
        self.sites.borrow().iter().filter(|s| matches!(s.beat, Beat::Lattice(_) | Beat::Spin(_))).count()
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
            if self.pane.get() {
                self.pane_wants.set(true);
            }
        }
    }

    /// A view pane (Settings, Usage: cached views of their own) draws: what
    /// moves in it asks the app to draw it again ([`Motion::pane_wants`]).
    pub(crate) fn in_pane(&self, on: bool) {
        self.pane.set(on);
    }

    /// Something in the view pane moved last frame: draw it again.
    pub(crate) fn pane_wants(&self) -> bool {
        self.pane_wants.replace(false)
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
        self.beat_on(every)
    }

    /// The connection words' dots' beat (motion.md §6.7: text, not a
    /// spinner): [`Self::beat`], and on while the link is lost
    /// ([`Self::hold_still`]): the one thing that steps then.
    pub(crate) fn words_beat(&self, every: u32) -> Option<u32> {
        if !self.lively.get() {
            return None;
        }
        self.beat_on(every)
    }

    fn beat_on(&self, every: u32) -> Option<u32> {
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
        if self.stale.get().is_none() {
            self.want_at(self.on_grid(at, BEAT * 8));
        }
    }

    /// The second this frame belongs to (Shady #17 M2): the last instant of
    /// the window's 1 s grid at or before the frame's. Clocks read their
    /// whole seconds here, not at the frame's own instant, which is late by
    /// up to a refresh: read late, a clock whose next second fell inside
    /// that lateness showed it already, asked for the one after (rounded
    /// up a whole second), and the second between was never drawn.
    /// Tests: `what` painted now ([`Self::paint_log`]).
    #[cfg(test)]
    pub(crate) fn painted(&self, what: &'static str) {
        self.paint_log.borrow_mut().push(what);
    }

    /// The link lost (R2-L1), the instant of the loss: the clocks hold.
    pub(crate) fn clock_now(&self) -> Instant {
        let grid = (BEAT * 8).as_nanos();
        let now = self.stale.get().map_or(self.now(), |at| at.min(self.now()));
        let n = now.saturating_duration_since(self.epoch).as_nanos() / grid;
        self.epoch + Duration::from_nanos((grid * n) as u64)
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

    /// `write_on` (polish.md P4, finish gate #17): a mark writing on over
    /// the glyph it replaces, never an empty box. The old glyph's light
    /// goes to the ground over 2 × [`WRITE_DELAY`], linearly; from
    /// [`WRITE_DELAY`] the strokes draw over [`BASE`] on [`ease::WRITE`]
    /// (the cross's two strokes 100 ms each), so the two overlap. The old
    /// one's light (1 → 0) and the drawn fraction (0 → 1, its strokes cut
    /// at it). Reduced and Off: at once.
    pub(crate) fn write_on(&self, since: Option<Instant>) -> (f32, f32) {
        let drawn = self.shot(since, WRITE_DELAY, BASE, &ease::WRITE, Kind::Spatial);
        let old = 1. - self.shot(since, Duration::ZERO, WRITE_DELAY * 2, &ease::LINEAR, Kind::Spatial);
        (old, drawn)
    }

    /// The frame a spinner showed at `at` (its beat then): what a write-on
    /// takes over. `None` (its still) when ambient is off.
    pub(crate) fn beat_at(&self, at: Instant) -> Option<u32> {
        self.ambient.get().then(|| (at.saturating_duration_since(self.epoch).as_nanos() / BEAT.as_nanos()) as u32)
    }

    /// A live indicator's beat (indicators/proposal.md §6): the shared
    /// grid's beat now, the next frame asked for where `pat`'s pose next
    /// changes (none for one that holds). `None` when ambient is off
    /// (Reduced, Off, low power): its still pose.
    pub(crate) fn lattice_beat(&self, pat: crate::lattice::Pat) -> Option<u32> {
        if !self.ambient.get() {
            return None;
        }
        let beat = (self.now().saturating_duration_since(self.epoch).as_nanos() / BEAT.as_nanos()) as u32;
        if let Some(next) = crate::lattice::next_change(pat, beat) {
            self.want_at(self.epoch + BEAT * next);
        }
        Some(beat)
    }

    /// A text slot rolling from one value to the next (cards4/proposal.md
    /// §8 #5's clock, #17's step) since `at`: the old one's travel out (90
    /// ms, `in`) and the new one's in (`snap` from +60, 2 × `STAGGER`), each
    /// 0..1. The old is gone before the new is half in. At rest: (1, 1).
    pub(crate) fn handover(&self, at: Option<Instant>) -> (f32, f32) {
        let old = self.shot(at, Duration::ZERO, exit(QUICK), &ease::IN, Kind::Spatial);
        let Some(at) = at.filter(|_| self.level.get() == Level::Full) else { return (old, 1.) };
        let start = at + 2 * STAGGER;
        let now = self.now();
        if now < start {
            self.want_at(start);
            return (old, 0.);
        }
        // `snap`'s spring, from rest: no overshoot.
        let x = Spring::from_rest(1., SNAP_RESPONSE, now.saturating_duration_since(start).as_secs_f32());
        if x < ROLL_EPSILON {
            return (old, 1.);
        }
        self.want_frame();
        (old, 1. - x)
    }

    /// A ping (cards4/proposal.md §8 #7, #8): one ring going out from a
    /// glyph over `sig`, `delay` after `since`, once: its progress 0..1
    /// while it runs, `None` before and after (and under Reduced and Off,
    /// where it never plays). Never a loop: a held wait is still.
    pub(crate) fn ping(&self, since: Option<Instant>, delay: Duration) -> Option<f32> {
        let since = since.filter(|_| self.level.get() == Level::Full)?;
        if self.now() < since + delay {
            self.want_at(since + delay);
            return None;
        }
        let p = self.shot(Some(since), delay, SIG, &ease::OUT, Kind::Spatial);
        (p < 1.).then_some(p)
    }

    pub(crate) fn set_scale(&self, scale: f32) {
        self.scale.set(scale.max(0.5));
    }

    /// `v` logical px on the nearest device pixel (a moving 2 px quad stays
    /// solid: no anti-aliased edge, which is alpha).
    pub(crate) fn snap(&self, v: f32) -> f32 {
        let s = self.scale.get();
        (v * s).round() / s
    }

    /// The link is lost (motion.md §5.3, Shady #17 M1), `since` then: the
    /// window is visibly stale and nothing in it is live. No ambient from
    /// here to the frame's end: the spinners and orbits draw still, and the clocks hold their count as of the loss (R2-L1: a
    /// worker's clock never counts through an outage); the connection
    /// words' dots are all that steps.
    pub(crate) fn hold_still(&self, since: Option<Instant>) {
        self.ambient.set(false);
        self.stale.set(since);
    }

    /// The star hop's offset (polish.md P6), `since` the favouriting: `−v·t·e^(−ωt)`
    /// on `glide`'s ω, never above 0 (critically damped: it rises and comes
    /// back, no overshoot), 0 once settled. Full only.
    pub(crate) fn hop(&self, since: Option<Instant>) -> f32 {
        let Some(since) = since.filter(|_| self.level.get() == Level::Full) else { return 0. };
        let t = self.now().saturating_duration_since(since).as_secs_f32();
        let y = hop_at(t);
        if t > 0.05 && y.abs() < 0.05 {
            return 0.;
        }
        self.want_frame();
        y
    }

    /// The sparks' progress along their 8 px (polish.md P6) on `glide`: 0 → 1;
    /// `None` once there (or not Full).
    pub(crate) fn sparks(&self, since: Option<Instant>) -> Option<f32> {
        let since = since.filter(|_| self.level.get() == Level::Full)?;
        let t = self.now().saturating_duration_since(since).as_secs_f32();
        let x = 1. - Spring::from_rest(1., GLIDE_RESPONSE, t);
        if x > 0.98 {
            return None;
        }
        self.want_frame();
        Some(x)
    }

    /// How many beats (of `every`) from the window's epoch `at` is, if it
    /// is on that grid (tests: what a timer is armed for).
    #[cfg(test)]
    pub(crate) fn on_beats(&self, at: Instant, every: u32) -> bool {
        at.saturating_duration_since(self.epoch).as_nanos().is_multiple_of((BEAT * every).as_nanos())
    }

    /// The rolls under way in the key space `space` (all: `None`) (tests).
    #[cfg(test)]
    pub(crate) fn rolling_in(&self, space: Option<u64>) -> Vec<u64> {
        let rolls = self.rolls.borrow();
        rolls.iter().filter(|(k, r)| space.is_none_or(|s| *k & (0xFFFF << 48) == s) && r.since.is_some()).map(|(k, _)| *k).collect()
    }

    /// The count `key` showed last ([`roll_count`]).
    pub(crate) fn roll_was(&self, key: u64) -> Option<u64> {
        self.roll_n.borrow().get(&key).copied()
    }

    pub(crate) fn roll_saw(&self, key: u64, n: u64) {
        self.roll_n.borrow_mut().insert(key, n);
    }

    /// `text` of `key` this frame (polish.md P5), rolling if it changed while
    /// drawn at Full: the text it left, how far it still travels (1 → 0 of
    /// its line, on `snap`: no overshoot) and which way (`up`: the new one
    /// comes in from below). First drawn, not drawn last frame (history, a
    /// switch), Reduced or Off: at rest.
    pub(crate) fn roll(&self, key: u64, text: &SharedString, up: bool) -> Option<(SharedString, f32, bool)> {
        let (frame, now) = (self.frame.get(), self.now());
        let mut rolls = self.rolls.borrow_mut();
        let r = rolls.entry(key).or_insert_with(|| Rolling { value: text.clone(), prev: text.clone(), since: None, up, frame });
        if r.value != *text {
            let watched = r.frame + 1 >= frame && self.level.get() == Level::Full;
            r.prev = std::mem::replace(&mut r.value, text.clone());
            r.since = watched.then_some(now);
            r.up = up;
        }
        r.frame = frame;
        let since = r.since?;
        let x = Spring::from_rest(1., SNAP_RESPONSE, now.saturating_duration_since(since).as_secs_f32());
        if x < ROLL_EPSILON {
            r.since = None;
            return None;
        }
        // On the shared 30 Hz grid, not the display's rate (Shady #17
        // R2-M1): a roll is an app frame, and a live fan-out rolls a count
        // every fraction of a second; N rolls share the ticks, and with the
        // composer's sweep on screen they land on its frames.
        self.want_tick();
        Some((r.prev.clone(), x, r.up))
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

    /// Something small arriving in a row (a worker chip, polish.md P13),
    /// `born` then (staggered: maybe still to come): its content's travel
    /// from below ([`ARRIVE`] → 0 on `flick`) and its strength ([`LIFT`] →
    /// 1 over `quick` `out`). Not born, or not fluid: at rest.
    pub(crate) fn arrival(&self, born: Option<Instant>) -> (f32, f32) {
        let Some(born) = born.filter(|_| self.fluid()) else { return (0., 1.) };
        let Some(since) = self.now().checked_duration_since(born) else {
            self.want_at(born);
            return (ARRIVE, LIFT);
        };
        let t = since.as_secs_f32();
        let travel = Spring::from_rest(ARRIVE, FLICK_RESPONSE, t);
        let strength = LIFT + (1. - LIFT) * ease::OUT.sample((t / QUICK.as_secs_f32()).clamp(0., 1.));
        if travel < FLUID_EPSILON && strength >= 1. {
            return (0., 1.);
        }
        self.want_frame();
        (travel, strength)
    }

    /// Row `key` is revealing: growing, or born too lately to have begun.
    pub(crate) fn revealing(&self, key: u64) -> bool {
        self.grows.borrow().contains_key(&key) || self.birth(key).is_some_and(|b| self.now().saturating_duration_since(b) < QUICK)
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

    // ── chrome (fluid.md §3.5–3.10, motion.md PR 5) ───────────────────

    /// Chrome position `key` this frame, sprung toward `target` (fluid.md
    /// §2.1): when the target moves it starts where it is, with its
    /// velocity. `from`: where it starts the first time it is drawn (a
    /// tab's width from 0); none, at its target. Not drawn last frame, or
    /// not [`Level::Full`]: at its target. Its value, and whether it moves.
    pub(crate) fn spring(&self, key: u64, from: Option<f32>, target: f32, response: f32) -> (f32, bool) {
        let (frame, now) = (self.frame.get(), self.now());
        let full = self.level.get() == Level::Full;
        let mut springs = self.springs.borrow_mut();
        let start = from.filter(|_| full).unwrap_or(target);
        let s = springs.entry(key).or_insert(Sprung { target, x0: start - target, v0: 0., at: now, frame, response });
        if s.frame + 1 < frame || !full {
            *s = Sprung { target, x0: 0., v0: 0., at: now, frame, response };
        }
        if s.target != target {
            let (x, v) = s.state(now);
            *s = Sprung { target, x0: x + s.target - target, v0: v, at: now, frame, response };
        }
        s.response = response;
        s.frame = frame;
        if s.settled(now) {
            return (target, false);
        }
        self.want_frame();
        (target + s.state(now).0, true)
    }

    /// [`Motion::spring`] for what only moves when told to: at rest, a new
    /// target is taken at once (a resize, a scroll: `follow`); moving, it
    /// retargets. [`Motion::spring_jump`] starts a move.
    pub(crate) fn spring_held(&self, key: u64, target: f32, response: f32) -> (f32, bool) {
        let now = self.now();
        if let Some(s) = self.springs.borrow_mut().get_mut(&key)
            && s.target != target
            && s.settled(now)
        {
            *s = Sprung { target, x0: 0., v0: 0., at: now, ..*s };
        }
        self.spring(key, None, target, response)
    }

    /// [`Motion::spring_held`] on whole device pixels (a side panel's
    /// width: GPUI lays boxes out on them anyway). Its value snapped; the
    /// next frame is asked for only when the snapped value will have
    /// changed by then, else a timer for the instant it does (or the spring
    /// settles): the slow tail moves under a pixel a frame, and no frame
    /// draws what the last one drew. Its value, and whether it moves.
    pub(crate) fn spring_px(&self, key: u64, target: f32, response: f32) -> (f32, bool) {
        let asked = (self.display.get(), self.pane_wants.get());
        let (v, moving) = self.spring_held(key, target, response);
        let shown = self.snap(v);
        if !moving {
            return (shown, false);
        }
        self.display.set(asked.0);
        self.pane_wants.set(asked.1);
        let now = self.now();
        let Some(s) = self.springs.borrow().get(&key).copied() else { return (shown, true) };
        let next = (1..=1000u64)
            .map(|k| now + Duration::from_millis(k))
            .find(|&t| s.settled(t) || self.snap(s.target + s.state(t).0) != shown)
            .unwrap_or(now + Duration::from_secs(1));
        if next <= now + PX_FRAME {
            self.want_frame();
        } else {
            self.want_at(next);
        }
        (shown, true)
    }

    /// Value `key` eased toward `to` (a label's colour, as 0..1): over `up`
    /// (rising) or `down` (falling) for a whole 0 → 1, from where it is.
    /// First drawn, not drawn last frame, or Off: at `to`. Reduced: over
    /// at most [`QUICK`].
    pub(crate) fn ease_to(&self, key: u64, to: f32, up: Duration, down: Duration) -> f32 {
        let (frame, now) = (self.frame.get(), self.now());
        let mut hovers = self.hovers.borrow_mut();
        let h = hovers.entry(key).or_insert((Tween::rest(to, now), frame));
        if h.1 + 1 < frame || !self.on() {
            h.0 = Tween::rest(to, now);
        }
        h.1 = frame;
        let full = match self.level.get() {
            Level::Full => {
                if to > h.0.sample(now, &ease::OUT) {
                    up
                } else {
                    down
                }
            }
            _ => QUICK.min(if to > h.0.sample(now, &ease::OUT) { up } else { down }),
        };
        h.0.retarget(to, now, full, &ease::OUT);
        if h.0.running(now) {
            self.want_frame();
        }
        h.0.sample(now, &ease::OUT)
    }

    /// Chrome position `key` moves to `target` from wherever it was drawn
    /// last (`at`: there, now), as [`Motion::spring`] will draw it.
    pub(crate) fn spring_jump(&self, key: u64, at: f32, target: f32, response: f32) {
        if self.level.get() != Level::Full {
            return;
        }
        let (frame, now) = (self.frame.get(), self.now());
        self.springs.borrow_mut().insert(key, Sprung { target, x0: at - target, v0: 0., at: now, frame, response });
    }

    /// A grid item's place on one axis, `at` px: cell `cell` of its grid
    /// (a worker chip's column or line). A new cell is a move: from where
    /// it was drawn, in px on `snap`, to the end (no cap, no half-cell
    /// rest: [`Slide`]'s epsilon is a pixel's). A new pitch in the same
    /// cell (the column's share as a panel lands) is taken at once. Its
    /// paint offset. Not drawn last frame, or not [`Level::Full`]: 0.
    pub(crate) fn cell_slide(&self, key: u64, cell: u64, at: f32) -> f32 {
        let frame = self.frame.get();
        let moved = self.cells.borrow_mut().insert(key, (cell, frame)).is_some_and(|(c, f)| c != cell && f + 1 >= frame);
        if moved && let Some(was) = self.spring_at(key) {
            self.spring_jump(key, was, at, SNAP_RESPONSE);
        }
        self.spring_held(key, at, SNAP_RESPONSE).0 - at
    }

    /// Where chrome position `key` was drawn last, if it is on a spring.
    pub(crate) fn spring_at(&self, key: u64) -> Option<f32> {
        let now = self.now();
        self.springs.borrow().get(&key).map(|s| s.target + if s.settled(now) { 0. } else { s.state(now).0 })
    }

    /// The pointer went over (or off) control `key` (its `on_hover`): its
    /// fill comes on over [`HOVER_IN`] and goes off over [`HOVER_OUT`]
    /// (fluid.md §3.9: on fast, off decays). Reduced: on at once, off over
    /// [`QUICK`]. Off: at once.
    pub(crate) fn set_hover(&self, key: u64, on: bool) {
        let (frame, now) = (self.frame.get(), self.now());
        let to = if on { 1. } else { 0. };
        let full = match (self.level.get(), on) {
            (Level::Full, true) => HOVER_IN,
            (Level::Full, false) => HOVER_OUT,
            (Level::Reduced, false) => QUICK,
            _ => Duration::ZERO,
        };
        let mut hovers = self.hovers.borrow_mut();
        let h = hovers.entry(key).or_insert((Tween::rest(1. - to, now), frame));
        if full.is_zero() {
            h.0 = Tween::rest(to, now);
        } else {
            h.0.retarget(to, now, full, &ease::OUT);
        }
        h.1 = frame;
    }

    /// Control `key`'s hover (0 at rest … 1 hovered) this frame.
    pub(crate) fn hover(&self, key: u64) -> f32 {
        let (frame, now) = (self.frame.get(), self.now());
        let mut hovers = self.hovers.borrow_mut();
        let Some(h) = hovers.get_mut(&key) else { return 0. };
        h.1 = frame;
        if h.0.running(now) {
            self.want_frame();
        }
        h.0.sample(now, &ease::OUT)
    }

    /// Control `key`'s fill this frame: `rest` (none: no fill of its own)
    /// toward `hover` as it is hovered, and `press` while it is held.
    /// Its fill and its content's offset (`PRESS` px down while held,
    /// springing back on `flick` once let go).
    pub(crate) fn fill(&self, key: u64, ground: Hsla, rest: Option<Hsla>, hover: Hsla, press: Hsla) -> (Option<Hsla>, f32) {
        let h = self.hover(key);
        let (p, dy) = self.pressed(key);
        let base = rest.unwrap_or(ground);
        let lit = if h > 0. { Some(mix(base, hover, h)) } else { rest };
        if p > 0. {
            let under = lit.unwrap_or(base);
            return (Some(mix(under, press, p)), dy);
        }
        (lit, dy)
    }

    /// Control `key` is pressed (the pointer went down on it): its press
    /// look is drawn in the next frame, whatever the level (it is state,
    /// not motion).
    pub(crate) fn press_down(&self, key: u64) {
        self.press.set(Some((key, None)));
    }

    /// A control of the `space` (its keys' top 16 bits) is held down.
    pub(crate) fn held(&self, space: u64) -> bool {
        self.press.get().is_some_and(|(k, up)| up.is_none() && k >> 48 == space >> 48)
    }

    /// The pointer went up: the press lets go (springing back on `flick`).
    pub(crate) fn press_up(&self) -> bool {
        let Some((key, None)) = self.press.get() else { return false };
        self.press.set(Some((key, Some(self.now()))));
        true
    }

    /// Control `key`'s press this frame: its fill toward `press*` (1 held …
    /// 0) and its content's offset down.
    pub(crate) fn pressed(&self, key: u64) -> (f32, f32) {
        let Some((_, up)) = self.press.get().filter(|p| p.0 == key) else { return (0., 0.) };
        let full = self.level.get() == Level::Full;
        let travel = if full { PRESS } else { 0. };
        let now = self.now();
        let Some(up) = up.filter(|u| now >= *u) else {
            if let Some(Some(u)) = self.press.get().map(|p| p.1) {
                self.want_at(u);
            }
            return (1., travel);
        };
        if !full {
            self.press.set(None);
            return (0., 0.);
        }
        let x = Spring::from_rest(1., FLICK_RESPONSE, now.saturating_duration_since(up).as_secs_f32());
        if x < PRESS_EPSILON {
            self.press.set(None);
            return (0., 0.);
        }
        self.want_frame();
        (x, x * travel)
    }

    /// An overlay that opened at `born` (none: from the keyboard, or at
    /// rest): its strength (0 → 1 over [`QUICK`] `out`) and its rise
    /// ([`RISE_FLUID`] → 0 on `flick`, toward its anchor), fluid.md §3.10.
    /// Reduced: opacity only. Off: there at once.
    pub(crate) fn enter(&self, born: Option<Instant>) -> (f32, f32) {
        let Some(born) = born.filter(|_| self.on()) else { return (1., 0.) };
        let a = self.shot(Some(born), Duration::ZERO, QUICK, &ease::OUT, Kind::Fade);
        if self.level.get() != Level::Full {
            return (a, 0.);
        }
        let rise = Spring::from_rest(RISE_FLUID, FLICK_RESPONSE, self.now().saturating_duration_since(born).as_secs_f32());
        if rise >= FLUID_EPSILON {
            self.want_frame();
            return (a, rise);
        }
        (a, 0.)
    }

    /// An overlay that closed at `at`: its strength (1 → 0 over 90 ms
    /// `in`) and how far it has sunk back toward its anchor ([`SINK`]);
    /// none once gone (or not on: gone at once).
    pub(crate) fn leave(&self, at: Instant) -> Option<(f32, f32)> {
        if !self.on() {
            return None;
        }
        let p = self.shot(Some(at), Duration::ZERO, exit(QUICK), &ease::IN, Kind::Fade);
        if p >= 1. {
            return None;
        }
        let sink = if self.level.get() == Level::Full { SINK * p } else { 0. };
        Some((1. - p, sink))
    }

    /// Number `key`'s colour this frame (motion.md §6.6): `role`, and for
    /// [`GLOW`] after `value` changed while drawn, mixed toward the ink,
    /// stepping 1 → 0.66 → 0.33 → 0 (three frames, not 30 a second).
    /// First drawn, or not drawn last frame: at rest. Reduced: lit for
    /// [`QUICK`]. Off: never.
    pub(crate) fn glow(&self, key: u64, value: u64, role: Hsla) -> Hsla {
        let k = self.glow_k(key, value);
        if k > 0. { mix(role, self.ink.get(), k) } else { role }
    }

    /// When `value` of `key` last changed while drawn, and what it was:
    /// none first drawn or not drawn last frame (at rest), or under Off.
    pub(crate) fn changed(&self, key: u64, value: u64) -> Option<(u64, Instant)> {
        let frame = self.frame.get();
        let mut glows = self.glows.borrow_mut();
        let s = glows.entry(key).or_insert(Seen { value, prev: value, since: None, frame });
        if s.value != value {
            s.since = (s.frame + 1 >= frame && self.on()).then_some(self.now());
            (s.prev, s.value) = (s.value, value);
        }
        s.frame = frame;
        s.since.map(|t| (s.prev, t))
    }

    /// A meter's segment `i` (motion.md §6.6): `lit` now or not, of a
    /// meter keyed `key` with `filled` lit. Newly lit ones light in order
    /// (from the low end, `stagger` apart), each `edge` → `bar` over
    /// [`QUICK`]; ones going out go from the high end. Its colour.
    pub(crate) fn segment(&self, key: u64, filled: usize, i: usize, edge: Hsla, bar: Hsla) -> Hsla {
        let lit = i < filled;
        let Some((prev, since)) = self.changed(key, filled as u64) else { return if lit { bar } else { edge } };
        let prev = prev as usize;
        let k = if filled > prev && (prev..filled).contains(&i) {
            i - prev
        } else if filled < prev && (filled..prev).contains(&i) {
            prev - 1 - i
        } else {
            return if lit { bar } else { edge };
        };
        let t = self.shot(Some(since), stagger(k), QUICK, &ease::OUT, Kind::Fade);
        if lit { mix(edge, bar, t) } else { mix(bar, edge, t) }
    }

    /// The afterglow's step (1, 0.66, 0.33, 0) of `key` showing `value`.
    pub(crate) fn glow_k(&self, key: u64, value: u64) -> f32 {
        let (frame, now) = (self.frame.get(), self.now());
        let mut glows = self.glows.borrow_mut();
        let s = glows.entry(key).or_insert(Seen { value, prev: value, since: None, frame });
        if s.value != value {
            s.since = (s.frame + 1 >= frame && self.on()).then_some(now);
            (s.prev, s.value) = (s.value, value);
        }
        s.frame = frame;
        let k = self.afterglow(s.since);
        if k <= 0. && s.since.is_some_and(|t| t <= now) {
            s.since = None;
        }
        k
    }

    /// An afterglow's `k` (§6.6) at the frame's time, for one that began at
    /// `since` (a time still to come: due then, 0 until it is): 1 → 0.66 →
    /// 0.33 → 0 over `glow` at Full, one step of `quick` at Reduced, 0 at
    /// Off. Asks for the frame its next step is due at.
    pub(crate) fn afterglow(&self, since: Option<Instant>) -> f32 {
        let Some(since) = since.filter(|_| self.on()) else { return 0. };
        let now = self.now();
        if now < since {
            self.want_at(since);
            return 0.;
        }
        let dur = if self.level.get() == Level::Full { GLOW } else { QUICK };
        let gone = now.saturating_duration_since(since);
        if gone >= dur {
            return 0.;
        }
        let steps = if self.level.get() == Level::Full { 3 } else { 1 };
        let step = dur / steps;
        let n = (gone.as_nanos() / step.as_nanos()) as u32;
        self.want_at(since + step * (n + 1));
        1. - n as f32 / steps as f32
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
        let (mut wants, mut next) = (false, None::<u32>);
        for s in self.sites.borrow().iter() {
            let quad = |x: f32, y: f32, w: f32, h: f32, c: Hsla, window: &mut Window| {
                window.with_content_mask(Some(s.mask), |window| {
                    window.paint_quad(fill(Bounds::new(s.origin + point(px(x), px(y)), size(px(w), px(h))), c));
                });
            };
            // A lattice (indicators/proposal.md §6): its pose at this beat,
            // and the beat it next changes on (the timer goes there).
            let mut lattice = |n: f32, pat: crate::lattice::Pat, window: &mut Window| {
                wants = true;
                let d = crate::lattice::Dots::of(crate::lattice::pose(pat, Some(beat)));
                window.with_content_mask(Some(s.mask), |window| crate::lattice::paint(window, s.origin, n, d, s.mark, s.ground));
                if let Some(b) = crate::lattice::next_change(pat, beat) {
                    next = Some(next.map_or(b, |n: u32| n.min(b)));
                }
            };
            match s.beat {
                Beat::Spin(g) => lattice(10., crate::glyph::live_pat(g), window),
                Beat::Lattice(pat) => lattice(crate::glyph::LIFE, pat, window),
                Beat::Breath(at) => {
                    let step = beat.saturating_sub((at.saturating_duration_since(self.epoch).as_nanos() / BEAT.as_nanos()) as u32);
                    if step < BREATH_BEATS * BREATHS {
                        wants = true;
                        // Its cell steps every other beat (as before).
                        let b = (beat / 2 + 1) * 2;
                        next = Some(next.map_or(b, |n: u32| n.min(b)));
                        let (x, y, w) = crate::glyph::BREATH_CELL;
                        quad(x, y, w, w, crate::glyph::breath_cell(step, s.mark, s.ground), window);
                    }
                }
            }
        }
        if wants {
            #[cfg(test)]
            self.painted.borrow_mut().push(beat);
        }
        // The one timer to the earliest pose change (running: twice a
        // 4-beat step; streaming, a chase and the breath: every beat).
        if let Some(n) = next {
            self.beat_next.set(Some(self.epoch + BEAT * n));
        }
        if !self.display.get() {
            self.arm(cx, app, layer);
        }
    }

    /// Arms the one timer to the earliest change: the app's (it draws a
    /// frame, the layer with it) or the beat's (only the layer).
    fn arm(&self, cx: &mut App, app: EntityId, layer: EntityId) {
        let target = match (self.next.get(), self.beat_next.get()) {
            // The app's change and the layer's within a hair (a clock's
            // second and the 30 Hz grid's 30th tick, 10 ns apart): one wake,
            // the app's (its frame paints the layer too).
            (Some(a), Some(b)) if a.max(b).saturating_duration_since(a.min(b)) < Duration::from_millis(2) => Some((a.max(b), Wake::App)),
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
        let clock = self.clock_fn();
        let task = cx.spawn(async move |cx| {
            cx.background_executor().timer(wait).await;
            // The beat layer's wake is for a pose change that can be beats
            // away (indicators/proposal.md §6): one that fired more than an
            // OS timer's slack ([`EARLY`]) ahead of it sleeps the rest.
            if wake == Wake::Beat {
                let rest = at.saturating_duration_since(clock());
                if rest > EARLY {
                    cx.background_executor().timer(rest).await;
                }
            }
            armed.set(None);
            woke.set(Some((at, wake)));
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

    use gpui_kit::{Bounds, SharedString, point, px, size};

    use super::{
        BASE, BEAT, DECAY_TICK, GLOW, HEAD_HOLD, Kind, LINK_GRACE, Level, Link, Lock, Motion, QUICK, RELOAD_GRACE, SLIDE_CAP, SLOW,
        STAGGER, STAGGER_CAP, Setting, Slide, Tag, Tween, ease, exit, hop_at, stagger,
    };

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// H2 (indicators/proposal.md §6, §8 `lattice_wakes_only_on_pose_changes`):
    /// a running lattice asks for a frame only where its pose next changes,
    /// on the shared beat grid: running twice a 4-beat step (4 a second),
    /// streaming and a chase every beat. Phase-locked: two drawn in a frame
    /// read the same. A frame that draws none asks for nothing.
    #[test]
    fn lattice_wakes_only_on_pose_changes() {
        use crate::lattice::{Pat, next_change, pose};
        let t0 = Instant::now();
        let m = Motion::new(t0);
        let mut wakes = Vec::new();
        let mut at = t0 + ms(5);
        // Two seconds of a running lattice, frame by frame at its wakes.
        while at < t0 + ms(2000) {
            m.begin_frame(Level::Full, false, at);
            let b = m.lattice_beat(Pat::Run).expect("Full: it runs");
            assert_eq!(m.lattice_beat(Pat::Run), Some(b), "one clock app-wide");
            let (display, next) = m.demand();
            assert!(!display, "no display-rate frame");
            let next = next.expect("its next pose asked for");
            assert_eq!(next, t0 + BEAT * next_change(Pat::Run, b).unwrap(), "at its next pose change");
            assert_ne!(pose(Pat::Run, Some(b)), pose(Pat::Run, Some((next.duration_since(t0).as_nanos() / BEAT.as_nanos()) as u32)));
            wakes.push(next);
            at = next;
        }
        assert_eq!(wakes.len(), 8, "4 wakes a second: {wakes:?}");
        m.begin_frame(Level::Full, false, at);
        let b = m.lattice_beat(Pat::Stream).unwrap();
        assert_eq!(m.demand().1, Some(t0 + BEAT * (b + 1)), "streaming: every beat");
        m.begin_frame(Level::Full, false, at + ms(1));
        assert_eq!(m.demand(), (false, None), "nothing running drawn: nothing asked");
    }

    /// Reduced, Off and low power: a lattice is its still pose and asks for
    /// no frame (0 frames).
    #[test]
    fn lattice_is_still_when_reduced_or_off() {
        let t0 = Instant::now();
        let m = Motion::new(t0);
        for (level, low) in [(Level::Reduced, false), (Level::Off, false), (Level::Full, true)] {
            m.begin_frame(level, low, t0 + ms(10));
            for p in [crate::lattice::Pat::Run, crate::lattice::Pat::Stream, crate::lattice::Pat::Reconnect] {
                assert_eq!(m.lattice_beat(p), None, "{level:?} low power {low}: still");
            }
            assert_eq!(m.demand(), (false, None), "{level:?}: no frames");
        }
    }

    /// A wait pings once (§8 #7): its ring goes out over `sig` and is gone;
    /// a wait held for a minute does not ping again. Reduced and Off: no ping.
    #[test]
    fn ping_runs_once_per_wait() {
        let t0 = Instant::now();
        let m = Motion::new(t0);
        let mut seen = 0;
        for k in 0..=120 {
            m.begin_frame(Level::Full, false, t0 + ms(k * 10));
            if let Some(p) = m.ping(Some(t0), Duration::ZERO) {
                assert!((0. ..1.).contains(&p));
                seen += 1;
            }
        }
        assert!((50..=61).contains(&seen), "one ping over sig: {seen} frames");
        m.begin_frame(Level::Full, false, t0 + ms(60_000));
        assert_eq!(m.ping(Some(t0), Duration::ZERO), None, "held: still");
        assert_eq!(m.demand(), (false, None));
        for level in [Level::Reduced, Level::Off] {
            m.begin_frame(level, false, t0 + ms(100));
            assert_eq!(m.ping(Some(t0), Duration::ZERO), None, "{level:?}: no ping");
        }
    }

    /// A rolling slot hands over (§8 #5, #17): the old value is all the
    /// way out before the new one is half in, at every frame.
    #[test]
    fn a_step_roll_never_overlaps() {
        let t0 = Instant::now();
        let m = Motion::new(t0);
        for k in 0..400 {
            m.begin_frame(Level::Full, false, t0 + ms(k));
            let (old, new) = m.handover(Some(t0));
            assert!(new <= 0.5 || old >= 1., "at {k} ms: old {old}, new {new}");
        }
        m.begin_frame(Level::Reduced, false, t0 + ms(1));
        assert_eq!(m.handover(Some(t0)), (1., 1.), "Reduced: at once");
    }

    /// History never animates (§8): a card's state first seen (it loaded
    /// with the session, or scrolled back in) is at rest, its glyph drawn
    /// whole; only a change watched in the next frame plays.
    #[test]
    fn history_cards_are_drawn_at_rest() {
        let t0 = Instant::now();
        let m = Motion::new(t0);
        let (done, orbit) = (crate::glyph::Life::Done.code(), crate::glyph::Life::Run.code());
        m.begin_frame(Level::Full, false, t0);
        assert_eq!(m.change(Tag::Outcome, 7, done, None), None, "loaded done: at rest");
        assert_eq!(m.change(Tag::Outcome, 8, orbit, None), None);
        m.begin_frame(Level::Full, false, t0 + ms(16));
        assert_eq!(m.change(Tag::Outcome, 8, done, None).map(|c| c.1), Some(orbit), "watched: it plays from the orbit");
        // Not drawn for a frame (scrolled away): back at rest.
        m.begin_frame(Level::Full, false, t0 + ms(32));
        m.begin_frame(Level::Full, false, t0 + ms(48));
        m.begin_frame(Level::Full, false, t0 + ms(64));
        assert_eq!(m.change(Tag::Outcome, 7, crate::glyph::Life::Failed.code(), None), None, "unseen change: at rest");
    }

    /// A side panel's width on whole pixels ([`Motion::spring_px`]): a
    /// frame is asked for only when the width drawn will have changed by
    /// then, else a timer for the instant it does. Driven as the app is (the
    /// next 144 Hz frame, or the timer), a 280-px `sheet` fold draws each
    /// width once (the landing frame apart) and lands as the spring does:
    /// ~52 frames where drawing every frame of it takes ~71.
    #[test]
    fn a_pixel_spring_draws_each_width_once() {
        let t0 = Instant::now();
        let m = Motion::new(t0);
        let sheet = super::SHEET_RESPONSE;
        m.begin_frame(Level::Full, false, t0);
        m.spring_jump(9, 280., 0., sheet);
        let (mut t, mut drawn) = (t0, Vec::new());
        for _ in 0..200 {
            m.begin_frame(Level::Full, false, t);
            let (v, moving) = m.spring_px(9, 0., sheet);
            assert_eq!(v, v.round(), "whole pixels");
            drawn.push(v);
            if !moving {
                break;
            }
            t = match m.demand() {
                (true, _) => t + Duration::from_micros(6944),
                (false, Some(at)) => at,
                (false, None) => panic!("moving, and nothing asked for"),
            };
        }
        let ms = t.duration_since(t0).as_millis();
        assert_eq!(drawn.last(), Some(&0.), "landed");
        assert!((470..=520).contains(&ms), "as long as the spring: {ms} ms");
        let same = drawn.windows(2).filter(|p| p[0] == p[1]).count();
        assert!(same <= 1, "each width drawn once: {drawn:?}");
        assert!((45..=60).contains(&drawn.len()), "{} frames", drawn.len());
        assert!(drawn.windows(2).all(|p| p[1] <= p[0]), "one way");
        assert_eq!(m.demand(), (false, None), "at rest: nothing asked for");
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
        assert_eq!(m.write_on(Some(t0)), (0., 1.));
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

    /// The link (§5.3): a drop is nothing for its grace; past it the tear's
    /// two frames (60 ms, 40 ms), then the drain over `slow`; a reload's
    /// grace is longer, and each new goodbye restarts it; back after a
    /// loss, it restores and scans; a second loss within the budget doesn't
    /// tear again.
    #[test]
    fn the_link_tears_after_its_grace() {
        let t0 = Instant::now();
        let m = Motion::new(t0);
        let mut l = Link::default();
        let at = |m: &Motion, l: &mut Link, t: u64, up: bool, reload: Option<u64>| {
            m.begin_frame(Level::Full, false, t0 + ms(t));
            l.see(up, reload, m);
            (l.tear(m), l.drain(m))
        };
        let (no, yes) = (None, Some(1));
        at(&m, &mut l, 0, true, no);
        assert!(l.scan.is_some(), "the first hello scans");
        assert_eq!(at(&m, &mut l, 1000, false, no), (None, 0.), "dropped: the grace");
        assert_eq!(m.demand(), (false, Some(t0 + ms(1000) + LINK_GRACE)), "one timer, at the grace's end");
        assert_eq!(at(&m, &mut l, 1999, false, no), (None, 0.));
        assert_eq!(at(&m, &mut l, 2000, false, no), (Some(0), 0.), "lost: the tear's first frame");
        assert_eq!(at(&m, &mut l, 2061, false, no), (Some(1), 0.), "its second; the drain waits");
        assert_eq!(at(&m, &mut l, 2101, false, no).0, None);
        assert_eq!(at(&m, &mut l, 2100 + 360, false, no), (None, 1.), "drained");
        assert_eq!(at(&m, &mut l, 2600, true, no).1, 1., "back: from where it is");
        assert!(l.scan.is_some(), "back after a loss: it scans");
        assert_eq!(at(&m, &mut l, 2600 + 360, true, no).1, 0., "restored");
        // A reload: nothing for RELOAD_GRACE; a second goodbye 10 s in (the
        // client back for a moment between two frames) restarts it; then
        // a loss (past the last tear's budget: it tears).
        let rg = RELOAD_GRACE.as_millis() as u64;
        assert_eq!(at(&m, &mut l, 3000, false, yes), (None, 0.));
        let mut l2 = l;
        assert_eq!(at(&m, &mut l, 13_000, false, Some(2)), (None, 0.), "a second goodbye");
        assert_eq!(at(&m, &mut l2, 3000 + rg - 1, false, yes), (None, 0.), "reloading");
        assert_eq!(at(&m, &mut l2, 3000 + rg, false, yes).0, Some(0), "a reload that never came back: lost");
        assert_eq!(at(&m, &mut l, 3000 + rg, false, Some(2)), (None, 0.), "the second goodbye's own grace");
        assert_eq!(at(&m, &mut l, 13_000 + rg - 1, false, Some(2)), (None, 0.));
        assert_eq!(at(&m, &mut l, 13_000 + rg, false, Some(2)).0, Some(0), "lost, from the last goodbye");
        // Back, and lost again within FLASH_BUDGET: it drains, no tear.
        let t = 13_000 + rg;
        at(&m, &mut l, t + 200, true, no);
        at(&m, &mut l, t + 300, false, no);
        assert_eq!(at(&m, &mut l, t + 1300, false, no).0, None, "within the budget: no second tear");
        assert!(l.lost);
        assert!(at(&m, &mut l, t + 2000, false, no).1 > 0.99, "it drains all the same");
    }

    /// The first hello scans when the window started at `Connecting` (its
    /// first frames down), however long the daemon took; never before; and
    /// a launch that never connected has nothing to lose.
    #[test]
    fn the_first_hello_scans_from_connecting() {
        let t0 = Instant::now();
        let m = Motion::new(t0);
        let mut l = Link::default();
        for t in [0, 16, 5000, 20_000] {
            m.begin_frame(Level::Full, false, t0 + ms(t));
            l.see(false, None, &m);
            assert_eq!((l.scan, l.tear(&m), l.lost), (None, None, false), "connecting at {t} ms: nothing");
        }
        m.begin_frame(Level::Full, false, t0 + ms(20_100));
        l.see(true, None, &m);
        assert_eq!(l.scan, Some(t0 + ms(20_100)), "the first hello scans");
        // Off: no Scan, and the first hello is spent all the same.
        let mut l = Link::default();
        m.begin_frame(Level::Off, false, t0 + ms(30_000));
        l.see(false, None, &m);
        l.see(true, None, &m);
        assert!(l.ever_up && l.scan.is_none());
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
        assert_eq!(m.write_on(Some(t0)), (0., 1.));
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
        let run = crate::glyph::live_pat(crate::glyph::Glyph::Run);
        assert_eq!(crate::lattice::pose(run, old), crate::lattice::pose(run, fresh));
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

    /// P6: the hop rises 4 px at 48 ms and comes back with no overshoot.
    #[test]
    fn star_hop_never_passes_rest() {
        let peak = (0..400).map(|ms| hop_at(ms as f32 / 1000.)).fold(0f32, f32::min);
        assert!((peak + 4.).abs() < 0.1, "{peak}");
        let at = (0..400).min_by(|a, b| hop_at(*a as f32 / 1000.).total_cmp(&hop_at(*b as f32 / 1000.))).unwrap();
        assert!((47..=49).contains(&at), "{at} ms");
        for ms in 0..2000 {
            assert!(hop_at(ms as f32 / 1000.) <= 0., "never below rest");
        }
        assert!(hop_at(0.35).abs() < 0.1, "back by ~300 ms");
    }

    /// P5: a roll ends on its value, only while watched at Full; Reduced
    /// and Off are instant; first drawn (history) never rolls.
    #[test]
    fn roll_ends_on_value() {
        let t0 = Instant::now();
        let m = Motion::new(t0);
        let (a, b): (SharedString, SharedString) = ("3".into(), "4".into());
        m.begin_frame(Level::Full, false, t0);
        assert!(m.roll(1, &a, true).is_none(), "first drawn: at rest");
        m.begin_frame(Level::Full, false, t0 + ms(16));
        let (old, x, up) = m.roll(1, &b, true).expect("it rolls");
        assert_eq!((old.as_ref(), x, up), ("3", 1., true));
        m.begin_frame(Level::Full, false, t0 + ms(116));
        let x2 = m.roll(1, &b, true).unwrap().1;
        assert!(x2 < 1. && x2 > 0.);
        m.begin_frame(Level::Full, false, t0 + ms(900));
        assert!(m.roll(1, &b, true).is_none(), "over: the value, exact");
        for level in [Level::Reduced, Level::Off] {
            m.begin_frame(level, false, t0 + ms(1000));
            assert!(m.roll(1, &a, false).is_none(), "{level:?}: instant");
        }
    }

    /// Shady #17 R2-M1: a roll asks for the next tick of the shared 30 Hz
    /// grid, never a display frame: a live fan-out's rolls cost at most 30
    /// app frames a second, however many, on the sweep's own ticks.
    #[test]
    fn a_roll_steps_on_the_30hz_grid() {
        let t0 = Instant::now();
        let fx = Motion::new(t0);
        fx.begin_frame(Level::Full, false, t0 + Duration::from_millis(5));
        assert!(fx.roll(1, &"1 tool".into(), true).is_none(), "first drawn: at rest");
        fx.demand();
        fx.begin_frame(Level::Full, false, t0 + Duration::from_millis(12));
        assert!(fx.roll(1, &"2 tools".into(), true).is_some(), "it changed while drawn: it rolls");
        let (frame, at) = fx.demand();
        assert!(!frame, "no display frame");
        let at = at.expect("its next tick");
        assert_eq!(at.duration_since(t0), DECAY_TICK, "the grid's next tick");
    }

    /// P5: only the changed digit cells of a count roll; a count that
    /// changes length rolls whole.
    #[test]
    fn roll_changed_digits_only() {
        use super::changed_cells as cells;
        assert_eq!(cells("12", "13"), Some((1, 2)), "the 3");
        assert_eq!(cells("+3 more", "+4 more"), Some((1, 2)), "the 4, not `more`");
        assert_eq!(cells("19 calls", "20 calls"), Some((0, 2)));
        assert_eq!(cells("109", "200"), Some((0, 3)));
        assert_eq!(cells("9", "10"), Some((0, 2)), "a longer number rolls whole, not its words");
        assert_eq!(cells("1 tool", "2 tools"), Some((0, 1)), "the 2; the plural swaps in place");
        assert_eq!(cells("9   NEED YOU", "10   NEED YOU"), Some((0, 2)));
        assert_eq!(cells("7", "7"), Some((0, 0)), "nothing rolls");
        assert_eq!(cells("reading a.rs", "grep b"), None, "no number: the whole line");
        // Where the old words start (re-gate #17: `1 tools` never shows).
        assert_eq!(super::old_cells_end("1 tool", "2 tools", 1), Some(1), "` tool` from 1");
        assert_eq!(super::old_cells_end("9 tools", "10 tools", 2), Some(1), "a shorter number: its own end");
        assert_eq!(super::old_cells_end("10 tools", "9 tools", 1), Some(2));
    }

    /// P4 (finish gate #17): the old glyph's light goes out over 80 ms, the
    /// strokes draw from 40 ms over `base`. No frame shows an empty box:
    /// at every millisecond one of the two is lit. The stroke is seen: at
    /// 60 fps, most of its frames are partial, and it is not mostly drawn
    /// in its first 60 ms.
    #[test]
    fn a_write_on_never_shows_an_empty_box() {
        let t0 = Instant::now();
        let m = Motion::new(t0);
        let at = |t: f32| {
            m.begin_frame(Level::Full, false, t0 + Duration::from_secs_f32(t / 1000.));
            m.write_on(Some(t0))
        };
        assert_eq!(at(0.), (1., 0.), "the old glyph, lit; nothing drawn");
        for t in 0..=260 {
            let (old, drawn) = at(t as f32);
            assert!(old > 0. || drawn > 0.1, "{t} ms: an empty box ({old}, {drawn})");
        }
        assert_eq!(at(30.).1, 0., "the strokes wait 40 ms");
        assert!(at(60.).0 > 0., "the old glyph overlaps the first strokes");
        assert!(at(100.).1 < 0.5, "60 ms into the stroke: under half drawn");
        let partial = (0..16).filter(|k| (0.05..0.95).contains(&at(*k as f32 * 1000. / 60.).1)).count();
        assert!(partial >= 8, "at 60 fps, {partial} partial frames");
        assert_eq!(at(250.), (0., 1.), "over");
        m.begin_frame(Level::Reduced, false, t0 + ms(10));
        assert_eq!(m.write_on(Some(t0)), (0., 1.), "Reduced: at once");
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

    /// motion.md §6.4 Flip (fluid.md §2.1 `snap`): a chrome spring started
    /// anywhere settles exactly on its target and asks for nothing more;
    /// a retarget mid-flight keeps where it is.
    #[test]
    fn flip_offsets_settle_to_zero() {
        let t0 = Instant::now();
        let ms = |n: u64| Duration::from_millis(n);
        let m = Motion::new(t0);
        m.begin_frame(Level::Full, false, t0);
        assert_eq!(m.spring(1, None, 100., super::SNAP_RESPONSE), (100., false), "first drawn: at its target");
        m.spring_jump(1, 260., 100., super::SNAP_RESPONSE);
        let mut last = 260.;
        for k in 1..=40 {
            m.begin_frame(Level::Full, false, t0 + ms(7 * k));
            let (v, moving) = m.spring_held(1, 100., super::SNAP_RESPONSE);
            assert!(v <= last + 1e-3 && v >= 100., "critically damped: no overshoot ({last} → {v})");
            last = v;
            if k == 10 {
                assert!(moving && v > 100.5, "mid-flight at {v}");
                // A new target: it starts from where it is.
                let (w, _) = m.spring_held(1, 40., super::SNAP_RESPONSE);
                assert!((w - v).abs() < 1e-3, "a retarget keeps the position: {v} → {w}");
                m.spring_held(1, 100., super::SNAP_RESPONSE);
            }
        }
        m.begin_frame(Level::Full, false, t0 + ms(600));
        assert_eq!(m.spring_held(1, 100., super::SNAP_RESPONSE), (100., false), "settled exactly");
        assert!(!m.demand().0, "asks for no frame");
        // At rest, a new target is taken at once (a resize, a scroll).
        m.begin_frame(Level::Full, false, t0 + ms(610));
        assert_eq!(m.spring_held(1, 140., super::SNAP_RESPONSE), (140., false));
        // Not Full: never moves.
        m.begin_frame(Level::Reduced, false, t0 + ms(620));
        m.spring_jump(1, 0., 140., super::SNAP_RESPONSE);
        assert_eq!(m.spring_held(1, 140., super::SNAP_RESPONSE), (140., false));
    }

    /// fluid.md §3.9: a hover fill comes on over 60 ms and goes off over
    /// 160 (on fast, off decays); Reduced on at once, off over `quick`;
    /// Off at once both ways.
    #[test]
    fn hover_eases_in_fast_and_out_slow() {
        let t0 = Instant::now();
        let ms = |n: u64| Duration::from_millis(n);
        for (level, on, off) in [(Level::Full, 60, 160), (Level::Reduced, 0, 120), (Level::Off, 0, 0)] {
            let m = Motion::new(t0);
            m.begin_frame(level, false, t0);
            m.set_hover(7, true);
            assert_eq!(m.hover(7) == 1., on == 0, "{level:?}: on at once only if instant");
            m.begin_frame(level, false, t0 + ms(on));
            assert_eq!(m.hover(7), 1., "{level:?}: on after {on} ms");
            m.set_hover(7, false);
            if off > 0 {
                m.begin_frame(level, false, t0 + ms(on + off / 2));
                let h = m.hover(7);
                assert!(h > 0. && h < 1., "{level:?}: half way off: {h}");
            }
            m.begin_frame(level, false, t0 + ms(on + off));
            assert_eq!(m.hover(7), 0., "{level:?}: off after {off} ms");
        }
    }

    /// motion.md §6.6: a changed number afterglows in three steps over
    /// `glow` (1, 0.66, 0.33, then the role), three frames rather than 30
    /// a second; first drawn it is at rest; Reduced lit for `quick`; Off never.
    #[test]
    fn a_changed_number_afterglows_in_three_steps() {
        let t0 = Instant::now();
        let ms = |n: u64| Duration::from_millis(n);
        let m = Motion::new(t0);
        m.begin_frame(Level::Full, false, t0);
        assert_eq!(m.glow_k(3, 10), 0., "first drawn: at rest");
        m.begin_frame(Level::Full, false, t0 + ms(10));
        assert_eq!(m.glow_k(3, 11), 1., "changed: lit");
        assert_eq!(m.demand().1, Some(t0 + ms(170)), "its next step is a timer, not a frame");
        for (at, k) in [(100, 1.), (170, 1. - 1. / 3.), (330, 1. - 2. / 3.), (490, 0.)] {
            m.begin_frame(Level::Full, false, t0 + ms(at));
            assert!((m.glow_k(3, 11) - k).abs() < 1e-5, "{at} ms: {k}");
        }
        let m = Motion::new(t0);
        m.begin_frame(Level::Reduced, false, t0);
        m.glow_k(3, 10);
        m.begin_frame(Level::Reduced, false, t0 + ms(10));
        assert_eq!(m.glow_k(3, 11), 1.);
        m.begin_frame(Level::Reduced, false, t0 + ms(130));
        assert_eq!(m.glow_k(3, 11), 0., "Reduced: lit for quick");
        let m = Motion::new(t0);
        m.begin_frame(Level::Off, false, t0);
        m.glow_k(3, 10);
        m.begin_frame(Level::Off, false, t0 + ms(10));
        assert_eq!(m.glow_k(3, 11), 0., "Off: never");
    }
}
