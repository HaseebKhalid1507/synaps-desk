//! H2 · Turning lattice (indicators/proposal.md §6, Haseeb's pick): the
//! one "live" family across the cards, the composer's state cell, the
//! chips, the rail and the tabs. A 3×3 grid of 2 px dots on whole device
//! pixels; each state is a pattern of levels. Running, one lit face of the
//! ring (and a trailing dot, the core at 74 %) advances a quarter turn
//! every [`STEP_BEATS`] beats through a 45° corner pose held one beat;
//! streaming types its bottom row a dot a beat; reconnecting chases the
//! ring. A turn of the grid is a shift of the ring's slots, so every pose,
//! the state-change turn's too, is whole dots: nothing is rotated.
//!
//! Every ambient pose is a pure function of the shared beat (the beat
//! layer's grid, [`crate::motion::BEAT`]): an indicator repaints only when
//! its pose changes (running 4 a second, streaming and reconnecting 8),
//! phase-locked app-wide, and idle has none. Reduced, Off, low power and
//! history: each state's still pose.

use gpui_kit::*;

/// A pose: each dot's level, row-major (`levels`' index; 0: not drawn).
pub(crate) type Pose = [u8; 9];

/// The levels of `mark` over the ground (§6: 0.20 unlit, 0.45, 0.74, 1.0).
const LEVELS: [f32; 5] = [0., 0.2, 0.45, 0.74, 1.];
const UN: u8 = 1;
const LOW: u8 = 2;
const MID: u8 = 3;
const LIT: u8 = 4;

/// The ring: the 8 outer dots, clockwise from the top-left.
pub(crate) const RING: [usize; 8] = [0, 1, 2, 5, 8, 7, 6, 3];

/// `LATTICE_STEP` (500 ms): a running face advances a quarter every 4
/// beats (a 2 s lap); its 45° corner pose holds the step's first beat
/// (125 ms; the proposal's 133 is a 30 Hz grid's 4 ticks: one beat keeps
/// every pose on the one beat grid).
pub(crate) const STEP_BEATS: u32 = 4;

/// What an indicator shows ([`pose`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Pat {
    /// Work going on (a call, a worker, a busy session).
    Run,
    /// A call's output (or the answer) being written: 16 px only.
    Stream,
    /// Waiting its turn: the core at 74 %, `dim`.
    Queued,
    /// Needs you: the plus, `acc`.
    Needs,
    /// On its way (opening, reconnecting): a head chasing the ring, `dim`.
    Reconnect,
}

impl Pat {
    /// It moves on the beat (the beat layer paints it).
    pub(crate) fn ambient(self) -> bool {
        matches!(self, Pat::Run | Pat::Stream | Pat::Reconnect)
    }
}

/// `p` turned `slots` eighths of a turn (45° each) clockwise: the ring
/// shifted, the centre fixed.
pub(crate) fn turn(p: Pose, slots: i32) -> Pose {
    let mut out = p;
    for (i, &at) in RING.iter().enumerate() {
        out[RING[(i as i32 + slots).rem_euclid(8) as usize]] = p[at];
    }
    out
}

/// `pat`'s pose at `beat` (the shared grid's); `None`: its still pose.
pub(crate) fn pose(pat: Pat, beat: Option<u32>) -> Pose {
    match pat {
        Pat::Run => {
            // The face's first slot: on a step's first beat the 45° pose
            // (one slot on), then the landing (two), held.
            let base = beat.map_or(1, |b| {
                let k = (b / STEP_BEATS) as i32;
                if b.is_multiple_of(STEP_BEATS) { 2 * k - 1 } else { 2 * k }
            });
            let mut p = [UN; 9];
            p[4] = MID;
            p[RING[(base - 1).rem_euclid(8) as usize]] = LOW;
            for j in 0..3 {
                p[RING[(base + j).rem_euclid(8) as usize]] = LIT;
            }
            p
        }
        Pat::Stream => {
            let typed = beat.map_or(1, |b| b % 4) as usize;
            let mut p = [LOW, LOW, LOW, MID, MID, MID, UN, UN, UN];
            for c in 0..typed.min(3) {
                p[6 + c] = LIT;
            }
            p
        }
        Pat::Queued => [UN, UN, UN, UN, MID, UN, UN, UN, UN],
        Pat::Needs => [UN, LIT, UN, LIT, LIT, LIT, UN, LIT, UN],
        Pat::Reconnect => {
            let head = beat.unwrap_or(0) % 8;
            let mut p = [UN; 9];
            for (i, l) in [LIT, MID, LOW].into_iter().enumerate() {
                p[RING[(head as i32 - i as i32).rem_euclid(8) as usize]] = l;
            }
            p
        }
    }
}

/// The next beat after `beat` on which `pat`'s pose changes (none: it
/// holds): what the beat layer arms its one timer to.
pub(crate) fn next_change(pat: Pat, beat: u32) -> Option<u32> {
    match pat {
        Pat::Run if beat.is_multiple_of(STEP_BEATS) => Some(beat + 1),
        Pat::Run => Some((beat / STEP_BEATS + 1) * STEP_BEATS),
        Pat::Stream | Pat::Reconnect => Some(beat + 1),
        _ => None,
    }
}

/// A pose as numbers to draw: each dot's strength (0..1 of `mark` over the
/// ground; 0: not drawn).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Dots {
    pub(crate) a: [f32; 9],
}

impl Dots {
    pub(crate) fn of(p: Pose) -> Dots {
        Dots { a: p.map(|l| LEVELS[l.min(LIT) as usize]) }
    }

    /// Every dot at `k` of its strength (a turn's last pose at 74 %).
    pub(crate) fn scaled(mut self, k: f32) -> Dots {
        self.a = self.a.map(|a| a * k);
        self
    }

    /// Nothing drawn.
    pub(crate) fn none() -> Dots {
        Dots { a: [0.; 9] }
    }
}

/// A pose's code (3 bits a dot): its debug selector's number.
pub(crate) fn code(p: Pose) -> u32 {
    p.iter().fold(0, |c, &l| c << 3 | u32::from(l))
}

/// Where the dots sit in an `n` px box (16: cards, chips, the composer's
/// cell; 10: the rail and the tabs).
fn grid(n: f32) -> [f32; 3] {
    if n >= 16. { [3., 7., 11.] } else { [1., 4., 7.] }
}

/// Paints `d` in the `n` px box at `origin`, `mark` over `ground`: one quad
/// a lit dot, origins and sizes on device pixels (a 2 px dot stays one
/// solid square at 1.25× or 1.5×). The app's frame and the beat layer
/// paint the same.
pub(crate) fn paint(window: &mut Window, origin: Point<Pixels>, n: f32, d: Dots, mark: Hsla, ground: Hsla) {
    let s = window.scale_factor();
    let snap = |v: f32| (v * s).round() / s;
    let dot = snap(2.).max(1. / s);
    let o = grid(n);
    let (ox, oy) = (f32::from(origin.x), f32::from(origin.y));
    for (i, &a) in d.a.iter().enumerate() {
        if a <= 0.004 {
            continue;
        }
        let (x, y) = (o[i % 3], o[i / 3]);
        let c = crate::motion::mix(ground, mark, a.min(1.));
        window.paint_quad(fill(Bounds::new(point(px(snap(ox + x)), px(snap(oy + y))), size(px(dot), px(dot))), c));
    }
}

/// `d` as an element: the `n` px box painting it ([`paint`]), selected as
/// `lattice-{sel:07x}`.
pub(crate) fn el(n: f32, d: Dots, sel: u32, mark: Hsla, ground: Hsla) -> Div {
    div()
        .relative()
        .flex_none()
        .size(px(n))
        .debug_selector(move || format!("lattice-{sel:07x}"))
        .child(canvas(|_, _, _| {}, move |b, _, window, _| paint(window, b.origin, n, d, mark, ground)).absolute().inset_0())
}

/// H2's state-change turn (§6, `TURN_ON_CHANGE`), `t` into it at Full:
/// the old pattern turned +45° for [`TURN`], then the new one arriving
/// from −45° for another; onto a mark (`new` none) the old one turned +90°
/// at 74 %. `None`: the turn is over.
pub(crate) fn turning(old: Pose, new: Option<Pose>, t: std::time::Duration) -> Option<Dots> {
    if t < TURN {
        Some(Dots::of(turn(old, 1)))
    } else if t < 2 * TURN {
        Some(match new {
            Some(n) => Dots::of(turn(n, -1)),
            None => Dots::of(turn(old, 2)).scaled(0.74),
        })
    } else {
        None
    }
}

/// `LATTICE_TURN`: each of a state change's two poses (66 ms: 2 ticks at
/// 30 Hz, 1 at 15).
pub(crate) const TURN: std::time::Duration = std::time::Duration::from_millis(66);

/// After a turn onto a mark (§6): the old dots, turned +90° at 74 %, go out
/// diagonal by diagonal (`quick` each, 30 ms apart), `t` after the turn.
pub(crate) fn going_out(old: Pose, t: std::time::Duration, level: crate::motion::Level) -> Option<Dots> {
    if level != crate::motion::Level::Full {
        return None;
    }
    let mut d = Dots::of(turn(old, 2)).scaled(0.74);
    let mut any = false;
    for (i, a) in d.a.iter_mut().enumerate() {
        let start = crate::motion::STAGGER * ((i / 3 + i % 3) as u32);
        let k = t.saturating_sub(start).as_secs_f32() / crate::motion::QUICK.as_secs_f32();
        *a *= 1. - k.min(1.);
        any |= *a > 0.004;
    }
    any.then_some(d)
}

#[cfg(test)]
mod tests {
    use super::{LIT, Pat, RING, code, next_change, pose, turn};

    /// A turn is a ring shift: two slots are 90°, eight a whole turn, the
    /// centre fixed; nothing is rotated off the grid.
    #[test]
    fn a_turn_is_a_ring_shift() {
        let p = pose(Pat::Needs, None);
        assert_eq!(turn(turn(p, 1), -1), p);
        assert_eq!(turn(p, 8), p);
        assert_eq!(turn(p, 2), p, "the plus is its own quarter turn");
        assert_ne!(turn(p, 1), p, "and an × at 45°");
        for k in 0..8 {
            assert_eq!(turn(pose(Pat::Run, Some(0)), k)[4], pose(Pat::Run, Some(0))[4], "the centre holds");
        }
        let run = pose(Pat::Run, Some(1));
        assert_eq!(pose(Pat::Run, Some(5)), turn(run, 2), "a step is a quarter turn");
        assert_eq!(pose(Pat::Run, Some(4)), turn(run, 1), "through its 45° corner pose");
        let _ = RING;
    }

    /// Running steps a quarter every 4 beats through one corner beat: its
    /// pose changes twice a step (4 a second), streaming and reconnecting
    /// every beat, the rest never.
    #[test]
    fn a_running_pose_changes_only_at_its_steps() {
        let mut changes = 0;
        for b in 0..32u32 {
            let next = next_change(Pat::Run, b).expect("running moves");
            for c in b + 1..next {
                assert_eq!(pose(Pat::Run, Some(c)), pose(Pat::Run, Some(b)), "held from {b} to {next}");
            }
            assert_ne!(pose(Pat::Run, Some(next)), pose(Pat::Run, Some(b)), "{b} → {next}");
            changes += usize::from(pose(Pat::Run, Some(b + 1)) != pose(Pat::Run, Some(b)));
        }
        assert_eq!(changes, 16, "2 changes a 4-beat step: 4 a second");
        for b in 0..16 {
            assert_eq!(next_change(Pat::Stream, b), Some(b + 1));
            assert_eq!(next_change(Pat::Reconnect, b), Some(b + 1));
        }
        for p in [Pat::Queued, Pat::Needs] {
            assert!(!p.ambient() && next_change(p, 3).is_none(), "{p:?} holds");
        }
    }

    /// Each state's still pose is its own shape: never colour alone.
    #[test]
    fn every_lattice_still_pose_is_unique() {
        let all = [Pat::Run, Pat::Stream, Pat::Queued, Pat::Needs, Pat::Reconnect];
        let codes: std::collections::HashSet<u32> = all.iter().map(|&p| code(pose(p, None))).collect();
        assert_eq!(codes.len(), all.len());
        assert_eq!(pose(Pat::Run, None).iter().filter(|&&l| l == LIT).count(), 3, "running: one lit face");
        // Every live pose fills the grid: the beat layer's paint covers the
        // frame's still pose under it.
        for p in [Pat::Run, Pat::Stream, Pat::Reconnect] {
            for b in 0..16 {
                assert!(pose(p, Some(b)).iter().all(|&l| l > 0), "{p:?} at {b}");
            }
        }
    }
}
