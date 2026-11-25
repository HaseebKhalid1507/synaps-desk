//! Colour: the Synaps TUI's theme roles, as its themes define them
//! ([`crate::palettes`], the 20 static palettes), each resolved once for the
//! surface it sits on and floored for contrast with the TUI's own
//! `lift_to_contrast` (mixed toward `input_fg` in 1 % steps): text 4.5:1,
//! glyphs, bars and state borders 3:1, the user and code surfaces 1.10:1
//! off the canvas. Which role goes where is `transcript-design/spec.md`
//! section 2 and the rail recommendation's section 3.
//!
//! The theme, first valid wins: `--theme`, `$SYNAPS_THEME`, the settings
//! file's `theme` (written by the in-app picker), `theme =` in the Synaps
//! config (`<base>[/<profile>]/config`), then nord. A `<base>/themes/<name>`
//! file overrides a built-in, as in the TUI.
//!
//! The accent is an optional override of `prompt_fg` and `border_active`
//! only (the user's bar and label, SEND, the on-screen bar in the rail,
//! focus and selection); it never colours a state. Sources: `--accent`,
//! `$SYNAPS_DESKTOP_ACCENT`, the settings file's `accent` (the key the
//! accent-only picker wrote, so an accent chosen before carries over as the
//! override).
//!
//! Everything is resolved into a GPUI [`Global`] ([`Scheme`]) when the
//! theme, accent or text size changes, never per frame: `render` copies the
//! [`Palette`] and [`Metrics`] out of it, nothing allocated.

use std::path::{Path, PathBuf};

use gpui_kit::base::{TextViewDefaults, TextViewStyle};
use gpui_kit::component::Theme;
use gpui_kit::{App, FontWeight, Global, HighlightStyle, Hsla, Rems, Rgba, StyleRefinement, Styled as _, px, rgb};

use crate::fonts::TextPx as _;
use crate::palettes::{self, Raw};
use crate::rail::Bucket;

pub const ENV_ACCENT: &str = "SYNAPS_DESKTOP_ACCENT";
/// The TUI's own per-process theme variable.
pub const ENV_THEME: &str = "SYNAPS_THEME";
pub const DEFAULT_THEME: &str = "nord";

/// The accent picker's presets: (tracked label, colour).
pub const PRESETS: &[(&str, u32)] = &[
    ("A R A S A K A", 0xe8132f),
    ("M I L I T E C H", 0xf4b400),
    ("K A N G   T A O", 0xcfa85a),
    ("N E T W A T C H", 0x2d7bff),
    ("K I R O S H I", 0x35f2ff),
    ("V O O D O O", 0x2bd47d),
];

/// The centre pane's text sizes (body px); Ctrl+= / Ctrl+- step through
/// them, Ctrl+0 goes back to [`DEFAULT_TEXT`].
pub const TEXT_STEPS: [u8; 7] = [14, 15, 16, 17, 18, 20, 22];
pub const DEFAULT_TEXT: u8 = 16;

/// Text: WCAG AA on the surface it sits on.
pub const TEXT: f64 = 4.5;
/// Glyphs, bars and state borders (WCAG 1.4.11).
pub const MARK: f64 = 3.0;
/// The authorize card's command on its well (polish.md §2.6, the finish
/// gate of #18): at least this, and at least the card's other text.
pub const CMD: f64 = 7.0;
/// The user and code surfaces stand at least this far off the canvas.
pub const SURFACE: f64 = 1.10;
/// A step up a surface (`raised` off the chrome, `card_hi` off the card).
pub const STEP: f64 = 1.14;
/// A region divider (`hybrid.html` `--line`).
pub const HAIR: f64 = 1.14;
/// A control's outline (`--line2`).
pub const EDGE: f64 = 1.27;
/// An overlay's outline and a control's on a card (`hybrid.html` `--line3`).
pub const FRAME: f64 = 1.56;
/// The overlay box's `card` off the chrome (design.md §1.4): two steps,
/// so its `card_hi` row and its `frame` both still read on it.
pub const CARD: f64 = 1.32;

/// Cards D (cards4/proposal.md §1): a card's hover and its well at least
/// this off it.
pub const D_CARD: f64 = 1.06;
/// `card_d` off the canvas: the card has no outline (Haseeb, 07:18), so it
/// reads by its fill alone, a [`SURFACE`] off the canvas as `user` and
/// `code` do (it was 1.06, read with a `line` hairline).
pub const D_FILL: f64 = SURFACE;
/// The step a card's hover is drawn at, and the well's.
pub const D_STEP: f64 = 1.08;
pub const D_WELL: f64 = 1.07;
/// `line` (an opened run's rule, a ghost button's outline) on the card,
/// the canvas and the hover.
pub const D_LINE: f64 = 1.25;
/// The authorize card's accent edge on the card.
pub const D_ACC_LINE: f64 = 1.4;
/// The washes (§1, §8): `motion::mix(state, card_d, t)`, one quad behind a
/// row. A failed row rests at 7.5 % `err` from an 18 % flush; a waiting
/// row at 6 % of the accent from 16 %; the Ask pill's fill is 12 %.
pub const D_FAIL_REST: f32 = 0.075;
pub const D_FAIL_PEAK: f32 = 0.18;
pub const D_WAIT_REST: f32 = 0.06;
pub const D_WAIT_PEAK: f32 = 0.16;
pub const D_SOFT: f32 = 0.12;
/// The authorize card's flush as it arrives (§8 #8) and the outlined
/// Allow's pressed fill: of the accent off the card.
pub const D_ASK_FLUSH: f32 = 0.12;
pub const D_OUT_PRESS: f32 = 0.18;
/// What each wash carries ([`d_washes`]): every text a failed row draws,
/// a waiting row's, the authorize card's, the outlined Allow's label.
pub fn d_wash_texts(r: &Roles) -> [Vec<u32>; 4] {
    [
        vec![r.prose_t, r.dim_t, r.arg, r.ok, r.err, r.ink],
        vec![r.prose_t, r.dim_t, r.arg, r.acc],
        vec![r.dim_t, r.ink, r.prose_t, r.prose],
        vec![r.acc],
    ]
}

/// Cards D's washes from the floored `err` and `acc` (Shady #23 M3): each
/// the proposal's strength, or the strongest (in 0.5 % steps, down to its
/// rest) where every text drawn on it stays ≥ [`TEXT`].
pub fn d_washes(r: &Roles) -> [u32; 6] {
    let [fail, wait, ask, out] = d_wash_texts(r);
    // The strength each lands on: a peak goes no lower than its rest did.
    let k = |from: f32, floor: f32, toward: u32, texts: &[u32]| {
        let reads = |k: f32| texts.iter().all(|&c| contrast(c, mix(r.card_d, toward, f64::from(k))) >= TEXT);
        let mut k = from;
        while k > floor + 1e-6 && !reads(k) {
            k = (k - 0.005).max(floor);
        }
        k
    };
    let (fail_rest, wait_rest) = (k(D_FAIL_REST, 0., r.err, &fail), k(D_WAIT_REST, 0., r.acc, &wait));
    [
        (r.err, fail_rest),
        (r.err, k(D_FAIL_PEAK, fail_rest, r.err, &fail)),
        (r.acc, wait_rest),
        (r.acc, k(D_WAIT_PEAK, wait_rest, r.acc, &wait)),
        (r.acc, k(D_ASK_FLUSH, 0., r.acc, &ask)),
        (r.acc, k(D_OUT_PRESS, 0., r.acc, &out)),
    ]
    .map(|(toward, k)| mix(r.card_d, toward, f64::from(k)))
}
/// The Ask pill stands at least this off its card.
pub const D_SOFT_OFF: f64 = 1.03;

/// A worker chip's fill (`card_agent`, Haseeb: a worker, not a tool, at a
/// glance): `card_d` turned this far (OKLab) toward the worker's hue at
/// `card_d`'s own luminance, so it reads as another material, never a step
/// or a state; as little as [`D_AGENT_MIN`] where the gamut is short. Its
/// hover (`hover_agent`) is `hover` turned the same way.
pub const D_AGENT: f64 = 0.03;
pub const D_AGENT_MIN: f64 = 0.02;
/// The worker's hue where the theme's worker colours have none, or none
/// that turns its card in gamut ([`agent_sources`]): violet, else teal.
pub const AGENT_HUES: [(&str, f64); 2] = [("violet", 305.), ("teal", 200.)];
/// Under this chroma (OKLab) a colour has no hue to take or to keep off.
const HUE_CHROMA: f64 = 0.04;

/// `#rrggbb` / `rrggbb` / `#rgb` / `rgb`, any case, surrounding space ignored.
pub fn parse_hex(s: &str) -> Option<u32> {
    let s = s.trim();
    let s = s.strip_prefix('#').unwrap_or(s);
    if !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match s.len() {
        6 => u32::from_str_radix(s, 16).ok(),
        3 => {
            let v = u32::from_str_radix(s, 16).ok()?;
            let (r, g, b) = ((v >> 8) & 0xf, (v >> 4) & 0xf, v & 0xf);
            Some(((r * 0x11) << 16) | ((g * 0x11) << 8) | (b * 0x11))
        }
        _ => None,
    }
}

pub fn to_hex(c: u32) -> String {
    format!("#{:06x}", c & 0xff_ffff)
}

// ── contrast, as the TUI computes it (theme/mod.rs) ───────────────────────

fn channels(c: u32) -> [u8; 3] {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8]
}

/// WCAG 2 relative luminance.
pub fn luminance(c: u32) -> f64 {
    let lin = |v: u8| {
        let v = f64::from(v) / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    let [r, g, b] = channels(c);
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// WCAG 2 contrast ratio (1..=21).
pub fn contrast(a: u32, b: u32) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `b` over `a` at `t` (0: `a`, 1: `b`), opaque: a state's colour between
/// two roles (never an opacity, design.md §5).
pub fn mix_hsla(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (Rgba::from(a), Rgba::from(b));
    let m = |x: f32, y: f32| x + (y - x) * t;
    Hsla::from(Rgba { r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b), a: 1. })
}

/// `a` mixed toward `b` by `t` in sRGB (the TUI's `mix_rgb`).
pub fn mix(a: u32, b: u32, t: f64) -> u32 {
    let (a, b) = (channels(a), channels(b));
    let m = |x: u8, y: u8| (f64::from(x) + (f64::from(y) - f64::from(x)) * t).round() as u32;
    (m(a[0], b[0]) << 16) | (m(a[1], b[1]) << 8) | m(a[2], b[2])
}

/// The TUI's `lift_to_contrast`, on every ground at once: `from` mixed
/// toward `toward` in 1 % steps just until it reads at `target` on all of
/// `grounds`; `toward` when nothing short of it does.
pub fn lift(from: u32, toward: u32, grounds: &[u32], target: f64) -> u32 {
    (0..=100u8)
        .map(|s| mix(from, toward, f64::from(s) / 100.0))
        .find(|&c| grounds.iter().all(|&g| contrast(c, g) >= target))
        .unwrap_or(toward)
}

/// The TUI's `tool_panel` / `raised_surface`: `ground` stepped toward `toward`
/// (0.5 % steps, at most 40 %) until it stands `target` off itself.
pub fn step(ground: u32, toward: u32, target: f64) -> u32 {
    step_upto(ground, toward, target, 80)
}

/// How far a pressed fill stands off both its hover fill and its rest
/// fill, at least (a contrast ratio): a press is a state of its own, never
/// "the hover went off" (fluid.md §3.9). 1.05 is about a third of a
/// [`STEP`], the least two flat fills side by side are told apart at.
pub const PRESS_MIN: f64 = 1.05;

/// A pressed fill (fluid.md §3.9): `hover` (which is `rest` one [`STEP`]
/// toward the ink) stepped on, to the ink's side of it (toward white or
/// black: a fill as light as the ink still has room), until it stands
/// [`STEP`] off it, no further than every one of `texts` still reads on it
/// at [`TEXT`] and every one of `marks` at [`MARK`]. Where that way is
/// blocked short of [`PRESS_MIN`] (texts floored onto the hover fill
/// itself), it steps back toward the rest fill and stops halfway, where it
/// stands farthest off both. Either way at least [`PRESS_MIN`] off each
/// (`press_is_visible_in_every_theme`): never the hover, never the rest.
pub fn pressed(rest: u32, hover: u32, toward: u32, texts: &[u32], marks: &[u32]) -> u32 {
    let ok = |c: u32| texts.iter().all(|&t| contrast(t, c) >= TEXT) && marks.iter().all(|&m| contrast(m, c) >= MARK);
    // How far `c` stands off the nearer of the two.
    let apart = |c: u32| contrast(c, hover).min(contrast(c, rest));
    let side = if luminance(toward) > luminance(hover) { 0xffffff } else { 0x000000 };
    let mut near = hover;
    for i in 1..=200u8 {
        let c = mix(hover, side, f64::from(i) * 0.005);
        if !ok(c) {
            break;
        }
        near = c;
        if contrast(c, hover) >= STEP {
            break;
        }
    }
    if apart(near) >= PRESS_MIN {
        return near;
    }
    // Back toward rest (away from the ink): the farthest from both.
    let away = side ^ 0xffffff;
    let mut far = hover;
    for i in 1..=80u8 {
        let c = mix(hover, away, f64::from(i) * 0.005);
        if !ok(c) || apart(c) < apart(far) {
            break;
        }
        far = c;
    }
    if apart(far) > apart(near) { far } else { near }
}

/// [`step`], at most `steps` half-percent steps toward `toward`.
fn step_upto(ground: u32, toward: u32, target: f64, steps: u8) -> u32 {
    let mut out = ground;
    for i in 1..=steps {
        out = mix(ground, toward, f64::from(i) * 0.005);
        if contrast(out, ground) >= target {
            break;
        }
    }
    out
}

// ── a worker's fill (OKLab) ──────────────────────────────────────────────

fn lin8(v: u8) -> f64 {
    let v = f64::from(v) / 255.0;
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn cbrt(x: f64) -> f64 {
    x.abs().powf(1.0 / 3.0).copysign(x)
}

/// `c` in OKLab (L, a, b).
pub fn oklab(c: u32) -> [f64; 3] {
    let [r, g, b] = channels(c).map(lin8);
    let l = cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
    let m = cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
    let s = cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}

/// OKLab to linear sRGB (unclamped).
fn from_oklab(l: f64, a: f64, b: f64) -> [f64; 3] {
    let (x, y, z) =
        (l + 0.3963377774 * a + 0.2158037573 * b, l - 0.1055613458 * a - 0.0638541728 * b, l - 0.0894841775 * a - 1.2914855480 * b);
    let (x, y, z) = (x * x * x, y * y * y, z * z * z);
    [
        4.0767416621 * x - 3.3077115913 * y + 0.2309699292 * z,
        -1.2684380046 * x + 2.6097574011 * y - 0.3413193965 * z,
        -0.0041960863 * x - 0.7034186147 * y + 1.7076147010 * z,
    ]
}

/// Linear sRGB in 8 bits; `None` out of gamut.
fn srgb8(c: [f64; 3]) -> Option<u32> {
    if c.iter().any(|v| !(-1e-6..=1.0 + 1e-6).contains(v)) {
        return None;
    }
    let enc = |v: f64| {
        let v = v.clamp(0., 1.);
        let e = if v <= 0.0031308 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (e * 255.0 + 0.5).floor() as u32
    };
    Some((enc(c[0]) << 16) | (enc(c[1]) << 8) | enc(c[2]))
}

/// How far apart two colours are (OKLab ΔE).
#[cfg(test)]
pub fn ok_distance(x: u32, y: u32) -> f64 {
    let (p, q) = (oklab(x), oklab(y));
    ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt()
}

/// `c`'s hue (degrees, OKLab) and chroma.
pub fn hue(c: u32) -> (f64, f64) {
    let [_, a, b] = oklab(c);
    (b.atan2(a).to_degrees().rem_euclid(360.), a.hypot(b))
}

/// `ground` turned `k` (OKLab) toward hue `h` at its own luminance: in 8
/// bits, the nearest that lies between `ground`'s luminance and `other`'s
/// (so whatever reads on both reads on it). `None` out of gamut.
fn tint(ground: u32, other: u32, h: f64, k: f64) -> Option<u32> {
    let [_, a0, b0] = oklab(ground);
    let (a, b) = (a0 + k * h.to_radians().cos(), b0 + k * h.to_radians().sin());
    let (y, y_other) = (luminance(ground), luminance(other));
    let (lo, hi) = (y.min(y_other), y.max(y_other));
    let lum_at = |l: f64| {
        let [r, g, b] = from_oklab(l, a, b);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    };
    let (mut x0, mut x1) = (0.0f64, 1.0f64);
    for _ in 0..50 {
        let m = (x0 + x1) / 2.;
        if lum_at(m) < y {
            x0 = m;
        } else {
            x1 = m;
        }
    }
    let (mut l, step) = (x1, if y_other > y { 0.0005 } else { -0.0005 });
    for _ in 0..100 {
        let c = srgb8(from_oklab(l, a, b))?;
        if (lo..=hi).contains(&luminance(c)) {
            return Some(c);
        }
        l += step;
    }
    None
}

/// [`tint`] by [`D_AGENT`], or as much less (to [`D_AGENT_MIN`]) as the
/// gamut allows.
fn agent_tint(ground: u32, other: u32, h: f64) -> Option<u32> {
    (0..=10u8).map(|i| D_AGENT - f64::from(i) * 0.001).take_while(|&k| k >= D_AGENT_MIN - 1e-9).find_map(|k| tint(ground, other, h, k))
}

/// The theme's own worker colours, the ones the TUI paints a subagent
/// with (Haseeb: the worker's colour is its theme's), in order: its name's,
/// then its tool's.
pub fn agent_sources(t: &Raw) -> [(&'static str, Option<u32>); 2] {
    [("subagent_name", Some(t.subagent_name)), ("tool_subagent", t.tool_subagent)]
}

/// A worker chip's fill and its hover (Haseeb: a worker, not a tool), and
/// the role its hue came from: `card_d` and `hover` turned toward the
/// theme's worker colour ([`agent_sources`]: the first with a hue, chroma
/// [`HUE_CHROMA`] or more, that turns both in gamut), each at its own
/// luminance, whatever the accent's hue or the error colour's (the theme's
/// own worker colour, as its TUI draws it). Where none does, violet, else
/// teal ([`AGENT_HUES`]); where neither does, the card's own.
pub fn agent(card_d: u32, hover: u32, t: &Raw) -> (u32, u32, &'static str) {
    let own = agent_sources(t).into_iter().filter_map(|(name, c)| c.map(hue).filter(|h| h.1 >= HUE_CHROMA).map(|h| (name, h.0)));
    own.chain(AGENT_HUES)
        .find_map(|(name, h)| Some((agent_tint(card_d, hover, h)?, agent_tint(hover, card_d, h)?, name)))
        .unwrap_or((card_d, hover, "card_d"))
}

// ── which theme ───────────────────────────────────────────────────────────

/// Where the theme or the accent came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Flag,
    Env,
    Settings,
    /// `theme =` in the Synaps config.
    Config,
    Default,
}

/// A theme name as the TUI takes one (`theme_env_name`): 1 to 64 ASCII
/// letters, digits, `-`, `_` or `.`, not starting with `.`, so it never
/// reaches outside `themes/`.
pub fn plain_name(s: &str) -> Option<&str> {
    let s = s.trim();
    let ok = !s.is_empty() && s.len() <= 64 && !s.starts_with('.') && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b));
    ok.then_some(s)
}

/// A built-in by its exact name (`Theme::builtin`: `night-city`, never
/// `night_city`).
pub fn builtin(name: &str) -> Option<(&'static str, Raw)> {
    palettes::THEMES.iter().find(|(n, _)| *n == name).copied()
}

/// A TUI theme file: `key = #hex` lines over the default palette; unknown
/// keys and bad values are skipped (`Theme::load_from`).
pub fn parse_theme_file(text: &str) -> Raw {
    let mut raw = palettes::THEMES[0].1;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        if let Some((k, v)) = line.split_once('=')
            && let Some(c) = parse_hex(v.trim().trim_matches('"').trim_matches('\''))
        {
            palettes::set(&mut raw, k.trim(), c);
        }
    }
    raw
}

/// The theme a name stands for: `<base>/themes/<name>`, else a built-in.
pub fn lookup(name: &str, base: Option<&Path>) -> Option<Raw> {
    let name = plain_name(name)?;
    if let Some(text) = base.and_then(|b| std::fs::read_to_string(b.join("themes").join(name)).ok()) {
        return Some(parse_theme_file(&text));
    }
    builtin(name).map(|(_, raw)| raw)
}

/// What [`synaps_theme`] calls the palette of a `theme` file.
pub const THEME_FILE: &str = "theme file";

/// The theme a Synaps config names, read as the TUI reads it
/// (`load_theme_from_config`): the first `theme = <name>` line (trimmed,
/// `#` lines skipped) whose value, only trimmed, `resolve` finds; no quote
/// or comment stripping, so `theme = "nord"` names no theme there either.
/// Also the values it skipped, to report.
pub fn config_theme(config: &str, resolve: impl Fn(&str) -> Option<Raw>) -> (Option<(String, Raw)>, Vec<String>) {
    let mut skipped = Vec::new();
    for line in config.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let Some((_, val)) = line.split_once('=').filter(|(k, _)| k.trim() == "theme") else { continue };
        let name = val.trim();
        match resolve(name) {
            Some(raw) => return (Some((name.to_string(), raw)), skipped),
            None if !name.is_empty() => {
                skipped.push(format!("config theme: '{}' is not a theme; skipped", name.chars().take(40).collect::<String>()));
            }
            None => {}
        }
    }
    (None, skipped)
}

/// The file `name` this desktop's profile reads: `<base>/<profile>/<name>`
/// when there is one, else `<base>/<name>` (agent-core `resolve_read_path`).
pub fn read_path(base: &Path, profile: Option<&str>, name: &str) -> PathBuf {
    profile.filter(|p| plain_name(p).is_some()).map(|p| base.join(p).join(name)).filter(|p| p.exists()).unwrap_or_else(|| base.join(name))
}

/// The theme the Synaps TUI shows for this profile after `$SYNAPS_THEME`,
/// found as it finds it: a `theme` file (`<base>[/<profile>]/theme`, a
/// palette over everything else), else [`config_theme`] of the config
/// (names: `<base>/themes/<name>`, else a built-in). Read when called.
pub fn synaps_theme(base: &Path, profile: Option<&str>) -> (Option<(String, Raw)>, Vec<String>) {
    let file = read_path(base, profile, "theme");
    if file.exists() {
        return (Some((THEME_FILE.to_string(), parse_theme_file(&std::fs::read_to_string(&file).unwrap_or_default()))), Vec::new());
    }
    let config = std::fs::read_to_string(read_path(base, profile, "config")).unwrap_or_default();
    config_theme(&config, |name| lookup(name, Some(base)))
}

/// The first of `flag`, `env`, `settings` that names a theme, else the
/// Synaps config's ([`synaps_theme`]), else nord. Each one that names none
/// is reported.
pub fn resolve_theme(
    flag: Option<&str>,
    env: Option<&str>,
    settings: Option<&str>,
    config: Option<(String, Raw)>,
    base: Option<&Path>,
) -> (String, Raw, Source, Vec<String>) {
    let mut warnings = Vec::new();
    for (v, src, what) in [(flag, Source::Flag, "--theme"), (env, Source::Env, ENV_THEME), (settings, Source::Settings, "settings theme")] {
        let Some(v) = v else { continue };
        match lookup(v, base) {
            Some(raw) => return (v.trim().to_string(), raw, src, warnings),
            None => warnings.push(format!("{what}: '{}' is not a theme; ignored", v.chars().take(40).collect::<String>())),
        }
    }
    if let Some((name, raw)) = config {
        return (name, raw, Source::Config, warnings);
    }
    let (name, raw) = builtin(DEFAULT_THEME).expect("nord is built in");
    (name.to_string(), raw, Source::Default, warnings)
}

/// The theme when the desktop follows the Synaps config (no `--theme`, no
/// saved choice): `$SYNAPS_THEME`, else [`synaps_theme`], else nord.
pub fn follow_config(env: Option<&str>, base: &Path, profile: Option<&str>) -> (String, Raw, Source, Vec<String>) {
    let (config, mut skipped) = synaps_theme(base, profile);
    let (name, raw, src, warnings) = resolve_theme(None, env, None, config, Some(base));
    skipped.extend(warnings);
    (name, raw, src, skipped)
}

/// The first valid of `flag`, `env`, `settings`; else no accent (the
/// theme's own colours). Each invalid one is reported.
pub fn resolve_accent(flag: Option<&str>, env: Option<&str>, settings: Option<&str>) -> (Option<u32>, Source, Vec<String>) {
    let mut warnings = Vec::new();
    for (v, src, what) in
        [(flag, Source::Flag, "--accent"), (env, Source::Env, ENV_ACCENT), (settings, Source::Settings, "settings accent")]
    {
        let Some(v) = v else { continue };
        match parse_hex(v) {
            Some(c) => return (Some(c), src, warnings),
            None => warnings.push(format!("{what}: '{}' is not a #RRGGBB colour; ignored", v.chars().take(40).collect::<String>())),
        }
    }
    (None, Source::Default, warnings)
}

// ── the resolved roles ────────────────────────────────────────────────────

macro_rules! roles {
    ($($name:ident),* $(,)?) => {
        /// Every colour the window paints with, resolved and floored (sRGB).
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        pub struct Roles { $(pub $name: u32,)* }

        /// [`Roles`] as GPUI colours, plus the selection wash. `Copy`:
        /// `render` takes it by value from the [`Scheme`] global.
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct Palette { $(pub $name: Hsla,)* pub sel: Hsla }

        impl Palette {
            pub fn of(r: &Roles) -> Self {
                // The selection wash is see-through: the text under it reads.
                // It is not a role (not in `ROLE_NAMES`, no floor): an
                // alpha over whatever it selects, not a colour of its own.
                Palette { $($name: hsla(r.$name),)* sel: Hsla { a: 0.35, ..hsla(r.focus) } }
            }

            /// Every role `t` (0..1) of the way to `to`'s, mixed as an
            /// opacity would paint it (motion.md §6.8's palette cross-fade);
            /// the selection wash is `to`'s once there.
            pub fn lerp(&self, to: &Palette, t: f32) -> Palette {
                if t >= 1. {
                    return *to;
                }
                if t <= 0. {
                    return *self;
                }
                Palette { $($name: crate::motion::mix(self.$name, to.$name, t),)* sel: self.sel }
            }

            /// Every role, by name.
            fn each_mut(&mut self, mut f: impl FnMut(&'static str, &mut Hsla)) {
                $(f(stringify!($name), &mut self.$name);)*
            }

            /// A role by its name (the tests).
            #[cfg(test)]
            pub fn role(&self, name: &str) -> Option<Hsla> {
                match name {
                    $(stringify!($name) => Some(self.$name),)*
                    _ => None,
                }
            }
        }

        /// Every role's name, in order (the tests find each in the
        /// [`Check`] log).
        #[cfg(test)]
        pub const ROLE_NAMES: &[&str] = &[$(stringify!($name)),*];
    };
}

roles! {
    // Surfaces: chrome (top bar, rail, right panel), the centre pane's
    // canvas, the user message and composer, code, a tool run and an opened
    // result, the permission card, a raised (selected) row on the chrome,
    // hairlines and input borders, diff line washes.
    chrome, canvas, user, code, tool, tool_out, card, card_hi, raised, hair, edge, frame, add_bg, del_bg,
    // An opened call's well (polish.md §2.3): `code`, or `tool_out` where
    // the ink does not read on `code` (paper's code is a dark island).
    well,
    // Rules drawn on Q1's boxes, each floored on its own ground and what is
    // around it: a well's frame and header rule, a chip's frame, OPEN's
    // outline (on a hovered row too).
    hair_w, hair_t, edge_t,
    // Text on the canvas.
    ink, prose, dim, you, agent, head, th, td, warn, run, ready, cost, err, well_fg,
    // On the user surface (a user message's markdown, the composer).
    dim_u, agent_u, head_u, hint,
    // On the code surface.
    code_fg, code_in, kw, string, num, com,
    // On a tool run (and its hover, the result surface).
    prose_t, dim_t, think, arg, ok, t_bash, t_read, t_write, t_edit, t_grep, t_find, t_ls, t_sub, t_ext, t_gen,
    // A subagent's block in a run: its name, step and facts, and its rule
    // (the TUI's `subagent_*` roles).
    sub_name_t, sub_status_t, sub_time_t, sub_rule,
    // On an opened result and its diff lines.
    res, dim_o, think_o, ok_o, err_o,
    // On the card (the permission card, the NEEDS YOU stub, overlays) and
    // its hovered step: text, then marks.
    // (`you_k`: the palette's matched letters, the accent floored as text.)
    prose_k, dim_k, warn_k, err_k, run_k, ok_k, you_k, run_mark_k, err_mark_k, rest_mark_k, focus_k,
    // Marks (3:1): the user's bar (on the canvas and user, and on a run:
    // the watched tray row), bullets, state glyphs and borders, focus, the
    // table rule, the two filled buttons.
    you_mark, you_mark_t, bullet, warn_mark, run_mark, focus, tb2, btn_fill, warn_fill,
    // The filled buttons' hover (a step toward the ink); a scrollbar's
    // thumb at rest on the canvas, the chrome, the user surface and the
    // card (the palette's list).
    btn_hi, warn_hi, thumb, thumb_ch, thumb_u, thumb_k,
    // Pressed (fluid.md §3.9): a step off each hover fill, toward the ink
    // or else away from it, never through a floor of what is drawn on it:
    // on the chrome (`raised`), the card (`card_hi`), and the two filled
    // buttons. (A run row's press waits for its row roles, Q1.)
    press_ch, press_k, btn_press, warn_press,
    // The chrome, on `bg` and the raised row: text, then marks.
    prose_ch, dim_ch, header, warn_ch, run_ch, unread_ch, open_ch, ready_ch, err_ch, cost_ch, ok_ch, edit_ch, write_ch, bash_ch,
    sub_name, sub_status, sub_done, sub_time,
    warn_mark_ch, run_mark_ch, unread_mark_ch, focus_ch, ready_mark_ch, err_mark_ch, rest_mark, you_bar,
    // Usage's chart series on the canvas (3:1): hosts, roles, models by
    // rank, and the rest.
    s1, s2, s3, s4, s5, s_rest,
    // Cards D (cards4/proposal.md §1): a run, an event, a chip and the
    // authorize card are one soft card a surface step off the canvas
    // (`card_d`; the proposal's `card`, a name the overlay card holds), in
    // a 1 px `line`; a row's hover fill; a call's body (`well_d`, the
    // proposal's `well`); the Ask pill, the authorize card's edge and the
    // text on the Allow fill; `acc`, the accent floored as text on all of
    // them (the running clock, waiting, receipts).
    card_d, line, hover, well_d, acc_soft, acc_line, acc_ink, acc,
    // A worker chip's fill (the chips over the composer, `+N more`, a
    // leaving chip; a spawn row is on its run's card, as a tool's row) and
    // its hover: the card and its hover turned toward the worker's hue, one
    // of the theme's own (Haseeb: a worker, not a tool, at a glance).
    card_agent, hover_agent,
    // The washes as painted (Shady #23 M3): a failed row's at rest and at
    // its flush's peak, a waiting row's, the authorize card's flush, and
    // the outlined Allow's pressed fill, from the floored `err` and `acc`,
    // each as strong as the proposal's where every text on it reads.
    wash_fail, wash_fail_pk, wash_wait, wash_wait_pk, ask_flush, out_press,
}

impl Palette {
    /// The link is down (motion.md §5.3): every role but the grounds and
    /// the ink `0.7 × k` of the way to the grey of its own luminance (so
    /// every contrast stays): status, identity, accent, the chrome's heads
    /// and wordmark, the rail's marks, tool hues, chart series. The window
    /// is visibly stale; its words still read.
    pub fn drained(&self, k: f32) -> Palette {
        if k <= 0. {
            return *self;
        }
        let t = 0.7 * k.min(1.);
        let mut p = *self;
        p.each_mut(|name, c| {
            if !UNDRAINED.contains(&name) {
                *c = drain_hsla(*c, t);
            }
        });
        p
    }

    /// The chrome's numbers as last seen, not live (the right panel
    /// detached, or a cached tab opening): every chrome text role in
    /// `dim_ch`, every chrome mark in `rest_mark` (it was `opacity(0.45)`).
    pub fn faded(&self) -> Palette {
        let (t, m) = (self.dim_ch, self.rest_mark);
        Palette {
            ink: t,
            prose_ch: t,
            header: t,
            warn_ch: t,
            run_ch: t,
            unread_ch: t,
            open_ch: t,
            ready_ch: t,
            err_ch: t,
            cost_ch: t,
            ok_ch: t,
            edit_ch: t,
            write_ch: t,
            bash_ch: t,
            sub_name: t,
            sub_status: t,
            sub_done: t,
            sub_time: t,
            warn_mark_ch: m,
            run_mark_ch: m,
            unread_mark_ch: m,
            focus_ch: m,
            ready_mark_ch: m,
            err_mark_ch: m,
            you_bar: m,
            ..*self
        }
    }

    /// Chart series `i`: `s1`–`s5`, then `s_rest`.
    pub fn series(&self, i: usize) -> Hsla {
        [self.s1, self.s2, self.s3, self.s4, self.s5].get(i).copied().unwrap_or(self.s_rest)
    }
}

fn hsla(c: u32) -> Hsla {
    rgb(c).into()
}

/// One floor a resolved role must meet: `role` at `floor` on every ground
/// (read by the tests, which check every one in every theme).
#[derive(Clone, Debug)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct Check {
    pub role: &'static str,
    /// The theme's own colour, and what the floor made of it.
    pub from: u32,
    pub color: u32,
    pub grounds: Vec<u32>,
    pub floor: f64,
}

struct Floors {
    ink: u32,
    log: Vec<Check>,
}

impl Floors {
    fn at(&mut self, role: &'static str, c: u32, toward: u32, grounds: &[u32], floor: f64) -> u32 {
        let v = lift(c, toward, grounds, floor);
        self.log.push(Check { role, from: c, color: v, grounds: grounds.to_vec(), floor });
        v
    }
    fn text(&mut self, role: &'static str, c: u32, grounds: &[u32]) -> u32 {
        self.at(role, c, self.ink, grounds, TEXT)
    }
    fn mark(&mut self, role: &'static str, c: u32, grounds: &[u32]) -> u32 {
        self.at(role, c, self.ink, grounds, MARK)
    }
}

/// The roles of `t` with `accent` (if any) as `prompt_fg` and
/// `border_active`, and every floor they were lifted to (the tests check
/// each one). The mapping is the spec's; the derivations are the TUI's.
pub fn resolve(t: &Raw, accent: Option<u32>) -> (Roles, Vec<Check>) {
    let ink = t.input_fg;
    let (canvas, chrome) = (t.message_bg, t.bg);
    let mut f = Floors { ink, log: Vec::new() };
    let surf = |c: u32| if contrast(c, canvas) >= SURFACE { c } else { step(canvas, t.claude_text, SURFACE) };
    let user = surf(t.user_bg);
    let code = surf(t.code_bg);
    for (role, from, c) in [("user", t.user_bg, user), ("code", t.code_bg, code)] {
        f.log.push(Check { role, from, color: c, grounds: vec![canvas], floor: SURFACE });
    }
    let tool = t.tool_input_bg.unwrap_or_else(|| step(canvas, t.claude_text, 1.14));
    let tool_out = t.tool_output_bg.unwrap_or_else(|| step(canvas, t.claude_text, 1.31));
    let card = step(chrome, ink, CARD);
    let card_hi = step(card, ink, STEP);
    let raised = step(chrome, ink, STEP);
    // Diff washes stand `SURFACE` off the result surface, as `user` and
    // `code` do off the canvas (design.md §1.6), mixed at most 30 % (the
    // most a built-in theme needs: solarized-dark's red), so a user theme
    // whose ok or error colour sits near its result surface gets no loud
    // wash; `ink` is floored on them.
    // Since Q1 cards they wash an opened call's well (polish.md §2.3), so
    // they stand off its ground: `code`, unless the theme's ink does not
    // read on `code` (paper's is a dark island), then `tool_out`.
    let washes = |g: u32| (step_upto(g, t.tool_result_ok, SURFACE, 60), step_upto(g, t.error_color, SURFACE, 60));
    let (on_code_add, on_code_del) = washes(code);
    let reads = [code, on_code_add, on_code_del].iter().all(|&c| contrast(ink, c) >= TEXT);
    let well = if reads { code } else { tool_out };
    let (add_bg, del_bg) = if reads { (on_code_add, on_code_del) } else { washes(tool_out) };
    let you = accent.unwrap_or(t.prompt_fg);
    let focus = accent.unwrap_or(t.border_active);
    let or = |raw: Option<u32>, d: u32| raw.unwrap_or(d);
    let ch = [chrome, raised];
    // A run's rows sit bare on the canvas (Q1 cards, F1), `tool_out` under
    // the hovered one; the worker chips are on `tool`.
    // Cards D (cards4/proposal.md §1): the card a surface step off the
    // canvas (toward white on a light canvas, else toward the ink: paper's
    // white is short of it), its hover a step on toward the ink, its body's
    // well a step back.
    let rise = |g: u32, toward: u32, target: f64| {
        (1..=200u8).map(|i| mix(g, toward, f64::from(i) * 0.005)).find(|&c| contrast(c, g) >= target).unwrap_or(toward)
    };
    let light = luminance(canvas) > 0.4;
    let card_d = if light && contrast(0xffffff, canvas) >= D_FILL { rise(canvas, 0xffffff, D_FILL) } else { rise(canvas, ink, D_FILL) };
    let hover = rise(card_d, ink, D_STEP);
    let well_d = rise(card_d, if light { ink } else { 0x000000 }, D_WELL);
    // A failed row's wash at rest and at its flush's peak (§8 #6), not
    // roles: what is drawn on them is floored on them.
    let (fail_rest, fail_peak) = (mix(card_d, t.error_color, f64::from(D_FAIL_REST)), mix(card_d, t.error_color, f64::from(D_FAIL_PEAK)));
    // A worker chip's fill and its hover (Haseeb: a worker, not a tool):
    // the card and its hover turned toward the worker's hue (the theme's
    // own), each at its own luminance, so what reads on the card and its
    // hover reads on them.
    let (card_agent, hover_agent, _) = agent(card_d, hover, t);
    let run_grounds = [canvas, tool, tool_out, card_d, hover, fail_rest, card_agent, hover_agent];
    // `thinking_colors()`: `thinking_color` lifted toward the answer's colour.
    let think = |f: &mut Floors, role, g: &[u32]| {
        let c = lift(t.thinking_color, t.claude_text, g, TEXT);
        f.text(role, c, g)
    };
    let mut r = Roles {
        chrome,
        canvas,
        user,
        code,
        tool,
        tool_out,
        card,
        card_hi,
        raised,
        add_bg,
        del_bg,
        well,
        card_d,
        hover,
        well_d,
        card_agent,
        hover_agent,
        ..Roles::default()
    };
    f.log.push(Check { role: "card_d", from: canvas, color: card_d, grounds: vec![canvas], floor: D_FILL });
    f.log.push(Check { role: "hover", from: card_d, color: hover, grounds: vec![card_d], floor: D_CARD });
    f.log.push(Check { role: "well_d", from: card_d, color: well_d, grounds: vec![card_d], floor: D_CARD });
    f.log.push(Check { role: "card_agent", from: card_d, color: card_agent, grounds: vec![canvas], floor: D_FILL });
    f.log.push(Check { role: "hover_agent", from: card_agent, color: hover_agent, grounds: vec![card_agent], floor: D_CARD });
    r.line = f.at("line", t.separator, ink, &[card_d, canvas, hover], D_LINE);
    for (role, from, c) in [("add_bg", well, add_bg), ("del_bg", well, del_bg)] {
        f.log.push(Check { role, from, color: c, grounds: vec![well], floor: SURFACE });
    }
    // Its ground stands off the canvas as `code` and `tool_out` do.
    f.log.push(Check { role: "well", from: well, color: well, grounds: vec![canvas], floor: SURFACE });
    f.log.push(Check { role: "card_hi", from: card, color: card_hi, grounds: vec![card], floor: STEP });
    // The ladder (design.md §1.4): a run and its open body off the canvas,
    // the card and the raised row off the chrome.
    for (role, from, c, ground, floor) in [
        ("tool", t.tool_input_bg.unwrap_or(canvas), tool, canvas, SURFACE),
        ("tool_out", t.tool_output_bg.unwrap_or(canvas), tool_out, canvas, SURFACE),
        ("card", chrome, card, chrome, CARD),
        ("raised", chrome, raised, chrome, STEP),
    ] {
        f.log.push(Check { role, from, color: c, grounds: vec![ground], floor });
    }
    // The composer foot's state is on `user` (design.md §4.4); the card's
    // hovered rows on `card_hi`.
    let k = [card, card_hi];
    // Lines (design.md §1.3): dividers on the chrome and the canvas;
    // control outlines there and on the user surface.
    r.hair = f.at("hair", t.separator, ink, &[canvas, chrome], HAIR);
    r.edge = f.at("edge", t.border, ink, &[canvas, chrome, user], EDGE);
    r.frame = f.at("frame", t.border, ink, &[card, card_hi, canvas, chrome, tool_out], FRAME);
    // A well's frame and header rule, floored on `tool_out` as well as the
    // well and the canvas: a hovered row's `tool_out` fill sits 2·r over its
    // well, and paper's well *is* `tool_out`, where a rule floored on the
    // well alone drew nothing (finish gate r1: well and hovered row read as
    // one grey L). So it reads heavier than `hair` in most themes (median
    // 1.37:1 on the well; `hair` is ~1.16:1 on the canvas), on purpose: the
    // wells are the only boxes in a run, and the design re-gate (r2) kept it.
    r.hair_w = f.at("hair_w", t.separator, ink, &[well, canvas, tool_out], HAIR);
    r.hair_t = f.at("hair_t", t.separator, ink, &[tool, tool_out, canvas], HAIR);
    r.edge_t = f.at("edge_t", t.border, ink, &[canvas, tool_out], EDGE);
    r.ink = f.text("ink", ink, &[canvas, user, tool_out, chrome, raised, add_bg, del_bg, card, card_hi, card_d, hover, well_d, fail_peak]);
    r.prose = f.text("prose", t.claude_text, &[canvas]);
    r.dim = f.text("dim", t.muted, &[canvas]);
    r.you = f.text("you", you, &[canvas]);
    r.agent = f.text("agent", t.claude_label, &[canvas]);
    r.head = f.text("head", t.heading_color, &[canvas]);
    r.th = f.text("th", t.table_header_color, &[canvas]);
    r.td = f.text("td", t.table_cell_color, &[canvas]);
    r.warn = f.text("warn", t.warning_color, &[canvas, tool, tool_out, user]);
    r.run = f.text("run", t.status_streaming, &[canvas, tool, tool_out, user]);
    r.ready = f.text("ready", t.status_ready, &[canvas, user]);
    r.cost = f.text("cost", t.cost_color, &[canvas]);
    r.err = f.text("err", t.error_color, &[canvas, tool, tool_out, user, card_d, hover, fail_rest, fail_peak, card_agent, hover_agent]);
    r.dim_u = f.text("dim_u", t.muted, &[user]);
    r.agent_u = f.text("agent_u", t.claude_label, &[user]);
    r.head_u = f.text("head_u", t.heading_color, &[user]);
    // The kit's placeholder colour, in every input: the composer (user),
    // find and hex (chrome), NAME (canvas), the secret field (card).
    r.hint = f.text("hint", t.muted, &[chrome, raised, canvas, user, card]);
    // Code: lifted toward the code text (a dark island on `paper`).
    r.code_fg = f.text("code_fg", t.code_fg, &[code]);
    // Inline code's chip is the well (paper's code is a dark island: a
    // near-black chip in light prose; its chip is `tool_out`), its text
    // the well's.
    r.code_in = f.text("code_in", if well == code { t.code_fg } else { t.tool_result_color }, &[well]);
    r.kw = f.at("kw", t.heading_color, t.code_fg, &[code], TEXT);
    r.string = f.at("string", t.list_bullet_color, t.code_fg, &[code], TEXT);
    r.num = f.at("num", t.warning_color, t.code_fg, &[code], TEXT);
    r.com = f.at("com", t.quote_color, t.code_fg, &[code], TEXT);
    r.prose_t = f.text("prose_t", t.claude_text, &run_grounds);
    r.dim_t = f.text("dim_t", t.muted, &run_grounds);
    r.think = think(&mut f, "think", &run_grounds);
    r.arg = f.text("arg", t.tool_param, &run_grounds);
    r.ok = f.text("ok", t.tool_result_ok, &[canvas, tool, tool_out, card_d, hover, card_agent, hover_agent]);
    // `draw.rs` `tool_accent()`: an unset `tool_*` derives from the palette.
    r.t_bash = f.text("t_bash", or(t.tool_bash, t.tool_result_ok), &run_grounds);
    r.t_read = f.text("t_read", or(t.tool_read, t.claude_label), &run_grounds);
    r.t_write = f.text("t_write", or(t.tool_write, t.warning_color), &run_grounds);
    r.t_edit = f.text("t_edit", or(t.tool_edit, t.cost_color), &run_grounds);
    r.t_grep = f.text("t_grep", or(t.tool_grep, t.table_header_color), &run_grounds);
    r.t_find = f.text("t_find", or(t.tool_find, t.subagent_name), &run_grounds);
    r.t_ls = f.text("t_ls", or(t.tool_ls, t.table_cell_color), &run_grounds);
    r.t_sub = f.text("t_sub", or(t.tool_subagent, t.subagent_name), &run_grounds);
    r.t_ext = f.text("t_ext", or(t.tool_ext, or(t.event_icon, t.border_active)), &run_grounds);
    r.t_gen = f.text("t_gen", or(t.tool_generic, t.tool_label), &run_grounds);
    r.sub_name_t = f.text("sub_name_t", t.subagent_name, &run_grounds);
    r.sub_status_t = f.text("sub_status_t", t.subagent_status, &run_grounds);
    r.sub_time_t = f.text("sub_time_t", t.subagent_time, &run_grounds);
    r.sub_rule = f.mark("sub_rule", t.subagent_border, &run_grounds);
    // An opened call's well (polish.md §2.3): its text is the code's on
    // `code`, the result's on `tool_out`.
    r.res = f.text("res", if well == code { t.code_fg } else { t.tool_result_color }, &[well, well_d]);
    r.dim_o = f.text("dim_o", t.muted, &[well, well_d]);
    // Unpainted since Q1 cards (an opened thought is `think` on the
    // canvas); kept for the roles that list it.
    r.think_o = think(&mut f, "think_o", &[tool_out]);
    r.ok_o = f.text("ok_o", t.tool_result_ok, &[add_bg]);
    r.err_o = f.text("err_o", t.error_color, &[del_bg, well, well_d]);
    r.prose_k = f.text("prose_k", t.claude_text, &k);
    r.dim_k = f.text("dim_k", t.muted, &k);
    r.warn_k = f.text("warn_k", t.warning_color, &k);
    r.err_k = f.text("err_k", t.error_color, &k);
    // The authorize card's command (polish.md §2.6): its first read, so
    // its loudest text, at least `CMD` on its well (the canvas) and as
    // loud as any other text on the card is on the card.
    let loud = [r.prose_k, r.dim_k, r.warn_k, r.err_k].iter().map(|&c| contrast(c, card)).fold(CMD, f64::max);
    r.well_fg = f.at("well_fg", t.code_fg, ink, &[canvas], loud);
    r.run_k = f.text("run_k", t.status_streaming, &k);
    r.ok_k = f.text("ok_k", t.tool_result_ok, &k);
    r.you_k = f.text("you_k", you, &k);
    r.run_mark_k = f.mark("run_mark_k", t.status_streaming, &k);
    r.err_mark_k = f.mark("err_mark_k", t.error_color, &k);
    r.rest_mark_k = f.mark("rest_mark_k", t.muted, &k);
    r.focus_k = f.mark("focus_k", focus, &k);
    r.you_mark = f.mark("you_mark", you, &[canvas, user]);
    r.you_mark_t = f.mark("you_mark_t", you, &run_grounds);
    r.bullet = f.mark("bullet", t.list_bullet_color, &[canvas]);
    // `tool_out` too: a run's open body and hovered rows (`glyph::ink`).
    r.warn_mark = f.mark("warn_mark", t.warning_color, &[canvas, tool, tool_out, user, card, card_hi]);
    r.run_mark = f.mark("run_mark", t.status_streaming, &[canvas, tool, tool_out, user]);
    r.focus = f.mark("focus", focus, &[canvas, chrome]);
    r.tb2 = f.mark("tb2", t.table_border_color, &[canvas]);
    // A filled button's label is `bg`: the fill is lifted until it reads.
    let btn = f.mark("btn_fill", you, &[user, card, card_d]);
    r.btn_fill = f.at("btn_fill", btn, ink, &[chrome], TEXT);
    let warn = f.mark("warn_fill", t.warning_color, &[user, card]);
    r.warn_fill = f.at("warn_fill", warn, ink, &[chrome], TEXT);
    // The final fills still stand off the surfaces they sit on.
    for (role, c) in [("btn_fill", r.btn_fill), ("warn_fill", r.warn_fill)] {
        f.log.push(Check { role, from: c, color: c, grounds: vec![user, card], floor: MARK });
    }
    // Hovered, a step toward the ink: the label (`chrome`) reads on it still.
    r.btn_hi = step(r.btn_fill, ink, STEP);
    r.warn_hi = step(r.warn_fill, ink, STEP);
    for (role, from, c) in [("btn_hi", r.btn_fill, r.btn_hi), ("warn_hi", r.warn_fill, r.warn_hi)] {
        f.log.push(Check { role, from, color: c, grounds: vec![chrome], floor: TEXT });
    }
    r.prose_ch = f.text("prose_ch", t.claude_text, &ch);
    r.dim_ch = f.text("dim_ch", t.muted, &ch);
    r.header = f.text("header", t.header_fg, &ch);
    r.warn_ch = f.text("warn_ch", t.warning_color, &ch);
    r.run_ch = f.text("run_ch", t.status_streaming, &ch);
    r.unread_ch = f.text("unread_ch", t.claude_label, &ch);
    r.open_ch = f.text("open_ch", focus, &ch);
    r.ready_ch = f.text("ready_ch", t.status_ready, &ch);
    r.err_ch = f.text("err_ch", t.error_color, &ch);
    r.cost_ch = f.text("cost_ch", t.cost_color, &ch);
    r.ok_ch = f.text("ok_ch", t.tool_result_ok, &ch);
    r.edit_ch = f.text("edit_ch", or(t.tool_edit, t.cost_color), &ch);
    r.write_ch = f.text("write_ch", or(t.tool_write, t.warning_color), &ch);
    r.bash_ch = f.text("bash_ch", or(t.tool_bash, t.tool_result_ok), &ch);
    r.sub_name = f.text("sub_name", t.subagent_name, &ch);
    r.sub_status = f.text("sub_status", t.subagent_status, &ch);
    r.sub_done = f.text("sub_done", t.subagent_done, &ch);
    r.sub_time = f.text("sub_time", t.subagent_time, &ch);
    r.warn_mark_ch = f.mark("warn_mark_ch", t.warning_color, &ch);
    r.run_mark_ch = f.mark("run_mark_ch", t.status_streaming, &ch);
    r.unread_mark_ch = f.mark("unread_mark_ch", t.claude_label, &ch);
    r.focus_ch = f.mark("focus_ch", focus, &ch);
    r.ready_mark_ch = f.mark("ready_mark_ch", t.status_ready, &ch);
    r.err_mark_ch = f.mark("err_mark_ch", t.error_color, &ch);
    r.rest_mark = f.mark("rest_mark", t.muted, &ch);
    r.you_bar = f.mark("you_bar", you, &ch);
    // A thumb at rest: its ground's dim text, 55 % of the way off the
    // ground (what `opacity(0.55)` painted). It stands off the ground as a
    // divider does.
    [r.thumb, r.thumb_ch, r.thumb_u, r.thumb_k] =
        [mix(canvas, r.dim, 0.55), mix(chrome, r.dim_ch, 0.55), mix(user, r.dim_u, 0.55), mix(card, r.dim_k, 0.55)];
    // `thumb_k` is drawn on the card and on its selected row's `card_hi`
    // (the palette's bar lies over the selection quad).
    for (role, grounds, from, c) in [
        ("thumb", vec![canvas], r.dim, r.thumb),
        ("thumb_ch", vec![chrome], r.dim_ch, r.thumb_ch),
        ("thumb_u", vec![user], r.dim_u, r.thumb_u),
        ("thumb_k", vec![card, card_hi], r.dim_k, r.thumb_k),
    ] {
        f.log.push(Check { role, from, color: c, grounds, floor: HAIR });
    }
    // Pressed ([`pressed`]): what is drawn on each is logged on it.
    {
        let (texts_ch, marks_ch) = (
            [r.ink, r.prose_ch, r.dim_ch, r.header, r.warn_ch, r.run_ch, r.unread_ch, r.open_ch, r.ready_ch, r.err_ch, r.cost_ch, r.ok_ch],
            [r.warn_mark_ch, r.run_mark_ch, r.unread_mark_ch, r.focus_ch, r.ready_mark_ch, r.err_mark_ch, r.rest_mark, r.you_bar],
        );
        let (texts_k, marks_k) = (
            [r.ink, r.prose_k, r.dim_k, r.warn_k, r.err_k, r.run_k, r.ok_k],
            [r.run_mark_k, r.err_mark_k, r.rest_mark_k, r.focus_k, r.warn_mark],
        );
        let label = [chrome];
        // (role, rest, hover, what is drawn on it: texts, marks)
        type Set<'a> = (&'static str, u32, u32, &'a [u32], &'a [u32]);
        let sets: [Set; 4] = [
            ("press_ch", chrome, raised, &texts_ch, &marks_ch),
            ("press_k", card, card_hi, &texts_k, &marks_k),
            ("btn_press", r.btn_fill, r.btn_hi, &label, &[]),
            ("warn_press", r.warn_fill, r.warn_hi, &label, &[]),
        ];
        let mut out = [0u32; 4];
        for (i, (role, rest, hover, texts, marks)) in sets.into_iter().enumerate() {
            let c = pressed(rest, hover, ink, texts, marks);
            out[i] = c;
            for &t in texts {
                f.log.push(Check { role, from: t, color: t, grounds: vec![c], floor: TEXT });
            }
            for &m in marks {
                f.log.push(Check { role, from: m, color: m, grounds: vec![c], floor: MARK });
            }
        }
        [r.press_ch, r.press_k, r.btn_press, r.warn_press] = out;
    }
    // Series: five theme colours at the mark floor on the canvas; one that
    // lands within 1.25:1 of an earlier one is mixed 40 % toward the ink.
    // Warning and error colours are never series (they keep their meaning).
    let mut series: Vec<u32> = Vec::with_capacity(5);
    for (role, c) in
        [("s1", t.claude_label), ("s2", t.list_bullet_color), ("s3", t.tool_result_color), ("s4", t.heading_color), ("s5", t.tool_param)]
    {
        let mut v = lift(c, ink, &[canvas], MARK);
        if series.iter().any(|&o| contrast(o, v) < 1.25) {
            v = mix(v, ink, 0.4);
        }
        series.push(f.at(role, v, ink, &[canvas], MARK));
    }
    [r.s1, r.s2, r.s3, r.s4, r.s5] = [series[0], series[1], series[2], series[3], series[4]];
    r.s_rest = f.mark("s_rest", t.muted, &[canvas]);
    // The accent's D roles (cards4/proposal.md §1): the Ask pill's fill,
    // the authorize card's and a watched chip's edge, the Allow fill's
    // text, and `acc` floored as text on every ground it is drawn on (the
    // card, its hover, the pill, the waiting row's wash at rest and peak).
    let soft = mix(card_d, you, f64::from(D_SOFT));
    // An accent as dark as the card steps the pill toward the ink instead.
    r.acc_soft = if contrast(soft, card_d) >= D_SOFT_OFF {
        soft
    } else {
        Some(rise(card_d, you, D_SOFT_OFF)).filter(|&c| contrast(c, card_d) >= D_SOFT_OFF).unwrap_or_else(|| rise(card_d, ink, D_SOFT_OFF))
    };
    let (wait_rest, wait_peak) = (mix(card_d, you, f64::from(D_WAIT_REST)), mix(card_d, you, f64::from(D_WAIT_PEAK)));
    r.acc = f.text("acc", you, &[card_d, hover, r.acc_soft, wait_rest, wait_peak, card_agent, hover_agent]);
    f.log.push(Check { role: "acc_soft", from: you, color: r.acc_soft, grounds: vec![card_d], floor: D_SOFT_OFF });
    r.acc_line = f.at("acc_line", mix(card_d, you, 0.4), ink, &[card_d], D_ACC_LINE);
    r.acc_ink = if [r.btn_fill, r.btn_hi].iter().all(|&g| contrast(canvas, g) >= TEXT) { canvas } else { chrome };
    f.log.push(Check { role: "acc_ink", from: canvas, color: r.acc_ink, grounds: vec![r.btn_fill, r.btn_hi], floor: TEXT });
    let washes = d_washes(&r);
    [r.wash_fail, r.wash_fail_pk, r.wash_wait, r.wash_wait_pk, r.ask_flush, r.out_press] = washes;
    // Grounds, not texts: each is logged with the texts it carries as its
    // floor's grounds (every one ≥ TEXT on it, `d_washes`).
    let texts = d_wash_texts(&r);
    for (role, c, on) in [
        ("wash_fail", r.wash_fail, 0),
        ("wash_fail_pk", r.wash_fail_pk, 0),
        ("wash_wait", r.wash_wait, 1),
        ("wash_wait_pk", r.wash_wait_pk, 1),
        ("ask_flush", r.ask_flush, 2),
        ("out_press", r.out_press, 3),
    ] {
        f.log.push(Check { role, from: card_d, color: c, grounds: texts[on].clone(), floor: TEXT });
    }
    (r, f.log)
}

/// The centre pane's sizes at a text step (`transcript-design/spec.md`
/// section 3, all given at 16 and scaled with the step): fonts never under
/// 12 px, boxes in whole pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub step: u8,
    /// Prose and user text, and its line.
    pub body: f32,
    pub body_line: f32,
    /// The column: 42 rem.
    pub column: f32,
    /// Tool rows, results, diffs; code blocks.
    pub mono: f32,
    pub mono_line: f32,
    pub code_line: f32,
    pub meta: f32,
    pub label: f32,
    pub card_title: f32,
    pub card_lead: f32,
    pub card_lead_line: f32,
    pub think: f32,
    pub think_line: f32,
    pub table: f32,
    pub table_line: f32,
    pub row: f32,
    /// A compact list row (the tray's).
    pub row_compact: f32,
    pub summary: f32,
    pub first_gap: f32,
    pub turn_gap: f32,
    pub part_gap: f32,
    pub pad_y: f32,
    /// `inset`: the content inset of every surface in the column (design.md
    /// §1.1), a run's rows and the tray's included.
    pub pad_x: f32,
    /// `lead`: a row's glyph column (12) and its gap (8); a row's name and
    /// its open body start at `pad_x + lead`.
    pub lead: f32,
    /// The gap between a row's name, argument and trail.
    pub run_gap: f32,
    /// A label hangs this far left of the column.
    pub hang: f32,
    pub r: f32,
}

impl Metrics {
    pub fn at(step: u8) -> Self {
        let step = if TEXT_STEPS.contains(&step) { step } else { DEFAULT_TEXT };
        let r = f32::from(step) / 16.;
        let font = |v: f32| (v * r).max(12.);
        let px = |v: f32| (v * r).round();
        Metrics {
            step,
            body: f32::from(step),
            body_line: px(26.),
            column: 672. * r,
            mono: font(13.),
            mono_line: px(20.),
            code_line: px(21.),
            meta: font(12.),
            label: font(13.),
            card_title: font(14.),
            card_lead: font(15.),
            card_lead_line: px(24.),
            think: font(14.),
            think_line: px(22.),
            table: font(14.),
            table_line: px(22.),
            row: px(32.),
            row_compact: px(24.),
            summary: px(36.),
            first_gap: px(20.),
            turn_gap: px(40.),
            part_gap: px(16.),
            pad_y: px(12.),
            pad_x: px(16.),
            lead: px(20.),
            run_gap: px(12.),
            hang: px(20.),
            r,
        }
    }

    /// The step after `step` in `dir` (+1, −1), or `None` at either end.
    pub fn bump(step: u8, dir: i8) -> Option<u8> {
        let i = TEXT_STEPS.iter().position(|&s| s == step).unwrap_or(2) as i32 + i32::from(dir);
        usize::try_from(i).ok().and_then(|i| TEXT_STEPS.get(i)).copied()
    }
}

/// What the window is painted with: the user's choices and everything
/// derived from them.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    /// As named (`night-city`), and its palette.
    pub theme: String,
    pub raw: Raw,
    pub accent: Option<u32>,
    /// A [`TEXT_STEPS`] value.
    pub text: u8,
}

pub struct Scheme {
    pub choice: Choice,
    pub roles: Roles,
    pub p: Palette,
    pub m: Metrics,
    /// Agent prose (installed as every text view's default) and the user's
    /// own text, which only differs in its foreground.
    pub user_text: TextViewStyle,
    /// A theme or accent change cross-fading (motion.md §6.8): the palette
    /// before it, when it began, and how far it is as of the app's frame
    /// ([`Scheme::fade`]). [`palette`] paints from that blend.
    pub from: std::cell::Cell<Option<(Palette, Option<std::time::Instant>, f32)>>,
    /// The link is down (motion.md §5.3): how drained the palette is, as of
    /// the app's frame (0: not at all).
    pub drain: std::cell::Cell<f32>,
    /// The blended palette for the last (fade, drain), so the many reads a
    /// frame (one per transcript row) cost a copy, not a blend.
    cache: std::cell::Cell<Option<(f32, f32, Palette)>>,
    /// A cross-fade's kit theme and text views' style wait for its last
    /// frame (motion.md §6.8).
    sync_due: std::cell::Cell<bool>,
    /// How far through the cross-fade the text views' style was last
    /// installed (1: at the end, or no fade).
    pub text_t: std::cell::Cell<f32>,
}

impl Scheme {
    /// The cross-fade's start (none: its first frame is next) and its last
    /// progress, if one runs.
    pub fn fading(&self) -> Option<(Option<std::time::Instant>, f32)> {
        self.from.get().map(|(_, at, t)| (at, t))
    }

    /// The app's frame sets when the cross-fade began (its first frame) and
    /// how far it is (`t` 1: over). True on its last frame: the kit's theme
    /// and the text views' style are due ([`settle_fade`]).
    pub fn fade(&self, at: std::time::Instant, t: f32) -> bool {
        self.from.set(self.from.get().filter(|_| t < 1.).map(|(p, _, _)| (p, Some(at), t)));
        t >= 1. && self.sync_due.replace(false)
    }

    /// The cross-fade's blend, not drained.
    fn blended(&self) -> Palette {
        match self.from.get() {
            Some((from, _, t)) => from.lerp(&self.p, t),
            None => self.p,
        }
    }
}

impl Global for Scheme {}

impl Scheme {
    pub fn new(choice: Choice) -> Self {
        let (roles, _) = resolve(&choice.raw, choice.accent);
        let p = Palette::of(&roles);
        let m = Metrics::at(choice.text);
        // A user message's markdown sits on the user surface: its muted,
        // quoted, link and heading colours are the ones floored there.
        let on_user = Palette { prose: p.ink, dim: p.dim_u, agent: p.agent_u, head: p.head_u, ..p };
        let user_text = text_style(&on_user, &m, luminance(roles.canvas) < 0.5);
        Scheme {
            choice,
            roles,
            p,
            m,
            user_text,
            from: Default::default(),
            drain: Default::default(),
            cache: Default::default(),
            sync_due: Default::default(),
            text_t: std::cell::Cell::new(1.),
        }
    }
}

/// The palette not drained (a cross-fade's blend): what the link's Scan
/// is drawn in, live again from its first frame (motion.md §5.1).
pub fn live_palette(cx: &App) -> Palette {
    cx.global::<Scheme>().blended()
}

/// The palette, by value (no allocation).
pub fn palette(cx: &App) -> Palette {
    let s = cx.global::<Scheme>();
    let (k, fade) = (s.drain.get(), s.from.get());
    if k <= 0. && fade.is_none() {
        return s.p;
    }
    let t = fade.map_or(1., |f| f.2);
    if let Some((ck, ct, p)) = s.cache.get()
        && (ck, ct) == (k, t)
    {
        return p;
    }
    let p = s.blended().drained(k);
    s.cache.set(Some((k, t, p)));
    p
}

/// A colour's relative luminance (WCAG).
#[cfg(test)]
pub fn luminance_hsla(c: Hsla) -> f32 {
    let lin = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    let rgb = Rgba::from(c);
    0.2126 * lin(rgb.r) + 0.7152 * lin(rgb.g) + 0.0722 * lin(rgb.b)
}

/// What a drain leaves: the grounds (and what is a ground: fills, rules,
/// thumbs), and the ink and prose the words are in.
const UNDRAINED: &[&str] = &[
    "chrome",
    "canvas",
    "user",
    "code",
    "tool",
    "tool_out",
    "card",
    "card_hi",
    "raised",
    "hair",
    "edge",
    "frame",
    "add_bg",
    "del_bg",
    "thumb",
    "thumb_ch",
    "thumb_u",
    "thumb_k",
    "press_ch",
    "press_k",
    "ink",
    "prose",
    // Q1's (PR #16) ground and rules (a well greys while the code beside
    // it doesn't): `grounds_and_rules_never_drain` fails on any new one
    // missing here.
    "well",
    "hair_w",
    "hair_t",
    "edge_t", // Cards D's grounds and rules.
    "card_d",
    "line",
    "hover",
    "well_d", // A worker's fill and its hover.
    "card_agent",
    "hover_agent",
];

/// `c` mixed `t` toward the grey of its own relative luminance, in linear
/// light (motion.md §5.3's drain): its luminance, so every contrast it
/// makes, stays.
pub fn drain_hsla(c: Hsla, t: f32) -> Hsla {
    let lin = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    let enc = |l: f32| if l <= 0.003_130_8 { l * 12.92 } else { 1.055 * l.powf(1. / 2.4) - 0.055 };
    let rgb = Rgba::from(c);
    let (r, g, b) = (lin(rgb.r), lin(rgb.g), lin(rgb.b));
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let ch = |v: f32| enc(v + (y - v) * t).clamp(0., 1.);
    Hsla::from(Rgba { r: ch(r), g: ch(g), b: ch(b), a: rgb.a })
}

pub fn metrics(cx: &App) -> Metrics {
    cx.global::<Scheme>().m
}

/// Install (or change) what the window is painted with: the global, the
/// kit's theme (inputs, scrollbars, focus ring, code colours) and every
/// text view's default style; every window repaints once. A no-op when
/// nothing changed.
pub fn apply(choice: Choice, cx: &mut App) {
    if cx.try_global::<Scheme>().is_some_and(|s| s.choice == choice) {
        return;
    }
    let s = Scheme::new(choice);
    // A change of colour while the window shows one (not the first)
    // cross-fades from what was painted (motion.md §6.8), under Full: the
    // palette blends, and the kit's theme and the text views' style follow
    // on its last frame. Not across a polarity (dark to light): text and
    // ground would pass through each other's colour mid-fade, so that is
    // instant, as is a text size (no colour changes).
    let dark = luminance(s.roles.canvas) < 0.5;
    let fade = crate::motion::Level::current(cx) == crate::motion::Level::Full
        && cx.try_global::<Scheme>().is_some_and(|old| old.p != s.p && (luminance(old.roles.canvas) < 0.5) == dark);
    if fade {
        let old = cx.global::<Scheme>();
        s.from.set(Some((old.blended(), None, 0.)));
        s.sync_due.set(true);
        s.drain.set(old.drain.get());
    } else if let Some(old) = cx.try_global::<Scheme>() {
        s.drain.set(old.drain.get());
    }
    cx.set_global(s);
    if !fade {
        settle_fade(cx);
    }
    // A global: cached views (the app's) don't see it change.
    cx.refresh_windows();
}

/// The kit's theme (inputs, scrollbars, focus ring, code colours) and
/// every text view's default style, from the scheme: at once, or on a
/// cross-fade's last frame.
pub fn settle_fade(cx: &mut App) {
    let s = cx.global::<Scheme>();
    let (p, m, roles) = (s.p, s.m, s.roles);
    sync_kit(&p, &roles, cx);
    cx.global::<Scheme>().text_t.set(1.);
    let dark = luminance(roles.canvas) < 0.5;
    TextViewDefaults::global(cx).with_style(text_style(&p, &m, dark)).with_inherit_text_color(false).install(cx);
}

/// The text views' style from this frame's cross-fade blend (re-gate 5):
/// their colours (prose, links, code spans, headings) move with the
/// grounds, not after them. [`settle_fade`] installs the end.
pub fn blend_text(cx: &mut App) {
    let s = cx.global::<Scheme>();
    let (p, m, dark, t) = (s.blended(), s.m, luminance(s.roles.canvas) < 0.5, s.from.get().map_or(1., |f| f.2));
    s.text_t.set(t);
    TextViewDefaults::global(cx).with_style(text_style(&p, &m, dark)).with_inherit_text_color(false).install(cx);
}

/// Change one part of the current [`Choice`].
pub fn update(cx: &mut App, f: impl FnOnce(&mut Choice)) {
    let mut c = cx.global::<Scheme>().choice.clone();
    f(&mut c);
    apply(c, cx);
}

/// Agent prose as the spec sets it: headings 22 / 19 / 16 in
/// `heading_color`, code on the code surface in its own colours, tables
/// with a ruled head and no fill, bullets in `list_bullet_color`.
fn text_style(p: &Palette, m: &Metrics, dark: bool) -> TextViewStyle {
    let r = m.r;
    let (head, code_fg) = (p.head, p.code_fg);
    let sizes = [22. * r, 19. * r, 16. * r];
    let code_block = StyleRefinement::default()
        .bg(p.code)
        .border_1()
        .border_color(p.hair)
        .px(px((16. * r).round()))
        .py(px((12. * r).round()))
        .text_px(m.mono)
        .line_height(px(m.code_line))
        .text_color(code_fg);
    let table = StyleRefinement::default().text_px(m.table).line_height(px(m.table_line));
    let table_head = StyleRefinement::default().font_weight(FontWeight::SEMIBOLD).text_color(p.th).border_b_1().border_color(p.tb2);
    let table_cell = StyleRefinement::default().px(px((12. * r).round())).py(px((7. * r).round())).text_color(p.td);
    TextViewStyle::default()
        .with_foreground(p.prose)
        .with_muted_foreground(p.dim)
        .with_link(p.agent)
        .with_selection(p.sel)
        .with_code_background(p.code)
        .with_border(p.hair)
        .with_paragraph_gap(Rems(r))
        .with_heading(move |level| StyleRefinement::default().text_px(sizes[usize::from(level.clamp(1, 3)) - 1]).text_color(head))
        .with_code_block(code_block)
        .with_table(table)
        .with_table_head(table_head)
        .with_table_cell(table_cell)
        .with_inline_code(HighlightStyle { color: Some(p.code_in), background_color: Some(p.well), ..Default::default() })
        .with_list_style(StyleRefinement::default().text_color(p.bullet))
        .with_blockquote_style(StyleRefinement::default().text_color(p.dim).border_color(p.edge))
        .with_dark(dark)
}

/// Code colours from the theme's own roles (spec: keyword `heading_color`,
/// string `list_bullet_color`, number `warning_color`, comment
/// `quote_color`), each floored on the code surface.
fn highlight_theme(r: &Roles) -> Option<std::sync::Arc<gpui_kit::component::highlighter::HighlightTheme>> {
    let styles: [(&str, u32, Option<u16>); 33] = [
        ("keyword", r.kw, None),
        ("title", r.kw, Some(600)),
        ("function", r.code_fg, None),
        ("type", r.code_fg, None),
        ("constructor", r.code_fg, None),
        ("tag", r.kw, None),
        ("text.literal", r.code_fg, None),
        ("text.code.span", r.code_fg, None),
        ("link_text", r.code_fg, None),
        ("attribute", r.code_fg, None),
        ("property", r.code_fg, None),
        ("variable", r.code_fg, None),
        ("label", r.code_fg, None),
        ("number", r.num, None),
        ("boolean", r.num, None),
        ("constant", r.num, None),
        ("enum", r.code_fg, None),
        ("variant", r.code_fg, None),
        ("string", r.string, None),
        ("string.regex", r.string, None),
        ("string.special", r.string, None),
        ("string.special.symbol", r.string, None),
        ("string.escape", r.num, None),
        ("operator", r.code_fg, None),
        ("punctuation", r.code_fg, None),
        ("punctuation.bracket", r.code_fg, None),
        ("punctuation.delimiter", r.code_fg, None),
        ("punctuation.list_marker", r.code_fg, None),
        ("punctuation.special", r.code_fg, None),
        ("link_uri", r.com, None),
        ("comment", r.com, None),
        ("comment_doc", r.com, None),
        ("embedded", r.code_fg, None),
    ];
    let mut syntax = serde_json::Map::new();
    for (key, color, weight) in styles {
        let mut style = serde_json::Map::new();
        style.insert("color".into(), to_hex(color).into());
        if let Some(w) = weight {
            style.insert("font_weight".into(), w.into());
        }
        syntax.insert(key.into(), style.into());
    }
    let mut style = serde_json::Map::new();
    for (key, color) in [
        ("editor.background", r.code),
        ("editor.foreground", r.code_fg),
        ("editor.line_number", r.com),
        ("editor.active_line_number", r.code_fg),
    ] {
        style.insert(key.into(), to_hex(color).into());
    }
    style.insert("syntax".into(), syntax.into());
    let appearance = if luminance(r.canvas) < 0.5 { "dark" } else { "light" };
    let v = serde_json::json!({"name": "Synaps", "appearance": appearance, "style": style});
    serde_json::from_value(v).ok().map(std::sync::Arc::new)
}

/// The kit theme follows the roles: inputs, popovers, tooltips, scrollbars,
/// the focus ring and selection.
fn sync_kit(p: &Palette, r: &Roles, cx: &mut App) {
    let p = *p;
    let highlight = highlight_theme(r);
    let on_focus = hsla(if contrast(r.chrome, r.focus) >= contrast(r.ink, r.focus) { r.chrome } else { r.ink });
    Theme::update(cx, move |t| {
        if let Some(h) = highlight {
            t.highlight_theme = h;
        }
        t.font_family = crate::fonts::SANS.into();
        t.mono_font_family = crate::fonts::MONO.into();
        t.font_size = px(14.);
        t.mono_font_size = px(12.);
        t.radius = px(0.);
        t.radius_lg = px(0.);
        t.shadow = false;
        let c = &mut t.colors;
        c.background = p.chrome;
        c.foreground = p.ink;
        c.border = p.edge;
        c.input = p.edge;
        c.ring = p.focus;
        c.caret = p.ink;
        c.selection = p.sel;
        c.muted = p.raised;
        c.muted_foreground = p.hint;
        c.secondary = p.raised;
        c.secondary_hover = p.raised;
        c.secondary_active = p.raised;
        c.secondary_foreground = p.ink;
        c.accent = p.raised;
        c.accent_foreground = p.ink;
        c.popover = p.card;
        c.popover_foreground = p.ink;
        c.primary = p.focus;
        c.primary_hover = p.focus;
        c.primary_active = p.focus;
        c.primary_foreground = on_focus;
        c.danger = p.err_ch;
        c.danger_hover = p.err_ch;
        c.danger_active = p.err_ch;
        c.danger_foreground = p.chrome;
        c.warning = p.warn_ch;
        c.success = p.ready_ch;
        c.info = p.prose_ch;
        c.link = p.agent;
        c.link_hover = p.ink;
        c.link_active = p.ink;
        c.scrollbar = gpui_kit::transparent_black();
        c.scrollbar_thumb = p.edge;
        c.scrollbar_thumb_hover = p.dim_ch;
        c.sidebar = p.chrome;
        c.sidebar_border = p.hair;
        c.sidebar_accent = p.raised;
        c.sidebar_foreground = p.ink;
        c.list = p.chrome;
        c.list_hover = p.raised;
        c.list_active = p.raised;
        c.list_active_border = p.focus;
        c.tab_active = p.chrome;
        c.tab_active_foreground = p.ink;
        c.progress_bar = p.focus;
        c.slider_bar = p.focus;
        c.switch = p.focus;
    });
}

/// The settings file: `<config dir>/synaps-desktop/settings.json`.
/// Linux/BSD: `$XDG_CONFIG_HOME` or `~/.config`; macOS: `~/Library/Application
/// Support`; Windows: `%APPDATA%`. In a test, under the test process's own
/// folder ([`test_home`]): no test reads or writes the user's.
pub fn settings_path() -> Option<PathBuf> {
    let path = settings_path_in(env_var)?;
    #[cfg(test)]
    test_home::guard(&path);
    Some(path)
}

#[cfg(not(test))]
fn env_var(k: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(k)
}

#[cfg(test)]
fn env_var(k: &str) -> Option<std::ffi::OsString> {
    test_home::var(k)
}

/// [`settings_path`] in the environment `var` reads.
fn settings_path_in(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    let env_dir = |k: &str| var(k).map(PathBuf::from).filter(|p| p.is_absolute());
    let dir = if cfg!(windows) {
        env_dir("APPDATA")?
    } else if cfg!(target_os = "macos") {
        env_dir("HOME")?.join("Library").join("Application Support")
    } else {
        env_dir("XDG_CONFIG_HOME").or_else(|| env_dir("HOME").map(|h| h.join(".config")))?
    };
    Some(dir.join("synaps-desktop").join("settings.json"))
}

/// Tests' config: `XDG_CONFIG_HOME`, `APPDATA` and `HOME` are a folder of
/// the test process's own (the process environment is never changed: the
/// tests run on threads, and C code reads it too).
#[cfg(test)]
pub(crate) mod test_home {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    pub fn root() -> &'static Path {
        static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        ROOT.get_or_init(|| std::env::temp_dir().join(format!("synaps-desktop-test-home-{}", std::process::id())))
    }

    pub fn var(k: &str) -> Option<OsString> {
        matches!(k, "XDG_CONFIG_HOME" | "APPDATA" | "HOME").then(|| root().join(k).into_os_string())
    }

    /// A test resolved the user's real settings file (or folder): it stops
    /// there, before anything reads or writes it.
    pub fn guard(path: &Path) {
        if let Some(real) = super::settings_path_in(|k| std::env::var_os(k)) {
            let dir = real.parent().unwrap_or(&real);
            assert!(!path.starts_with(dir), "a test resolved the real settings file {}", path.display());
        }
    }

    #[test]
    fn tests_never_resolve_the_real_settings() {
        let path = super::settings_path().unwrap();
        assert!(path.starts_with(root()), "{}", path.display());
        let real = super::settings_path_in(|k| std::env::var_os(k));
        assert!(real.as_ref().is_none_or(|r| *r != path));
        let caught = std::panic::catch_unwind(|| guard(&super::settings_path_in(|k| std::env::var_os(k)).unwrap_or_default()));
        assert!(real.is_none() || caught.is_err(), "the guard stops the real path");
    }
}

fn load_settings(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// The saved accent override as written (unparsed: [`resolve_accent`]
/// checks it); `null` or missing: none. The accent-only picker wrote this
/// key too, so its choice carries over as the override.
pub fn load_accent(path: &Path) -> Option<String> {
    load_settings(path)?.get("accent")?.as_str().map(str::to_string)
}

/// Save the accent override, or `null` for the theme's own colours (see
/// [`save_settings`]).
pub fn save_accent(path: &Path, hex: Option<u32>) -> std::io::Result<()> {
    save_settings(path, &[("accent", hex.map(to_hex).into())])
}

/// A string the app saved under `key` (Settings' `daemon_label`,
/// `dash_url`, `usage_range`, `usage_host`); missing, `null` or empty: none.
pub fn load_str(path: &Path, key: &str) -> Option<String> {
    load_settings(path)?.get(key)?.as_str().map(str::to_string).filter(|v| !v.is_empty())
}

/// The value saved under `key`, as written (`None`: no file, no key).
pub fn load_value(path: &Path, key: &str) -> Option<serde_json::Value> {
    load_settings(path)?.get(key).cloned()
}

/// Split panes on (`panes.split`): on unless the file says `false`
/// (split panes step 6 turned them on by default; that is the off switch,
/// with `SYNAPS_DESK_SPLIT=0` for a run).
pub fn split_flag(v: &serde_json::Value) -> bool {
    v.get("panes.split").and_then(serde_json::Value::as_bool).unwrap_or(true)
}

/// [`split_flag`] in the settings file at `path` (none, or unreadable: on).
pub fn load_split(path: &Path) -> bool {
    load_settings(path).is_none_or(|v| split_flag(&v))
}

/// The theme the picker saved, as written ([`resolve_theme`] checks it).
pub fn load_theme(path: &Path) -> Option<String> {
    load_settings(path)?.get("theme")?.as_str().map(str::to_string)
}

/// Save the picked theme, or `null` to follow the Synaps config again.
pub fn save_theme(path: &Path, name: Option<&str>) -> std::io::Result<()> {
    save_settings(path, &[("theme", name.into())])
}

/// Which side panels are folded to their strips (`rail_collapsed`,
/// `right_collapsed` in the settings file; missing or not a boolean: open),
/// the widths they were dragged to (`rail_width`, `right_width`: whole
/// pixels, clamped to [`RAIL_WIDTHS`] / [`RIGHT_WIDTHS`]; missing, `null`
/// or not a number: the auto width), which HISTORY buckets are open
/// (`history_open`, a list of bucket names; missing or not a list: NAMED
/// and TODAY), whether FAVOURITES is folded (`favourites_closed`; missing:
/// open) and the centre pane's text size (`text_size`, one of
/// [`TEXT_STEPS`]; anything else: [`DEFAULT_TEXT`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Panels {
    pub rail_collapsed: bool,
    pub right_collapsed: bool,
    /// Set by a drag of the panel's edge; `None`: the auto width.
    pub rail_width: Option<u16>,
    pub right_width: Option<u16>,
    /// [`Bucket::bit`]s.
    pub history_open: u8,
    pub favs_closed: bool,
    pub text_size: u8,
}

/// The widths a drag can give the sessions rail and the right panel (px).
pub const RAIL_WIDTHS: (u16, u16) = (220, 480);
pub const RIGHT_WIDTHS: (u16, u16) = (240, 520);

impl Default for Panels {
    fn default() -> Self {
        Panels {
            rail_collapsed: false,
            right_collapsed: false,
            rail_width: None,
            right_width: None,
            history_open: Bucket::DEFAULT_OPEN,
            favs_closed: false,
            text_size: DEFAULT_TEXT,
        }
    }
}

impl Panels {
    const RAIL: &str = "rail_collapsed";
    const RIGHT: &str = "right_collapsed";
    const RAIL_WIDTH: &str = "rail_width";
    const RIGHT_WIDTH: &str = "right_width";
    const HISTORY: &str = "history_open";
    const FAVS: &str = "favourites_closed";
    const TEXT_SIZE: &str = "text_size";
}

/// Which of the panel keys a window changed (and so writes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PanelKeys {
    pub rail: bool,
    pub right: bool,
    pub rail_width: bool,
    pub right_width: bool,
    pub history: bool,
    pub favs: bool,
    pub text_size: bool,
}

impl PanelKeys {
    /// The keys that differ between `a` and `b`.
    pub fn changed(a: Panels, b: Panels) -> Self {
        PanelKeys {
            rail: a.rail_collapsed != b.rail_collapsed,
            right: a.right_collapsed != b.right_collapsed,
            rail_width: a.rail_width != b.rail_width,
            right_width: a.right_width != b.right_width,
            history: a.history_open != b.history_open,
            favs: a.favs_closed != b.favs_closed,
            text_size: a.text_size != b.text_size,
        }
    }

    pub fn is_empty(self) -> bool {
        self == Self::default()
    }

    pub fn or(self, o: Self) -> Self {
        PanelKeys {
            rail: self.rail || o.rail,
            right: self.right || o.right,
            rail_width: self.rail_width || o.rail_width,
            right_width: self.right_width || o.right_width,
            history: self.history || o.history,
            favs: self.favs || o.favs,
            text_size: self.text_size || o.text_size,
        }
    }
}

pub fn load_panels(path: &Path) -> Panels {
    let v = load_settings(path).unwrap_or_default();
    let flag = |k: &str| v.get(k).and_then(serde_json::Value::as_bool).unwrap_or(false);
    let history_open = v
        .get(Panels::HISTORY)
        .and_then(serde_json::Value::as_array)
        .map(|open| Bucket::ALL.into_iter().filter(|b| open.iter().any(|x| x.as_str() == Some(b.key()))).fold(0, |m, b| m | b.bit()))
        .unwrap_or(Bucket::DEFAULT_OPEN);
    let width = |k: &str, (lo, hi): (u16, u16)| {
        v.get(k).and_then(serde_json::Value::as_f64).filter(|w| w.is_finite()).map(|w| w.round().clamp(lo.into(), hi.into()) as u16)
    };
    Panels {
        rail_collapsed: flag(Panels::RAIL),
        right_collapsed: flag(Panels::RIGHT),
        rail_width: width(Panels::RAIL_WIDTH, RAIL_WIDTHS),
        right_width: width(Panels::RIGHT_WIDTH, RIGHT_WIDTHS),
        history_open,
        favs_closed: flag(Panels::FAVS),
        text_size: v
            .get(Panels::TEXT_SIZE)
            .and_then(serde_json::Value::as_u64)
            .and_then(|s| u8::try_from(s).ok())
            .filter(|s| TEXT_STEPS.contains(s))
            .unwrap_or(DEFAULT_TEXT),
    }
}

/// Save the panel states in `keys` only (see [`save_settings`]): another
/// window's fold of the other panel is left as it wrote it.
pub fn save_panels(path: &Path, p: Panels, keys: PanelKeys) -> std::io::Result<()> {
    let mut kv = Vec::with_capacity(2);
    if keys.rail {
        kv.push((Panels::RAIL, p.rail_collapsed.into()));
    }
    if keys.right {
        kv.push((Panels::RIGHT, p.right_collapsed.into()));
    }
    if keys.rail_width {
        kv.push((Panels::RAIL_WIDTH, p.rail_width.into()));
    }
    if keys.right_width {
        kv.push((Panels::RIGHT_WIDTH, p.right_width.into()));
    }
    if keys.history {
        let open: Vec<&str> = Bucket::ALL.into_iter().filter(|b| p.history_open & b.bit() != 0).map(Bucket::key).collect();
        kv.push((Panels::HISTORY, open.into()));
    }
    if keys.favs {
        kv.push((Panels::FAVS, p.favs_closed.into()));
    }
    if keys.text_size {
        kv.push((Panels::TEXT_SIZE, p.text_size.into()));
    }
    if kv.is_empty() {
        return Ok(());
    }
    save_settings(path, &kv)
}

/// Set `keys` in the settings file, keeping its other keys. A file that is
/// not a JSON object is kept as `settings.json.bad` (hand edits are not
/// lost), then replaced. Written to a temporary file of this process (two
/// windows never share one), flushed to disk, then renamed over the old
/// one (and, on Unix, the directory synced so the rename survives a
/// crash); the temporary file never outlives a failure.
pub fn save_settings(path: &Path, keys: &[(&str, serde_json::Value)]) -> std::io::Result<()> {
    save_settings_unless(path, keys, || false)
}

/// [`save_settings`], unless `stale()` (asked under the writer's lock)
/// says a later write already holds these keys: then nothing changes.
pub fn save_settings_unless(path: &Path, keys: &[(&str, serde_json::Value)], stale: impl FnOnce() -> bool) -> std::io::Result<()> {
    update_settings(path, |v| {
        if stale() {
            return;
        }
        for (k, val) in keys {
            v[*k] = val.clone();
        }
    })
}

/// The open tabs saved under `key` (the daemon they are of): `tabs.<key>`
/// in the settings file, `{"sessions": [ids], "journals": [ids], "active":
/// n}` (`journals` may be missing). Missing or malformed: none.
pub fn load_tabs(path: &Path, key: &str) -> Option<crate::tabs::Saved> {
    let v = load_settings(path)?;
    saved_of(v.get("tabs")?.get(key)?)
}

/// Save the open tabs under `key`, keeping every other daemon's (see
/// [`save_settings`]).
pub fn save_tabs(path: &Path, key: &str, saved: &crate::tabs::Saved) -> std::io::Result<()> {
    update_settings(path, |v| {
        if !v.get("tabs").is_some_and(serde_json::Value::is_object) {
            v["tabs"] = serde_json::json!({});
        }
        v["tabs"][key] = saved_json(saved);
    })
}

/// A split layout as the settings file keeps it (split/architecture.md
/// §5): `layout.<key>`, version 1. Each pane's tabs are exactly what
/// [`crate::tabs::Tabs::saved`] gives; `tree`'s leaves index `panes`.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub focused: usize,
    pub tree: LayoutNode,
    pub panes: Vec<crate::tabs::Saved>,
}

/// A node of a saved layout's tree: a pane (its index in
/// [`Layout::panes`]), or a split whose kids share it by `ratios` (Σ = 1).
#[derive(Clone, Debug, PartialEq)]
pub enum LayoutNode {
    Pane(usize),
    Split { axis: crate::split::Axis, ratios: Vec<f32>, kids: Vec<LayoutNode> },
}

impl LayoutNode {
    /// Its panes' indices, in tree order.
    pub fn leaves(&self) -> Vec<usize> {
        match self {
            LayoutNode::Pane(i) => vec![*i],
            LayoutNode::Split { kids, .. } => kids.iter().flat_map(LayoutNode::leaves).collect(),
        }
    }

    /// This node with only the panes `at` gives a new index (Shady #55 L2):
    /// a split left one kid is that kid, the others share its ratios.
    fn kept(&self, at: &impl Fn(usize) -> Option<usize>) -> Option<LayoutNode> {
        match self {
            LayoutNode::Pane(i) => at(*i).map(LayoutNode::Pane),
            LayoutNode::Split { axis, ratios, kids } => {
                let (mut k2, mut r2) = (Vec::new(), Vec::new());
                for (k, r) in kids.iter().zip(ratios) {
                    if let Some(k) = k.kept(at) {
                        k2.push(k);
                        r2.push(*r);
                    }
                }
                if k2.len() < 2 {
                    return k2.pop();
                }
                let sum: f32 = r2.iter().sum();
                Some(LayoutNode::Split { axis: *axis, ratios: r2.iter().map(|r| r / sum).collect(), kids: k2 })
            }
        }
    }

    fn to_json(&self) -> serde_json::Value {
        match self {
            LayoutNode::Pane(i) => serde_json::json!({"pane": i}),
            LayoutNode::Split { axis, ratios, kids } => serde_json::json!({
                "split": if *axis == crate::split::Axis::Row { "row" } else { "column" },
                "ratios": ratios,
                "kids": kids.iter().map(LayoutNode::to_json).collect::<Vec<_>>(),
            }),
        }
    }

    /// A node as written, or `None`: an unknown axis, a split with fewer
    /// than two kids or a ratio per kid, a ratio that is not a positive
    /// number, ratios that do not sum to 1.
    fn from_json(v: &serde_json::Value) -> Option<LayoutNode> {
        if let Some(i) = v.get("pane") {
            return Some(LayoutNode::Pane(usize::try_from(i.as_u64()?).ok()?));
        }
        let axis = match v.get("split")?.as_str()? {
            "row" => crate::split::Axis::Row,
            "column" => crate::split::Axis::Column,
            _ => return None,
        };
        let ratios: Vec<f32> = v.get("ratios")?.as_array()?.iter().map(|r| r.as_f64().map(|r| r as f32)).collect::<Option<_>>()?;
        let kids: Vec<LayoutNode> = v.get("kids")?.as_array()?.iter().map(LayoutNode::from_json).collect::<Option<_>>()?;
        let sum: f32 = ratios.iter().sum();
        let ok =
            kids.len() >= 2 && ratios.len() == kids.len() && ratios.iter().all(|r| r.is_finite() && *r > 0.) && (sum - 1.).abs() <= 1e-3;
        ok.then_some(LayoutNode::Split { axis, ratios, kids })
    }
}

impl Layout {
    /// The layout without its panes that have no session tabs (Shady #55
    /// L2): an empty pane split off, or one that showed only a view (views
    /// are not saved), would show on the first frame and close on the
    /// first list. Their shares go to the panes beside them; a focused one
    /// gives the focus to the pane before it (its left/up neighbour), else
    /// the first. All of them empty: the first pane alone.
    pub fn without_empty_panes(self) -> Layout {
        let mut keep: Vec<bool> = self.panes.iter().map(|p| !p.sessions.is_empty()).collect();
        if !keep.contains(&true) {
            keep = (0..self.panes.len()).map(|i| i == 0).collect();
        }
        if !keep.contains(&false) {
            return self;
        }
        let new: Vec<Option<usize>> = keep.iter().scan(0, |n, k| Some(k.then(|| std::mem::replace(n, *n + 1)))).collect();
        let at = |i: usize| new.get(i).copied().flatten();
        let focused = (0..=self.focused.min(new.len() - 1)).rev().find_map(at).or_else(|| new.iter().find_map(|n| *n)).unwrap_or(0);
        let tree = self.tree.kept(&at).unwrap_or(LayoutNode::Pane(0));
        let panes = self.panes.into_iter().zip(&keep).filter(|(_, k)| **k).map(|(p, _)| p).collect();
        Layout { focused, tree, panes }
    }

    /// What `tabs.<key>` keeps beside it (architecture §5): every pane's
    /// session tabs, in pane order, a session once; the focused pane's
    /// visible tab visible. An older build reopens them all, in one strip.
    pub fn union(&self) -> crate::tabs::Saved {
        let mut u = crate::tabs::Saved::default();
        let (mut journals, mut labels) = (Vec::new(), Vec::new());
        for (i, p) in self.panes.iter().enumerate() {
            for (j, s) in p.sessions.iter().enumerate() {
                let at = match u.sessions.iter().position(|x| x == s) {
                    Some(at) => at,
                    None => {
                        u.sessions.push(s.clone());
                        journals.push(p.journal(j).unwrap_or_default().to_string());
                        labels.push(p.label(j).unwrap_or_default().to_string());
                        u.sessions.len() - 1
                    }
                };
                if i == self.focused && p.active == Some(j) {
                    u.active = Some(at);
                }
            }
        }
        if journals.iter().any(|j| !j.is_empty()) {
            u.journals = journals;
        }
        if labels.iter().any(|l| !l.is_empty()) {
            u.labels = labels;
        }
        u
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "v": 1,
            "focused": self.focused,
            "tree": self.tree.to_json(),
            "panes": self.panes.iter().map(saved_json).collect::<Vec<_>>(),
        })
    }
}

/// One pane's tabs (or `tabs.<key>`) as the file keeps them.
fn saved_json(saved: &crate::tabs::Saved) -> serde_json::Value {
    serde_json::json!({"sessions": saved.sessions, "journals": saved.journals, "labels": saved.labels, "active": saved.active})
}

/// One pane's tabs (or `tabs.<key>`) as the file has them: `None` with no
/// list of sessions; what does not line up is left out ([`load_tabs`]).
fn saved_of(t: &serde_json::Value) -> Option<crate::tabs::Saved> {
    let strings =
        |k: &str| -> Option<Vec<String>> { Some(t.get(k)?.as_array()?.iter().filter_map(|s| s.as_str()).map(str::to_string).collect()) };
    let sessions = strings("sessions")?;
    let journals = strings("journals").filter(|j| j.len() == sessions.len()).unwrap_or_default();
    let labels = strings("labels").filter(|l| l.len() == sessions.len()).unwrap_or_default();
    let active = t.get("active").and_then(serde_json::Value::as_u64).map(|a| a as usize).filter(|&a| a < sessions.len());
    Some(crate::tabs::Saved { sessions, journals, labels, active })
}

/// The tests' read of the layout saved under `key` ([`layout_of`]).
#[cfg(test)]
pub fn load_layout(path: &Path, key: &str) -> Option<Layout> {
    layout_of(&load_settings(path)?, key)
}

/// The split layout saved under `key` (`layout.<key>`, v1) in settings
/// `v`, if it is whole (architecture §5): another version, a tree that
/// does not parse ([`LayoutNode`]), more than [`crate::pane::PANES_MAX`]
/// panes or more across or down than the caps, a pane no leaf names or
/// one named twice, a pane that is not a tab list: none, and the tabs are
/// read instead ([`load_restore`]). Never panics.
fn layout_of(v: &serde_json::Value, key: &str) -> Option<Layout> {
    let l = v.get("layout")?.get(key)?;
    if l.get("v")?.as_u64()? != 1 {
        return None;
    }
    let tree = LayoutNode::from_json(l.get("tree")?)?;
    let panes: Vec<crate::tabs::Saved> = l.get("panes")?.as_array()?.iter().map(saved_of).collect::<Option<_>>()?;
    let mut leaves = tree.leaves();
    leaves.sort_unstable();
    if panes.is_empty()
        || panes.len() > crate::pane::PANES_MAX
        || leaves != (0..panes.len()).collect::<Vec<_>>()
        || !crate::split::fits_caps(&tree)
    {
        return None;
    }
    let focused = l.get("focused").and_then(serde_json::Value::as_u64).map(|f| f as usize).filter(|&f| f < panes.len()).unwrap_or(0);
    Some(Layout { focused, tree, panes })
}

/// Save the layout under `key`, and beside it `tabs.<key>` as its union
/// ([`Layout::union`]: what an older build reads), in one write under the
/// settings lock, every other key and daemon kept ([`save_settings`]).
pub fn save_layout(path: &Path, key: &str, layout: &Layout) -> std::io::Result<()> {
    update_settings(path, |v| {
        for k in ["layout", "tabs"] {
            if !v.get(k).is_some_and(serde_json::Value::is_object) {
                v[k] = serde_json::json!({});
            }
        }
        v["layout"][key] = layout.to_json();
        v["tabs"][key] = saved_json(&layout.union());
    })
}

/// What a launch restores under `key`, from one read of the file (Shady
/// #55 n3): the layout when split panes are on (`env`, the run's
/// `SYNAPS_DESK_SPLIT`, if set; else [`split_flag`]; off, `layout.<key>`
/// is never read: n2), it is whole and
/// `tabs.<key>` is still exactly its union (an older build since, writing
/// only the tabs, a rename or a tab switch as much as other sessions,
/// leaves it stale: its tabs win, as one pane: L1); else the tabs alone.
pub fn load_restore(path: &Path, key: &str, env: Option<bool>) -> (Option<Layout>, Option<crate::tabs::Saved>) {
    let Some(v) = load_settings(path) else { return (None, None) };
    let tabs = v.get("tabs").and_then(|t| t.get(key)).and_then(saved_of);
    let split = env.unwrap_or_else(|| split_flag(&v));
    let layout = split.then(|| layout_of(&v, key)).flatten().filter(|l| tabs.as_ref().is_none_or(|t| *t == l.union()));
    (layout, tabs)
}

/// The favourite sessions saved under `key` (the daemon they are of, as
/// the tabs are keyed): `favourites.<key>` in the settings file, a list
/// ([`crate::favourites::to_json`]). Missing or malformed: none.
pub fn load_favourites(path: &Path, key: &str) -> Vec<crate::favourites::Favourite> {
    load_settings(path).and_then(|v| v.get("favourites")?.get(key).map(crate::favourites::from_json)).unwrap_or_default()
}

/// Save the favourites under `key`, keeping every other daemon's and every
/// other key (see [`save_settings`]): `ours` merged into the list the file
/// has now, from `base` (the list as this instance last read or wrote it;
/// [`crate::favourites::merge`]), so another instance's adds and removes
/// survive. Returns the list written. A file with no `favourites.<key>`
/// (none yet, or one that did not parse and was set aside) says nothing of
/// removals: `ours` is written as it is (a real "none left" is `[]`). A
/// `favourites` that is not an object, or a `favourites.<key>` that is not a
/// list (a newer build's form, a hand edit), is not overwritten: an error
/// says so.
pub fn save_favourites(
    path: &Path,
    key: &str,
    base: &[crate::favourites::Favourite],
    ours: &[crate::favourites::Favourite],
) -> std::io::Result<Vec<crate::favourites::Favourite>> {
    let mut written = Vec::new();
    try_update_settings(path, |v| {
        let refuse = |what: String| Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{what}: not overwritten")));
        match v.get("favourites") {
            None => v["favourites"] = serde_json::json!({}),
            Some(f) if !f.is_object() => return refuse("settings.json's favourites is not an object".into()),
            Some(f) => {
                if f.get(key).is_some_and(|l| !l.is_array()) {
                    return refuse(format!("settings.json's favourites.{key} is not a list"));
                }
            }
        }
        written = match v["favourites"].get(key) {
            Some(theirs) => crate::favourites::merge(&crate::favourites::from_json(theirs), base, ours),
            None => ours.to_vec(),
        };
        v["favourites"][key] = crate::favourites::to_json(&written);
        Ok(())
    })?;
    Ok(written)
}

/// How long a write waits for another process's lock on the settings file
/// (its read, merge and rename take milliseconds) before it writes
/// unlocked: a holder stopped mid-write (a suspended or hung process) never
/// freezes this one, on the UI thread or off it.
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_millis(250);

/// The advisory lock on `lock` (the settings' own lock file), tried until
/// `wait` has passed while another holds it (std's `WouldBlock`: `EWOULDBLOCK`,
/// Windows' `ERROR_LOCK_VIOLATION` only). `Err`: why it isn't held, at once
/// for a lock file that can't be opened or locked at all (the caller writes
/// unlocked and says so on stderr). Never a blocking lock.
fn lock_settings(lock: &Path, wait: std::time::Duration) -> Result<std::fs::File, String> {
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock)
        .map_err(|e| format!("{} not opened ({e})", lock.display()))?;
    let deadline = std::time::Instant::now() + wait;
    loop {
        match f.try_lock() {
            Ok(()) => return Ok(f),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(e) => return Err(format!("{} not locked ({e})", lock.display())),
        }
    }
}

/// The favourites this process holds, per settings file and daemon key, as
/// saved ([`crate::favourites::to_json`]): what a write that sets a corrupt
/// file aside carries into the new one ([`try_update_settings`]).
type Carried = Vec<(std::path::PathBuf, String, serde_json::Value)>;
static CARRIED: std::sync::Mutex<Carried> = std::sync::Mutex::new(Vec::new());

/// The favourites in memory under `key` now, for `path` ([`CARRIED`]).
pub fn carry_favourites(path: &Path, key: &str, list: &[crate::favourites::Favourite]) {
    let mut c = CARRIED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let v = crate::favourites::to_json(list);
    match c.iter_mut().find(|(p, k, _)| p == path && k == key) {
        Some((.., l)) => *l = v,
        None => c.push((path.to_path_buf(), key.to_string(), v)),
    }
}

/// Change the settings file with `edit` (on its JSON object), as
/// [`save_settings`] says.
fn update_settings(path: &Path, edit: impl FnOnce(&mut serde_json::Value)) -> std::io::Result<()> {
    try_update_settings(path, |v| {
        edit(v);
        Ok(())
    })
}

/// [`update_settings`] with an `edit` that may refuse: the file is then
/// left as it was. A file that is there but cannot be read is left as it
/// was too (an error says so): only a missing one starts empty.
fn try_update_settings(path: &Path, edit: impl FnOnce(&mut serde_json::Value) -> std::io::Result<()>) -> std::io::Result<()> {
    use std::io::Write;
    // One writer at a time in this process: the theme (UI thread) and the
    // panels (background) each re-read and merge, so neither write is lost.
    static WRITER: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = WRITER.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // And one at a time across processes (two windows, two builds): the
    // read, the merge and the rename under an advisory lock of a file of
    // its own, so no window writes over what another wrote after its read.
    let _lock = lock_settings(&path.with_extension("json.lock"), LOCK_WAIT)
        .inspect_err(|why| eprintln!("synaps-desktop: {why}: settings written unlocked"))
        .ok();
    // Lossy: a stray invalid byte is a file that does not parse (set aside
    // below), never one read as missing (which would wipe every key).
    let text = match std::fs::read(path) {
        Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let parsed = text.as_deref().and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok()).filter(serde_json::Value::is_object);
    let set_aside = parsed.is_none() && text.as_deref().is_some_and(|t| !t.trim().is_empty());
    if set_aside {
        std::fs::copy(path, path.with_extension("json.bad"))?;
    }
    let mut v = parsed.unwrap_or_else(|| serde_json::json!({}));
    edit(&mut v)?;
    // Set aside, the file had the favourites (they are in `.bad` now): a
    // write of other keys (the tabs, the theme) carries this process's in
    // memory, unless the edit wrote them itself.
    if set_aside {
        for (_, key, list) in CARRIED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().filter(|(p, ..)| p == path) {
            if !v.get("favourites").is_some_and(serde_json::Value::is_object) {
                v["favourites"] = serde_json::json!({});
            }
            if v["favourites"].get(key).is_none() {
                v["favourites"][key] = list.clone();
            }
        }
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let tmp = path.with_extension(format!("json.{}.{nanos}.tmp", std::process::id()));
    let written = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all((serde_json::to_string_pretty(&v).unwrap_or_default() + "\n").as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    #[cfg(unix)]
    if written.is_ok()
        && let Some(dir) = path.parent()
    {
        // Best effort: the file itself is already complete.
        let _ = std::fs::File::open(if dir.as_os_str().is_empty() { Path::new(".") } else { dir }).and_then(|d| d.sync_all());
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The drain (motion.md §5.3) keeps every role's contrast on every
    /// ground, in every theme and accent: each drained role sits at its own
    /// luminance (to within the 8-bit rounding of its channels), and the
    /// grounds, text and lines don't move.
    #[test]
    fn drain_keeps_every_floor() {
        let lum = |c: Hsla| f64::from(luminance_hsla(c));
        let con = |a: Hsla, b: Hsla| {
            let (x, y) = (lum(a), lum(b));
            (x.max(y) + 0.05) / (x.min(y) + 0.05)
        };
        let mut worst = 0f64;
        for (name, raw) in crate::palettes::THEMES {
            for accent in [None, Some(0xe8132f), Some(0xf4b400), Some(0x2d7bff), Some(0x35f2ff)] {
                let p = Palette::of(&resolve(&raw, accent).0);
                let d = p.drained(1.);
                assert_eq!(
                    (d.canvas, d.chrome, d.ink, d.prose, d.hair, d.tool, d.user),
                    (p.canvas, p.chrome, p.ink, p.prose, p.hair, p.tool, p.user),
                    "{name}"
                );
                assert_ne!(d.run, p.run, "{name}: the live colour drains");
                for (a, b) in [
                    (p.run, d.run),
                    (p.err_ch, d.err_ch),
                    (p.warn_mark, d.warn_mark),
                    (p.you_bar, d.you_bar),
                    (p.ok, d.ok),
                    (p.focus, d.focus),
                ] {
                    for ground in [p.canvas, p.chrome, p.tool, p.user] {
                        worst = worst.max((con(a, ground) - con(b, ground)).abs());
                    }
                }
                assert_eq!(p.drained(0.), p, "{name}: none at 0");
                // Every role but the grounds and the words loses most of its
                // hue (the wordmark and heads, rail marks, chart series too).
                let chroma = |c: Hsla| {
                    let c = Rgba::from(c);
                    c.r.max(c.g).max(c.b) - c.r.min(c.g).min(c.b)
                };
                let (mut dp, mut pp) = (d, p);
                let mut was = Vec::new();
                pp.each_mut(|n, c| was.push((n, *c)));
                let mut i = 0;
                dp.each_mut(|n, c| {
                    let (_, before) = was[i];
                    i += 1;
                    if !UNDRAINED.contains(&n) {
                        assert!(chroma(*c) <= 0.5 * chroma(before) + 0.01, "{name}: {n} keeps its hue: {before:?} → {c:?}");
                    }
                });
            }
        }
        assert!(worst < 0.06, "the drain moved a contrast by {worst:.3}");
    }

    /// The drain (motion.md §5.3) keeps every ground and rule: any role
    /// named as one (a `hair*`, `edge*`, `well`, `*_bg`, the grounds) is
    /// in `UNDRAINED`. A branch that adds one (Q1: `well`, `hair_w`,
    /// `hair_t`, `edge_t`) fails here until it is kept.
    #[test]
    fn grounds_and_rules_never_drain() {
        let mut p = Palette::of(&resolve(&crate::palettes::THEMES[0].1, None).0);
        let mut missing = Vec::new();
        p.each_mut(|n, _| {
            let rule = n.starts_with("hair") || n.starts_with("edge") || n.starts_with("well") && !n.contains("_fg") || n.ends_with("_bg");
            if rule && !UNDRAINED.contains(&n) {
                missing.push(n);
            }
        });
        assert!(missing.is_empty(), "grounds and rules that would drain: {missing:?}");
    }

    /// A press is a state of its own (fluid.md §3.9): every pressed fill
    /// stands [`PRESS_MIN`] off both its hover fill and its rest fill, in
    /// every theme (20) and accent (7), for all four press roles, and
    /// everything drawn on it still reads (its checks are in the log).
    #[test]
    fn press_is_visible_in_every_theme() {
        let mut worst = (f64::MAX, String::new());
        for (name, raw) in crate::palettes::THEMES {
            for accent in [None, Some(0xe8132f), Some(0xf4b400), Some(0xcfa85a), Some(0x2d7bff), Some(0x35f2ff), Some(0x2bd47d)] {
                let (r, log) = resolve(&raw, accent);
                let raised = step(r.chrome, r.ink, STEP);
                for (role, press, rest, hover) in [
                    ("press_ch", r.press_ch, r.chrome, raised),
                    ("press_k", r.press_k, r.card, r.card_hi),
                    ("btn_press", r.btn_press, r.btn_fill, r.btn_hi),
                    ("warn_press", r.warn_press, r.warn_fill, r.warn_hi),
                ] {
                    assert_ne!(press, hover, "{name} {accent:?} {role}: the hover fill");
                    assert_ne!(press, rest, "{name} {accent:?} {role}: the rest fill");
                    for (what, c) in [("hover", contrast(press, hover)), ("rest", contrast(press, rest))] {
                        if c < worst.0 {
                            worst = (c, format!("{name} {accent:?} {role} vs {what}"));
                        }
                    }
                }
                for c in log.iter().filter(|c| c.role.contains("press")) {
                    for &g in &c.grounds {
                        assert!(contrast(c.color, g) >= c.floor - 1e-9, "{name} {accent:?} {}: {:06x} on {g:06x}", c.role, c.color);
                    }
                }
            }
        }
        assert!(worst.0 >= PRESS_MIN, "a press too close to its hover or rest: {:.3} ({})", worst.0, worst.1);
    }

    /// The drain keeps every floor the theme was lifted to: each check of
    /// the log, re-run on the drained palette's colours.
    #[test]
    fn drained_checks_hold() {
        let lin = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        let lum = |c: Hsla| {
            let c = Rgba::from(c);
            f64::from(0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b))
        };
        let con = |a: Hsla, b: Hsla| {
            let (x, y) = (lum(a), lum(b));
            (x.max(y) + 0.05) / (x.min(y) + 0.05)
        };
        let mut bad = Vec::new();
        for (name, raw) in crate::palettes::THEMES {
            for accent in [None, Some(0xe8132f), Some(0x2d7bff)] {
                let (_, log) = resolve(&raw, accent);
                for c in &log {
                    let d = drain_hsla(hsla(c.color), 0.7);
                    for &g in &c.grounds {
                        // The grounds that drain (button fills) drain too.
                        let dg = drain_hsla(hsla(g), 0.7);
                        let got = con(d, hsla(g)).min(con(d, dg));
                        if got < c.floor - 0.01 {
                            bad.push(format!("{name} {accent:?} {}: {got:.3} < {}", c.role, c.floor));
                        }
                    }
                }
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex("#e8132f"), Some(0xe8132f));
        assert_eq!(parse_hex("E8132F"), Some(0xe8132f));
        assert_eq!(parse_hex("  #Ab0  "), Some(0xaabb00));
        assert_eq!(parse_hex("#fff"), Some(0xffffff));
        for bad in ["", "#", "#12345", "#1234567", "zzzzzz", "#12 345", "red", "#-12345", "+12345", "#e8132f00", "１２３"] {
            assert_eq!(parse_hex(bad), None, "{bad:?}");
        }
        assert_eq!(to_hex(0x0a1b2c), "#0a1b2c");
        assert_eq!(parse_hex(&to_hex(0x2d7bff)), Some(0x2d7bff));
    }

    #[test]
    fn contrast_and_lift_match_the_tui() {
        assert!((contrast(0x000000, 0xffffff) - 21.0).abs() < 1e-9);
        assert!((contrast(0x777777, 0x777777) - 1.0).abs() < 1e-9);
        assert_eq!(mix(0x000000, 0xffffff, 0.5), 0x808080);
        // Already readable: unchanged. Unreachable: the target colour itself.
        assert_eq!(lift(0xffffff, 0x000000, &[0x000000], 4.5), 0xffffff);
        assert_eq!(lift(0x101010, 0x202020, &[0x000000], 4.5), 0x202020);
        let c = lift(0x333333, 0xffffff, &[0x000000], 4.5);
        assert!(contrast(c, 0x000000) >= 4.5 && contrast(mix(0x333333, 0xffffff, 0.0), 0) < 4.5);
    }

    #[test]
    fn accent_garbage_falls_back_in_order_then_to_none() {
        assert_eq!(resolve_accent(None, None, None), (None, Source::Default, vec![]));
        let (c, src, warn) = resolve_accent(Some("not a colour"), None, None);
        assert_eq!((c, src, warn.len()), (None, Source::Default, 1));
        let (c, src, warn) = resolve_accent(Some("#zz0000"), Some("bogus"), Some("#2d7bff"));
        assert_eq!((c, src, warn.len()), (Some(0x2d7bff), Source::Settings, 2));
        assert_eq!(resolve_accent(Some("#f4b400"), Some("#2d7bff"), Some("#35f2ff")).1, Source::Flag);
        assert_eq!(resolve_accent(None, Some("#2d7bff"), Some("#35f2ff")), (Some(0x2d7bff), Source::Env, vec![]));
        let long = "x".repeat(10_000);
        assert!(resolve_accent(Some(&long), None, None).2[0].len() < 120);
    }

    /// The copy is agent-runtime's: spot values from its palettes and the
    /// spec's nord column.
    #[test]
    fn the_palettes_are_the_tuis() {
        let names: Vec<&str> = palettes::THEMES.iter().map(|t| t.0).collect();
        assert_eq!(names.len(), 20);
        for n in ["default", "nord", "catppuccin", "night-city", "tokyo-night", "paper", "solarized-dark", "rose-pine"] {
            assert!(names.contains(&n), "{n}");
        }
        assert!(!names.contains(&"myx"), "myx is out of scope");
        let nord = builtin("nord").unwrap().1;
        assert_eq!((nord.bg, nord.message_bg, nord.user_bg, nord.code_bg), (0x101216, 0x07080a, 0x16191e, 0x121419));
        assert_eq!((nord.prompt_fg, nord.claude_label, nord.warning_color, nord.tool_input_bg), (0x81a1c1, 0x88c0d0, 0xebcb8b, None));
        assert!(builtin("night_city").is_none(), "exact names, as the TUI");
        let nc = builtin("night-city").unwrap();
        assert_eq!((nc.0, nc.1.tool_bash, nc.1.prompt_fg), ("night-city", Some(0x6cf07a), 0xff2e88));
        // The default palette sets its tool panels explicitly.
        assert_eq!(palettes::THEMES[0].1.tool_input_bg, Some(0x171c28));
        assert!(palettes::SOURCE.starts_with("agent-runtime "));
    }

    /// Nord resolves to the spec's surfaces and lifted roles (section 2).
    #[test]
    fn nord_matches_the_spec() {
        let (r, _) = resolve(&builtin("nord").unwrap().1, None);
        assert_eq!((r.chrome, r.canvas, r.user), (0x101216, 0x07080a, 0x16191e));
        assert_eq!(r.code, 0x151618, "code_bg lifted off the canvas");
        assert_eq!((r.tool, r.tool_out, r.card), (0x191a1d, 0x242629, 0x292b30));
        // `edge` lifted to 1.27 (design.md §1.6); `hair` already stands 1.14 off.
        assert_eq!((r.hair, r.edge), (0x1c2028, 0x292e38));
        assert_eq!(r.btn_hi, 0x8facc8);
        assert_eq!((r.ink, r.prose, r.you, r.agent), (0xeceff4, 0xd8dee9, 0x81a1c1, 0x88c0d0));
        assert_eq!((r.think, r.dim), (0x868e9e, 0x707889));
        assert_eq!((r.warn, r.run, r.cost), (0xebcb8b, 0xa3be8c, 0xa3be8c));
        // design.md §1.6, as `roles_check.py` computes them.
        // Q1 cards: the washes stand off the well's ground, nord's `code`.
        assert_eq!(r.well, r.code);
        assert_eq!((r.add_bg, r.del_bg, r.you_mark_t), (0x1c2023, 0x261e20, 0x81a1c1));
    }

    /// No outline on a card (Haseeb, 07:18): `card_d` reads by its fill
    /// alone, [`D_FILL`] off the canvas, in every theme and every accent
    /// (the card is the canvas's: the accent never moves it). Each theme's
    /// card and its ratio off the canvas, as #71's table gives them. Paper's
    /// white is short of it (1.072), so paper's card steps toward the ink.
    #[test]
    fn a_card_stands_off_the_canvas_by_its_fill_alone() {
        let cards = [
            ("default", 0x171921, 1.107),
            ("night-city", 0x17181f, 1.110),
            ("neon-rain", 0x17141c, 1.107),
            ("amber", 0x191510, 1.101),
            ("phosphor", 0x08190c, 1.109),
            ("solarized-dark", 0x0d3843, 1.101),
            ("blood", 0x191111, 1.102),
            ("ocean", 0x10161e, 1.105),
            ("rose-pine", 0x1b181e, 1.120),
            ("nord", 0x151618, 1.107),
            ("dracula", 0x19171e, 1.108),
            ("monokai", 0x21221e, 1.104),
            ("gruvbox", 0x292827, 1.107),
            ("catppuccin", 0x1e1f2c, 1.109),
            ("tokyo-night", 0x1b1c25, 1.111),
            ("sunset", 0x211413, 1.108),
            ("ice", 0x11161c, 1.104),
            ("forest", 0x121a0e, 1.107),
            ("lavender", 0x1a1523, 1.108),
            ("paper", 0xeaecee, 1.105),
        ];
        assert_eq!(cards.len(), palettes::THEMES.len());
        for ((name, raw), (want_name, want, ratio)) in palettes::THEMES.iter().zip(cards) {
            assert_eq!(*name, want_name);
            for a in std::iter::once(None).chain(PRESETS.iter().map(|p| Some(p.1))) {
                let (r, _) = resolve(raw, a);
                let c = contrast(r.card_d, r.canvas);
                assert_eq!(r.card_d, want, "{name} {a:?}: card_d #{:06x} ({c:.3})", r.card_d);
                assert!(c >= D_FILL - 1e-9 && (c - ratio).abs() < 5e-4, "{name} {a:?}: {c:.4} off the canvas");
            }
        }
        let paper = resolve(&palettes::THEMES.iter().find(|t| t.0 == "paper").expect("paper").1, None).0;
        assert!(contrast(0xffffff, paper.canvas) < D_FILL && luminance(paper.card_d) < luminance(paper.canvas));
    }

    /// The message box has no outline (Haseeb, as the cards since #71): its
    /// fill, `user`, stands [`SURFACE`] (the cards' [`D_FILL`]) or more off
    /// the canvas it sits on in every theme and accent preset, the theme's
    /// `user_bg` where it does, else the canvas a step toward the answer's
    /// colour. Each theme's, as the PR's table gives it.
    #[test]
    fn the_message_box_stands_off_the_canvas_by_its_fill_alone() {
        let boxes = [
            ("default", 0x171920, 1.106),
            ("night-city", 0x16181e, 1.107),
            ("neon-rain", 0x17141b, 1.106),
            ("amber", 0x191611, 1.109),
            ("phosphor", 0x08190c, 1.109),
            ("solarized-dark", 0x0d3843, 1.101),
            ("blood", 0x191212, 1.109),
            ("ocean", 0x0e161e, 1.101),
            ("rose-pine", 0x1a171e, 1.110),
            ("nord", 0x16191e, 1.137),
            ("dracula", 0x19171e, 1.108),
            ("monokai", 0x272822, 1.189),
            ("gruvbox", 0x32302f, 1.241),
            ("catppuccin", 0x313244, 1.439),
            ("tokyo-night", 0x24283b, 1.293),
            ("sunset", 0x211413, 1.108),
            ("ice", 0x11161d, 1.105),
            ("forest", 0x121a0e, 1.107),
            ("lavender", 0x1a1523, 1.108),
            ("paper", 0xe0e6ee, 1.172),
        ];
        assert_eq!(boxes.len(), palettes::THEMES.len());
        const { assert!(SURFACE >= D_FILL) };
        for ((name, raw), (want_name, want, ratio)) in palettes::THEMES.iter().zip(boxes) {
            assert_eq!(*name, want_name);
            for a in std::iter::once(None).chain(PRESETS.iter().map(|p| Some(p.1))) {
                let (r, _) = resolve(raw, a);
                let c = contrast(r.user, r.canvas);
                assert_eq!(r.user, want, "{name} {a:?}: user #{:06x} ({c:.3})", r.user);
                assert!(c >= SURFACE - 1e-9 && (c - ratio).abs() < 5e-4, "{name} {a:?}: {c:.4} off the canvas");
            }
        }
    }

    /// A worker reads as a worker, not a tool (Haseeb): its chip's fill is
    /// the card turned [`D_AGENT`] (OKLab) toward the worker's hue at the
    /// card's own luminance (between the card's and its hover's: a
    /// material, not a step), its hover the card's hover turned the same
    /// way, in every theme and accent preset; every text and mark a worker
    /// chip draws reads on both. Its hue is the theme's worker colour, as
    /// its TUI paints a subagent (Haseeb: the colours match the themes):
    /// `subagent_name`, whatever the accent; violet only for phosphor,
    /// whose green turns its own green card out of gamut. Each theme's pair
    /// and its source, as the PR's table gives it.
    #[test]
    fn a_worker_reads_as_a_worker() {
        let fills = [
            ("default", "subagent_name", 0x17172e, 0x1e1e36),
            ("night-city", "subagent_name", 0x1b152b, 0x221d34),
            ("neon-rain", "subagent_name", 0x230f1c, 0x2b1625),
            ("amber", "subagent_name", 0x211300, 0x2a1a03),
            ("phosphor", "violet", 0x0e1719, 0x10201e),
            ("solarized-dark", "subagent_name", 0x063753, 0x0f3c58),
            ("blood", "subagent_name", 0x250b0a, 0x2f1312),
            ("ocean", "subagent_name", 0x001921, 0x002129),
            ("rose-pine", "subagent_name", 0x27131d, 0x2f1925),
            ("nord", "subagent_name", 0x091725, 0x111f2d),
            ("dracula", "subagent_name", 0x1c142a, 0x241c32),
            ("monokai", "subagent_name", 0x2f1d1c, 0x372323),
            ("gruvbox", "subagent_name", 0x36251a, 0x3d2a1e),
            ("catppuccin", "subagent_name", 0x231c38, 0x2a2240),
            ("tokyo-night", "subagent_name", 0x1e1932, 0x251f39),
            ("sunset", "subagent_name", 0x2c0f07, 0x36150b),
            ("ice", "subagent_name", 0x031728, 0x081f31),
            ("forest", "subagent_name", 0x091d02, 0x0f2405),
            ("lavender", "subagent_name", 0x1e112f, 0x251938),
            ("paper", "subagent_name", 0xe7ebfe, 0xdee2fb),
        ];
        assert_eq!(fills.len(), palettes::THEMES.len());
        let between = |c: u32, x: u32, y: u32| (luminance(x).min(luminance(y))..=luminance(x).max(luminance(y))).contains(&luminance(c));
        for ((name, raw), (want_name, want_from, fill, hi)) in palettes::THEMES.iter().zip(fills) {
            assert_eq!(*name, want_name);
            for a in std::iter::once(None).chain(PRESETS.iter().map(|p| Some(p.1))) {
                let (r, _) = resolve(raw, a);
                // The theme's, the same in every accent.
                assert_eq!((r.card_agent, r.hover_agent), (fill, hi), "{name} {a:?}: #{:06x} #{:06x}", r.card_agent, r.hover_agent);
                assert_eq!(agent(r.card_d, r.hover, raw), (fill, hi, want_from), "{name} {a:?}: its hue's source");
                let gamut = |h: f64| agent_tint(r.card_d, r.hover, h).is_some() && agent_tint(r.hover, r.card_d, h).is_some();
                match agent_sources(raw).into_iter().find(|s| s.0 == want_from) {
                    Some((_, c)) => assert!(hue(c.expect("set")).1 >= HUE_CHROMA, "{name}: {want_from} has a hue"),
                    None => assert!(
                        agent_sources(raw).iter().all(|(_, c)| c.is_none_or(|c| hue(c).1 < HUE_CHROMA || !gamut(hue(c).0))),
                        "{name}: {want_from}, where the theme's worker colour would do"
                    ),
                }
                for (what, c, card, other) in
                    [("card_agent", r.card_agent, r.card_d, r.hover), ("hover_agent", r.hover_agent, r.hover, r.card_d)]
                {
                    let off = ok_distance(c, card);
                    assert!((D_AGENT_MIN - 1e-9..=D_AGENT + 0.005).contains(&off), "{name} {a:?}: {what} #{c:06x} ΔE {off:.4}");
                    assert!(between(c, card, other), "{name} {a:?}: {what} #{c:06x} at its card's luminance");
                }
                assert!(contrast(r.card_agent, r.canvas) >= D_FILL - 1e-9, "{name} {a:?}: off the canvas");
                assert!(contrast(r.hover_agent, r.card_agent) >= D_CARD - 1e-9, "{name} {a:?}: its hover");
                for w in [r.wash_wait, r.wash_wait_pk, r.wash_fail, r.wash_fail_pk] {
                    let d = ok_distance(r.card_agent, w);
                    assert!(d >= D_AGENT_MIN, "{name} {a:?}: #{:06x} reads as the wash #{w:06x} (ΔE {d:.4})", r.card_agent);
                }
                for g in [r.card_agent, r.hover_agent] {
                    for c in [r.prose_t, r.dim_t, r.sub_name_t, r.sub_status_t, r.sub_time_t, r.err, r.ok, r.acc, r.warn, r.run, r.ink] {
                        assert!(contrast(c, g) >= TEXT - 1e-9, "{name} {a:?}: #{c:06x} on #{g:06x} is {:.2}", contrast(c, g));
                    }
                    for c in [r.sub_rule, r.you_mark_t, r.run_mark, r.warn_mark] {
                        assert!(contrast(c, g) >= MARK - 1e-9, "{name} {a:?}: mark #{c:06x} on #{g:06x} is {:.2}", contrast(c, g));
                    }
                }
            }
        }
    }

    /// Cards D's roles (cards4/proposal.md §1) meet their floors in every
    /// theme and accent preset: the card a surface step off the canvas, its
    /// hover and well off it, `line` on all three; every text a row
    /// draws on the card, its hover and the fail wash (at rest and at the
    /// flush's 18 % peak, "Exit N"); `acc` on the card, the Ask pill and
    /// the waiting wash; the Allow fill on the card and its text on it.
    #[test]
    fn cards_d_roles_meet_their_floors() {
        for (name, raw) in palettes::THEMES {
            for a in std::iter::once(None).chain(PRESETS.iter().map(|p| Some(p.1))) {
                let (r, _) = resolve(&raw, a);
                let at = |c: u32, g: &[u32], f: f64, what: &str| {
                    for &g in g {
                        assert!(contrast(c, g) >= f - 1e-9, "{name} {a:?}: {what} #{c:06x} on #{g:06x} is {:.3} < {f}", contrast(c, g));
                    }
                };
                at(r.card_d, &[r.canvas], D_FILL, "card_d");
                at(r.hover, &[r.card_d], D_CARD, "hover");
                at(r.well_d, &[r.card_d], D_CARD, "well_d");
                at(r.line, &[r.card_d, r.canvas, r.hover], D_LINE, "line");
                // Every text on the grounds as painted (Shady #23 M3): the
                // card, its hover, and each wash from the floored colours.
                let [on_fail, on_wait, on_ask, on_out] = d_wash_texts(&r);
                let named = |c: u32| format!("#{c:06x}");
                for c in &on_fail {
                    at(*c, &[r.card_d, r.hover, r.wash_fail, r.wash_fail_pk], TEXT, &format!("{} on the fail washes", named(*c)));
                }
                for c in &on_wait {
                    at(*c, &[r.card_d, r.wash_wait, r.wash_wait_pk], TEXT, &format!("{} on the wait washes", named(*c)));
                }
                for c in &on_ask {
                    at(*c, &[r.ask_flush], TEXT, "on the authorize card's flush");
                }
                for c in &on_out {
                    at(*c, &[r.out_press], TEXT, "the outlined Allow, pressed");
                }
                // Each is the proposal's own where that reads, never louder,
                // and the flush's peak never under its rest.
                assert_eq!(r.wash_fail, mix(r.card_d, r.err, f64::from(D_FAIL_REST)), "{name} {a:?}: the fail rest reads as specified");
                assert!(contrast(r.wash_fail_pk, r.card_d) >= contrast(r.wash_fail, r.card_d) - 1e-9, "{name} {a:?}");
                assert!(contrast(r.wash_fail_pk, r.card_d) <= contrast(mix(r.card_d, r.err, f64::from(D_FAIL_PEAK)), r.card_d) + 1e-9);
                assert!(contrast(r.wash_wait_pk, r.card_d) >= contrast(r.wash_wait, r.card_d) - 1e-9, "{name} {a:?}");
                at(r.res, &[r.well_d], TEXT, "res");
                at(r.dim_o, &[r.well_d], TEXT, "dim_o");
                at(r.acc, &[r.card_d, r.hover, r.acc_soft, r.wash_wait, r.wash_wait_pk], TEXT, "acc");
                at(r.acc_ink, &[r.btn_fill, r.btn_hi], TEXT, "acc_ink");
                at(r.acc_line, &[r.card_d], D_ACC_LINE, "acc_line");
                at(r.acc_soft, &[r.card_d], D_SOFT_OFF, "acc_soft");
                at(r.btn_fill, &[r.card_d], MARK, "the Allow fill");
            }
        }
    }

    /// The diff washes stand 1.10:1 off the well's ground (audit E2: they
    /// were up to 2.02:1 off the canvas against the card's 1.24–1.48), and
    /// off the canvas at most 0.15 louder than the card, but in
    /// solarized-dark (+0.21: its card is the quietest); text on them still
    /// reads.
    #[test]
    fn diff_washes_sit_at_card_level() {
        let mut accents: Vec<Option<u32>> = vec![None];
        accents.extend(PRESETS.iter().map(|p| Some(p.1)));
        for (name, raw) in palettes::THEMES {
            for &accent in &accents {
                let (r, _) = resolve(&raw, accent);
                // Q1 cards: the washes are a well's, off its ground (`code`,
                // paper's `tool_out`), so quieter off the canvas than they
                // were off the run's open body.
                assert!(r.well == r.code || r.well == r.tool_out, "{name}");
                for wash in [r.add_bg, r.del_bg] {
                    let off = contrast(wash, r.canvas);
                    assert!(contrast(wash, r.well) >= SURFACE - 1e-9, "{name}");
                    // Q1 measured 7.32 at the least (`roles_check.py`).
                    assert!(contrast(r.ink, wash) >= 7., "{name}: ink on a wash {:.2}", contrast(r.ink, wash));
                    let over = off - contrast(r.card, r.canvas);
                    let most = if name == "solarized-dark" { 0.22 } else { 0.15 };
                    assert!(over <= most, "{name}: {over:.3} louder than the card");
                }
                assert!(contrast(r.ok_o, r.add_bg) >= TEXT && contrast(r.err_o, r.del_bg) >= TEXT, "{name}");
            }
        }
    }

    /// Event rows ([`crate::events::roles`]): each severity's label and
    /// mark, ASK, the meta and the text, on the canvas, meet their floors in
    /// all 20 themes, without an accent and with every preset and the odd
    /// ones; high and critical never read as the quiet ones.
    #[test]
    fn event_rows_draw_every_severity_above_its_floor() {
        use synaps_client::events::Severity;
        let mut accents: Vec<Option<u32>> = vec![None];
        accents.extend(PRESETS.iter().map(|p| Some(p.1)));
        accents.extend([Some(0x0a1a40), Some(0x050505), Some(0xffffff), Some(0x777777)]);
        let mut bad = Vec::new();
        for (name, raw) in palettes::THEMES {
            for &accent in &accents {
                let (r, _) = resolve(&raw, accent);
                let p = Palette::of(&r);
                let want = [
                    (Severity::Low, r.dim, r.dim),
                    (Severity::Medium, r.dim, r.dim),
                    (Severity::High, r.warn, r.warn_mark),
                    (Severity::Critical, r.err, r.err),
                ];
                for (s, tone, mark) in want {
                    assert_eq!(crate::events::roles(s, &p), (hsla(tone), hsla(mark)), "{name} {s:?}");
                    for (what, c, floor) in [("label", tone, TEXT), ("mark", mark, MARK)] {
                        let k = contrast(c, r.canvas);
                        if k < floor - 1e-9 {
                            bad.push(format!("{name} {accent:?} {s:?} {what}: #{c:06x} on canvas {k:.3} < {floor}"));
                        }
                    }
                }
                for (what, c) in [("ASK", r.you), ("text", r.prose), ("meta", r.dim)] {
                    if contrast(c, r.canvas) < TEXT - 1e-9 {
                        bad.push(format!("{name} {accent:?} {what}"));
                    }
                }
                for (what, a, b) in [
                    ("high label", r.warn, r.dim),
                    ("critical label", r.err, r.dim),
                    ("high mark", r.warn_mark, r.dim),
                    ("critical mark", r.err, r.dim),
                ] {
                    if a == b {
                        bad.push(format!("{name} {accent:?}: {what} #{a:06x} is the quiet one's"));
                    }
                }
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    /// Q1 cards: every pair the rows, wells, spawn rows and chips draw, as
    /// drawn (a colour on the ground it is painted over), meets its floor
    /// in every theme and accent, whatever grounds its role was floored on
    /// (Shady M1, M4, M5: a rule on its own box, OPEN on a hovered row).
    #[test]
    fn q1_draws_every_pair_above_its_floor() {
        let mut accents: Vec<Option<u32>> = vec![None];
        accents.extend(PRESETS.iter().map(|p| Some(p.1)));
        let mut bad = Vec::new();
        for (name, raw) in palettes::THEMES {
            for &accent in &accents {
                let (r, _) = resolve(&raw, accent);
                // The row on the canvas, hovered on `tool_out`; a chip too
                // (no fill since the finish gate; `tool` before it).
                let row = [r.canvas, r.tool_out];
                let chip = [r.tool, r.tool_out];
                let verbs = [r.t_bash, r.t_read, r.t_write, r.t_edit, r.t_grep, r.t_find, r.t_ls, r.t_sub, r.t_ext, r.t_gen];
                let mut pairs: Vec<(&str, u32, Vec<u32>, f64)> = vec![
                    ("hover fill", r.tool_out, vec![r.canvas], SURFACE),
                    ("well", r.well, vec![r.canvas], SURFACE),
                    ("well frame and header rule", r.hair_w, vec![r.well, r.canvas, r.tool_out], HAIR),
                    ("chip frame", r.hair_t, vec![r.tool, r.tool_out, r.canvas], HAIR),
                    ("OPEN outline", r.edge_t, row.to_vec(), EDGE),
                    ("OPEN outline hovered", r.frame, vec![r.tool_out], FRAME),
                    ("OPEN word hovered", r.prose_t, vec![r.tool_out], TEXT),
                    ("well text", r.res, vec![r.well], TEXT),
                    ("well numbers and header", r.dim_o, vec![r.well], TEXT),
                    ("failed output", r.err_o, vec![r.well, r.del_bg], TEXT),
                    ("added sign", r.ok_o, vec![r.add_bg], TEXT),
                    ("diff line", r.ink, vec![r.add_bg, r.del_bg], TEXT),
                    ("spawn bar, chip bar", r.sub_rule, vec![r.canvas, r.tool, r.tool_out], MARK),
                    ("spawn task", r.dim_t, row.to_vec(), TEXT),
                    ("watched chip bar", r.you_mark_t, vec![r.tool_out], MARK),
                    ("gutter running", r.run_mark, row.to_vec(), MARK),
                    ("gutter waiting", r.warn_mark, row.to_vec(), MARK),
                ];
                for (k, c) in
                    [("target file", r.prose_t), ("target folder", r.arg), ("meta", r.dim_t), ("count +", r.ok), ("count −", r.err)]
                {
                    pairs.push((k, c, row.to_vec(), TEXT));
                }
                for c in verbs {
                    pairs.push(("verb", c, row.to_vec(), TEXT));
                }
                for (k, c) in [("worker name", r.sub_name_t), ("worker step", r.sub_status_t), ("worker clock", r.sub_time_t)] {
                    pairs.push((k, c, [row.as_slice(), chip.as_slice()].concat(), TEXT));
                }
                for (what, c, grounds, floor) in pairs {
                    for g in grounds {
                        let k = contrast(c, g);
                        if k < floor - 1e-9 {
                            bad.push(format!("{name} {accent:?}: {what} #{c:06x} on #{g:06x}: {k:.3} < {floor}"));
                        }
                    }
                }
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    /// Every role meets its floor in all 20 themes, without an accent and
    /// with every preset, a navy, a near-black, white and a mid grey.
    #[test]
    fn every_role_meets_its_floor_in_every_theme() {
        let mut accents: Vec<Option<u32>> = vec![None];
        accents.extend(PRESETS.iter().map(|p| Some(p.1)));
        accents.extend([Some(0x0a1a40), Some(0x050505), Some(0xffffff), Some(0x777777)]);
        let mut report = Vec::new();
        for (name, raw) in palettes::THEMES {
            let (mut lifted, mut total) = (0, 0);
            let mut worst = f64::MAX;
            for &accent in &accents {
                let (roles, checks) = resolve(&raw, accent);
                assert!(checks.len() > 80, "{name}");
                for c in &checks {
                    for &g in &c.grounds {
                        let ratio = contrast(c.color, g);
                        assert!(
                            ratio >= c.floor - 1e-9,
                            "{name} {accent:06x?}: {} {:06x} on {g:06x} is {ratio:.2} < {}",
                            c.role,
                            c.color,
                            c.floor
                        );
                        if accent.is_none() {
                            worst = worst.min(ratio / c.floor);
                        }
                    }
                }
                if accent.is_none() {
                    // How many roles the floors moved off the theme's own colour.
                    lifted = checks.iter().filter(|c| c.color != c.from).count();
                    total = checks.len();
                    let _ = roles;
                }
            }
            report.push(format!("{name:<15} {lifted:>3} of {total} roles lifted, lowest ratio / floor {worst:.3}"));
        }
        println!("{}", report.join("\n"));
    }

    /// Inline code is a chip on the well (Q3): on paper, whose code is a
    /// dark island, a light chip off the canvas with dark text, never the
    /// near-black chip in light prose; elsewhere the code surface, as before.
    #[test]
    fn inline_code_chips_are_the_well() {
        for (name, raw) in palettes::THEMES {
            let (r, checks) = resolve(&raw, None);
            assert!(contrast(r.code_in, r.well) >= TEXT - 1e-9, "{name}: code_in on its chip");
            assert!(contrast(r.well, r.canvas) >= SURFACE - 1e-9, "{name}: the chip stands off the canvas");
            // A user message's inline code sits on its bubble too (Shady L1):
            // Q3's own chip stands off it. (Where the chip is the code
            // surface, as on main, it may be the user surface's colour.)
            if r.well != r.code {
                assert!(contrast(r.well, r.user) >= SURFACE - 1e-9, "{name}: the chip stands off the user bubble");
            }
            // Floored on the chip, whatever the chip is (Shady L2).
            let c = checks.iter().find(|c| c.role == "code_in").expect("code_in is checked");
            assert_eq!(c.grounds, vec![r.well], "{name}: code_in is floored on the well");
            if name == "paper" {
                assert_ne!(r.well, r.code, "paper's chip is not its dark code island");
                assert!(luminance(r.well) > 0.4 && luminance(r.code_in) < 0.2, "paper: a light chip, dark text");
            }
        }
    }

    /// The authorize card's command is its first read (the finish gate of
    /// #18): `CMD` on its well and over every other text on the card.
    #[test]
    fn the_command_is_the_cards_loudest_text() {
        let mut accents: Vec<Option<u32>> = vec![None];
        accents.extend(PRESETS.iter().map(|p| Some(p.1)));
        for (name, raw) in palettes::THEMES {
            for &accent in &accents {
                let (r, _) = resolve(&raw, accent);
                let cmd = contrast(r.well_fg, r.canvas);
                assert!(cmd >= CMD - 1e-9, "{name} {accent:06x?}: the command is {cmd:.2} on its well");
                for (role, c) in [("prose_k", r.prose_k), ("dim_k", r.dim_k), ("warn_k", r.warn_k), ("err_k", r.err_k)] {
                    let other = contrast(c, r.card);
                    assert!(cmd >= other - 1e-9, "{name} {accent:06x?}: the command {cmd:.2} under {role} {other:.2}");
                }
            }
        }
    }

    /// Every role is in the `Check` log (so the floor test covers it), but
    /// the two grounds a theme gives as they are.
    /// Design re-gate #20 (4): the palette's matched letters (`you_k`, the
    /// accent) read as text on the card and on the selected row, with
    /// every accent.
    #[test]
    fn you_k_reads_on_the_card_and_its_selection() {
        for (name, raw) in palettes::THEMES {
            for accent in std::iter::once(None).chain(PRESETS.iter().map(|p| Some(p.1))) {
                let (r, checks) = resolve(&raw, accent);
                let c = checks.iter().find(|c| c.role == "you_k").expect("you_k is checked");
                assert_eq!((c.grounds.clone(), c.floor), (vec![r.card, r.card_hi], TEXT), "{name}");
                assert!(contrast(r.you_k, r.card_hi) >= TEXT - 1e-9 && contrast(r.you_k, r.card) >= TEXT - 1e-9, "{name} {accent:06x?}");
            }
        }
    }

    /// Shady #20 N5: the card's scrollbar thumb is drawn over the palette's
    /// selected row too: it meets its floor on `card_hi` as on `card`.
    #[test]
    fn thumb_k_reads_on_the_card_and_its_selection() {
        for (name, raw) in palettes::THEMES {
            let (r, checks) = resolve(&raw, None);
            let c = checks.iter().find(|c| c.role == "thumb_k").expect("thumb_k is checked");
            assert_eq!(c.grounds, vec![r.card, r.card_hi], "{name}");
            assert!(contrast(r.thumb_k, r.card_hi) >= HAIR, "{name}");
        }
    }

    #[test]
    fn every_role_is_checked() {
        for (name, raw) in palettes::THEMES {
            let (_, checks) = resolve(&raw, None);
            for role in super::ROLE_NAMES.iter().filter(|r| !["canvas", "chrome"].contains(r)) {
                assert!(checks.iter().any(|c| c.role == *role), "{name}: {role} has no floor in the log");
            }
        }
    }

    /// A detached panel's palette (`Palette::faded`, Shady L10) dims every
    /// chrome role: a new `*_ch` role left out of it would stay bright. Not
    /// `dim_ch` (the fade's own ink) nor `thumb_ch` (a scrollbar at rest,
    /// already at the hairline).
    #[test]
    fn a_faded_palette_dims_every_chrome_role() {
        for (name, raw) in palettes::THEMES {
            let p = Palette::of(&resolve(&raw, None).0);
            let f = p.faded();
            for role in super::ROLE_NAMES.iter().filter(|r| r.ends_with("_ch") && !["dim_ch", "thumb_ch", "press_ch"].contains(r)) {
                assert_ne!(p.role(role), f.role(role), "{name}: {role} stays bright on a faded panel");
            }
        }
    }

    /// The accent replaces `prompt_fg` and `border_active` only: no state
    /// colour moves with it.
    #[test]
    fn the_accent_colours_you_and_focus_only() {
        // The pressed fills are held back by the accent's marks drawn on them.
        let allowed = [
            "you",
            "you_mark",
            "you_mark_t",
            "btn_fill",
            "btn_hi",
            "focus",
            "focus_ch",
            "focus_k",
            "you_k",
            "open_ch",
            "you_bar",
            "press_ch",
            "press_k",
            "btn_press",
            // Cards D: the accent's own roles (cards4/proposal.md §1).
            "acc",
            "acc_soft",
            "acc_line",
            "acc_ink",
            // ... and its washes as painted (Shady #23 M3).
            "wash_wait",
            "wash_wait_pk",
            "ask_flush",
            "out_press",
        ];
        for (name, raw) in palettes::THEMES {
            let (a, base) = resolve(&raw, None);
            for &(_, accent) in PRESETS {
                let (b, checks) = resolve(&raw, Some(accent));
                for (c, before) in checks.iter().zip(&base).filter(|(c, _)| !allowed.contains(&c.role)) {
                    assert_eq!((before.role, before.color), (c.role, c.color), "{name}: {} moved with the accent {accent:06x}", c.role);
                }
                assert_eq!((a.warn, a.run, a.err, a.ready), (b.warn, b.run, b.err, b.ready), "{name}");
                assert_ne!(a.you_mark, b.you_mark, "{name}: {accent:06x}");
            }
        }
    }

    #[test]
    fn themes_resolve_in_order_and_garbage_falls_through() {
        let (n, _, src, w) = resolve_theme(None, None, None, None, None);
        assert_eq!((n.as_str(), src, w.len()), ("nord", Source::Default, 0));
        let amber = || Some(("amber".to_string(), builtin("amber").unwrap().1));
        let (n, raw, src, _) = resolve_theme(Some(" tokyo-night"), Some("paper"), Some("ocean"), amber(), None);
        assert_eq!((n.as_str(), src, raw), ("tokyo-night", Source::Flag, builtin("tokyo-night").unwrap().1));
        let (n, _, src, w) = resolve_theme(Some("tokyo_night"), Some("paper"), None, None, None);
        assert_eq!((n.as_str(), src, w.len()), ("paper", Source::Env, 1));
        let (n, _, src, w) = resolve_theme(Some("../etc/passwd"), Some("nope"), None, amber(), None);
        assert_eq!((n.as_str(), src, w.len()), ("amber", Source::Config, 2));
        assert_eq!(resolve_theme(None, None, Some("catppuccin"), amber(), None).2, Source::Settings);
        assert_eq!(resolve_theme(None, Some("dracula"), Some("catppuccin"), None, None).2, Source::Env);
        for bad in ["", ".hidden", "a/b", "x".repeat(65).as_str(), "myx"] {
            assert!(lookup(bad, None).is_none(), "{bad:?}");
        }
    }

    /// Read as the TUI's `load_theme_from_config` reads it: the first
    /// `theme =` line that names a theme, its value only trimmed.
    #[test]
    fn the_config_names_the_theme() {
        let name = |c: &str| config_theme(c, |n| lookup(n, None)).0.map(|t| t.0);
        assert_eq!(name("model = x\ntheme = nord\ntheme_transition = on\n").as_deref(), Some("nord"));
        assert_eq!(name("# theme = paper\ntheme=ocean\n").as_deref(), Some("ocean"));
        assert_eq!(name("  theme =   rose-pine  \n").as_deref(), Some("rose-pine"));
        assert_eq!(name("theme = ocean\ntheme = paper").as_deref(), Some("ocean"), "the first that resolves");
        assert_eq!(name("theme = nope\ntheme = paper").as_deref(), Some("paper"), "a bad one is skipped");
        assert_eq!(config_theme("theme = nope\ntheme = paper", |n| lookup(n, None)).1.len(), 1, "and reported");
        // The TUI shows `default` for these, so the desktop finds none here.
        for c in ["theme = \"nord\"", "theme = 'nord'", "theme = nord # mine", "theme=night_city", "theme =", "", "themes = nord"] {
            assert_eq!(name(c), None, "{c:?}");
        }
    }

    /// `<base>/themes/<name>` wins over the built-in; a profile's config
    /// wins over the root one, only when it exists.
    #[test]
    fn theme_files_and_profiles() {
        let dir = std::env::temp_dir().join(format!("synaps-desktop-themes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("themes")).unwrap();
        std::fs::write(
            dir.join("themes").join("nord"),
            "# mine\nbg = #000000\nprompt_fg = \"#ff0000\"\nnot_a_role = #123456\nmuted = garbage\n",
        )
        .unwrap();
        let raw = lookup("nord", Some(&dir)).unwrap();
        let default = palettes::THEMES[0].1;
        assert_eq!((raw.bg, raw.prompt_fg, raw.muted, raw.message_bg), (0x000000, 0xff0000, default.muted, default.message_bg));
        assert_eq!(lookup("tokyo-night", Some(&dir)), builtin("tokyo-night").map(|t| t.1));
        std::fs::write(dir.join("themes").join("custom"), "bg = #010203\n").unwrap();
        std::fs::write(dir.join("config"), "theme = custom\n").unwrap();
        assert_eq!(synaps_theme(&dir, None).0.map(|t| (t.0, t.1.bg)), Some(("custom".into(), 0x010203)));
        let config = |p: Option<&str>| read_path(&dir, p, "config");
        assert_eq!(config(Some("work")), dir.join("config"));
        std::fs::create_dir_all(dir.join("work")).unwrap();
        std::fs::write(dir.join("work").join("config"), "theme = paper\n").unwrap();
        assert_eq!(config(Some("work")), dir.join("work").join("config"));
        assert_eq!(config(Some("../x")), dir.join("config"));
        assert_eq!(config(None), dir.join("config"));
        assert_eq!(synaps_theme(&dir, Some("work")).0.map(|t| t.0), Some("paper".into()));
        // `$SYNAPS_THEME` first, as in the TUI; then the config.
        assert_eq!(follow_config(Some("dracula"), &dir, Some("work")).0, "dracula");
        assert_eq!(follow_config(None, &dir, Some("work")).2, Source::Config);
        // A `theme` file is a palette over everything in the config.
        std::fs::write(dir.join("theme"), "bg = #0a0b0c\n").unwrap();
        let (n, raw, src, _) = follow_config(None, &dir, Some("work"));
        assert_eq!((n.as_str(), raw.bg, src), (THEME_FILE, 0x0a0b0c, Source::Config));
        std::fs::write(dir.join("work").join("theme"), "bg = #0d0e0f\n").unwrap();
        assert_eq!(follow_config(None, &dir, Some("work")).1.bg, 0x0d0e0f, "the profile's own theme file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_steps_scale_the_centre_pane() {
        let m = Metrics::at(16);
        assert_eq!((m.body, m.body_line, m.column, m.mono, m.row), (16., 26., 672., 13., 32.));
        // The edges in a run: 16 and 36 (design.md §1.1); a tray row 24.
        assert_eq!((m.pad_x, m.pad_x + m.lead, m.row_compact, m.summary), (16., 36., 24., 36.));
        assert_eq!(Metrics::at(18).column, 756., "the column grows with the text");
        assert_eq!(Metrics::at(99), m, "an unknown step is the default");
        for s in TEXT_STEPS {
            let m = Metrics::at(s);
            for v in [m.body, m.mono, m.meta, m.label, m.card_title, m.card_lead, m.think, m.table] {
                assert!(v >= 12., "{s}: {v}");
            }
        }
        assert_eq!(Metrics::bump(16, 1), Some(17));
        assert_eq!(Metrics::bump(18, 1), Some(20));
        assert_eq!(Metrics::bump(22, 1), None);
        assert_eq!(Metrics::bump(14, -1), None);
        assert_eq!(Metrics::bump(15, -1), Some(14));
    }

    #[test]
    fn code_highlighting_deserializes_in_every_theme() {
        for (name, raw) in palettes::THEMES {
            assert!(highlight_theme(&resolve(&raw, None).0).is_some(), "{name}");
        }
    }

    #[test]
    fn the_scheme_is_derived_once_and_the_palette_is_copy() {
        let choice = Choice { theme: "nord".into(), raw: builtin("nord").unwrap().1, accent: None, text: 16 };
        let a = Scheme::new(choice.clone());
        let p = a.p;
        assert_eq!(p, Palette::of(&resolve(&choice.raw, None).0));
        let b = Scheme::new(Choice { accent: Some(0x2d7bff), ..choice });
        assert_ne!(b.p.you_mark, p.you_mark);
        assert_eq!(b.p.warn, p.warn);
    }

    /// An accent saved by the accent-only picker is the override now; the
    /// picker's "theme" choice saves `null`, which means none.
    #[test]
    fn a_saved_accent_carries_over_as_the_override() {
        let dir = std::env::temp_dir().join(format!("synaps-desktop-accent-{}", std::process::id()));
        let path = dir.join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, r##"{"rail_collapsed": true, "accent": "#2d7bff"}"##).unwrap();
        assert_eq!(resolve_accent(None, None, load_accent(&path).as_deref()), (Some(0x2d7bff), Source::Settings, vec![]));
        save_accent(&path, None).unwrap();
        assert_eq!(resolve_accent(None, None, load_accent(&path).as_deref()).0, None);
        assert!(load_panels(&path).rail_collapsed, "other keys kept");
        save_theme(&path, Some("catppuccin")).unwrap();
        assert_eq!(load_theme(&path).as_deref(), Some("catppuccin"));
        // SYNAPS CONFIG: `null`, which is no saved theme (the config's wins).
        save_theme(&path, None).unwrap();
        assert_eq!(load_theme(&path), None);
        assert_eq!(load_settings(&path).unwrap()["theme"], serde_json::Value::Null);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The theme (UI thread) and the panels (background) save at the same
    /// time: each merges its keys into what the other wrote, none is lost.
    #[test]
    fn concurrent_saves_lose_no_key() {
        let dir = std::env::temp_dir().join(format!("synaps-desktop-race-{}", std::process::id()));
        let path = dir.join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);
        let writers: Vec<_> = (0..8)
            .map(|w| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for i in 0..20 {
                        save_settings(
                            &path,
                            &[(
                                [
                                    "theme",
                                    "accent",
                                    "rail_collapsed",
                                    "right_collapsed",
                                    "rail_width",
                                    "right_width",
                                    "history_open",
                                    "text_size",
                                ][w],
                                i.into(),
                            )],
                        )
                        .unwrap();
                    }
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        let v = load_settings(&path).unwrap();
        assert_eq!(v.as_object().map(|o| o.len()), Some(8), "{v}");
        assert!(v.as_object().unwrap().values().all(|x| x == 19), "{v}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Favourites are kept per daemon (the tabs' keys): one daemon's list
    /// never shows on another, saving one keeps the others, the tabs and
    /// every key this build does not know; a list survives a write of
    /// anything else.
    #[test]
    fn favourites_round_trip_per_daemon_keeping_unknown_keys() {
        use crate::favourites::{Favourite, Favourites, Source};
        let dir = std::env::temp_dir().join(format!("synaps-desktop-favs-{}", std::process::id()));
        let path = dir.join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            &path,
            r##"{"accent": "#2d7bff", "from_the_future": {"x": [1, 2]}, "tabs": {"socket:/a": {"sessions": ["s1"], "active": 0}}}"##,
        )
        .unwrap();
        assert!(load_favourites(&path, "socket:/a").is_empty(), "none yet");
        let saved = |id: &str, cwd: &str| -> synaps_client::SavedSession {
            serde_json::from_value(
                serde_json::json!({"id": id, "title": format!("t {id}"), "cwd": cwd, "updated_at": "2026-01-02T03:04:05Z"}),
            )
            .unwrap()
        };
        let mut a = Favourites::default();
        a.toggle(Source::Saved(&saved("j-old", "/home/u/deep")), 10);
        a.toggle(Source::Saved(&saved("j-new", "/home/u/new")), 20);
        let b = vec![Favourite::of(Source::Saved(&saved("j-b", "/srv/b")), 30)];
        assert_eq!(save_favourites(&path, "socket:/a", &[], &a.list).unwrap(), a.list);
        save_favourites(&path, "socket:/b", &[], &b).unwrap();
        assert_eq!(load_favourites(&path, "socket:/a"), a.list, "order and snapshot kept");
        assert_eq!(load_favourites(&path, "socket:/b"), b, "another daemon's apart");
        assert!(load_favourites(&path, "offline").is_empty());
        // Everything else stays: unknown keys, the tabs, the accent.
        save_settings(&path, &[("theme", "nord".into())]).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["from_the_future"], serde_json::json!({"x": [1, 2]}));
        assert_eq!(v["tabs"]["socket:/a"]["sessions"], serde_json::json!(["s1"]));
        assert_eq!(load_accent(&path).as_deref(), Some("#2d7bff"));
        assert_eq!(load_favourites(&path, "socket:/a"), a.list, "a write of another key keeps them");
        assert_eq!(load_tabs(&path, "socket:/a").map(|t| t.sessions), Some(vec!["s1".to_string()]));
        // Unfavourited: an empty list, not the daemon's key gone with the others.
        assert!(save_favourites(&path, "socket:/a", &a.list, &[]).unwrap().is_empty());
        assert!(load_favourites(&path, "socket:/a").is_empty());
        assert_eq!(load_favourites(&path, "socket:/b"), b);
        // Another instance's add since this one read the file: kept.
        let c = vec![Favourite::of(Source::Saved(&saved("j-c", "/srv/c")), 40)];
        save_favourites(&path, "socket:/b", &b, &[b.clone(), c.clone()].concat()).unwrap();
        let mine = vec![b[0].clone(), Favourite::of(Source::Saved(&saved("j-d", "/srv/d")), 50)];
        let written = save_favourites(&path, "socket:/b", &b, &mine).unwrap();
        assert_eq!(written.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["j-b", "j-c", "j-d"]);
        assert_eq!(load_favourites(&path, "socket:/b"), written);
        // Not an object, or a key's value not a list (a newer build's
        // form, a hand edit): read as none, and never overwritten.
        for odd in [r#"{"favourites": [1, 2]}"#, r#"{"favourites": {"socket:/b": {"v": 2, "list": []}}}"#] {
            std::fs::write(&path, odd).unwrap();
            assert!(load_favourites(&path, "socket:/b").is_empty());
            let e = save_favourites(&path, "socket:/b", &[], &b).unwrap_err();
            assert!(e.to_string().contains("not overwritten"), "{e}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), odd, "left as it was");
        }
        // R2: a file that does not parse (a hand edit's trailing comma) is
        // set aside and says nothing of removals: every favourite this
        // instance has is written, and stays in its list.
        std::fs::write(&path, "{}").unwrap();
        save_favourites(&path, "socket:/b", &[], &mine).unwrap();
        let bad = std::fs::read_to_string(&path).unwrap().replacen('{', "{,", 1);
        std::fs::write(&path, &bad).unwrap();
        let written = save_favourites(&path, "socket:/b", &mine, &mine).unwrap();
        assert_eq!(written, mine, "none dropped as another instance's remove");
        assert_eq!(load_favourites(&path, "socket:/b"), mine);
        assert_eq!(std::fs::read_to_string(path.with_extension("json.bad")).unwrap(), bad, "the hand edit kept");
        let mut list = Favourites::new(mine.clone());
        assert!(!list.written(&mine, written), "the list keeps them");
        assert_eq!(list.list, mine);
        // No `favourites`, or no key for this daemon: the same.
        for none in [r#"{"theme": "nord"}"#, r#"{"favourites": {"socket:/a": []}}"#] {
            std::fs::write(&path, none).unwrap();
            assert_eq!(save_favourites(&path, "socket:/b", &mine, &mine).unwrap(), mine, "{none}");
        }
        // A file there that cannot be read is left as it was.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(&path, r#"{"theme": "nord", "favourites": {"socket:/b": []}}"#).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o200)).unwrap();
            // Root reads it anyway: nothing to check there.
            if std::fs::read(&path).is_err() {
                assert!(save_favourites(&path, "socket:/b", &[], &mine).is_err());
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
                assert_eq!(std::fs::read_to_string(&path).unwrap(), r#"{"theme": "nord", "favourites": {"socket:/b": []}}"#);
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R4: a write waits for another process's lock (advisory, on a file of
    /// its own: two opens conflict as two processes do). N4: never for
    /// longer than its wait: a holder that never lets go (stopped mid-write)
    /// does not freeze it, and a lock file that can't be opened isn't waited
    /// for at all; either way it writes, unlocked. No bound here depends on
    /// how fast the machine is: a wait that was waited out took at least its
    /// length, one not waited for took less than a minute of it.
    #[test]
    fn a_write_waits_for_the_lock() {
        use std::time::{Duration, Instant};
        let dir = std::env::temp_dir().join(format!("synaps-desktop-lock-{}", std::process::id()));
        let path = dir.join("settings.json");
        let lock = path.with_extension("json.lock");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, r#"{"theme": "nord"}"#).unwrap();
        let other = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&lock).unwrap();
        other.lock().unwrap();
        // Held: waited for (a minute's wait: still waiting while it is held)
        // and taken once let go.
        let l = lock.clone();
        let waiter = std::thread::spawn(move || lock_settings(&l, Duration::from_secs(60)));
        std::thread::sleep(Duration::from_millis(100));
        assert!(!waiter.is_finished(), "held: still waiting");
        other.unlock().unwrap();
        assert!(waiter.join().unwrap().is_ok(), "taken once let go");
        // Held for good: not taken, after the whole wait; the write goes
        // ahead, unlocked.
        other.lock().unwrap();
        let t0 = Instant::now();
        let why = lock_settings(&lock, LOCK_WAIT).unwrap_err();
        assert!(t0.elapsed() >= LOCK_WAIT, "{:?}", t0.elapsed());
        assert!(why.contains("not locked"), "{why}");
        save_settings(&path, &[("accent", "#35f2ff".into())]).unwrap();
        assert_eq!(load_accent(&path).as_deref(), Some("#35f2ff"));
        assert_eq!(load_theme(&path).as_deref(), Some("nord"), "merged");
        // A lock file that can't be opened (a directory there: EISDIR,
        // Windows' ERROR_ACCESS_DENIED): not waited for, whatever the wait;
        // the write goes ahead, unlocked.
        drop(other);
        std::fs::remove_file(&lock).unwrap();
        std::fs::create_dir(&lock).unwrap();
        let t0 = Instant::now();
        let why = lock_settings(&lock, Duration::from_secs(60)).unwrap_err();
        assert!(t0.elapsed() < Duration::from_secs(60), "not waited for: {:?}", t0.elapsed());
        assert!(why.contains("not opened"), "{why}");
        save_settings(&path, &[("accent", "#ff2d7b".into())]).unwrap();
        assert_eq!(load_accent(&path).as_deref(), Some("#ff2d7b"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// N5: a write of other keys (the tabs, the theme) that sets a corrupt
    /// file aside carries the favourites this process holds for that file
    /// into the new one (they were in it); the favourites' own write
    /// merges as before. Another file's are not carried.
    #[test]
    fn a_set_aside_carries_the_favourites() {
        use crate::favourites::{Favourite, Source};
        let dir = std::env::temp_dir().join(format!("synaps-desktop-carry-{}", std::process::id()));
        let path = dir.join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let v: synaps_client::SavedSession =
            serde_json::from_value(serde_json::json!({"id": "j1", "title": "Relay", "cwd": "/w"})).unwrap();
        let mine = vec![Favourite::of(Source::Saved(&v), 7)];
        save_favourites(&path, "socket:/b", &[], &mine).unwrap();
        carry_favourites(&path, "socket:/b", &mine);
        carry_favourites(&dir.join("other.json"), "socket:/c", &mine);
        std::fs::write(&path, r#"{"favourites": {"socket:/b": [{"id": "j1"},]}}"#).unwrap();
        save_tabs(&path, "socket:/b", &crate::tabs::Saved::default()).unwrap();
        assert_eq!(load_favourites(&path, "socket:/b"), mine, "carried over");
        assert!(load_tabs(&path, "socket:/b").is_some());
        let file: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(file["favourites"].get("socket:/c").is_none(), "only this file's");
        // Not set aside: nothing carried (another instance's remove stays).
        std::fs::write(&path, r#"{"theme": "nord"}"#).unwrap();
        save_accent(&path, Some(0x2d7bff)).unwrap();
        assert!(load_favourites(&path, "socket:/b").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Tabs are saved per daemon; other keys and other daemons' tabs stay.
    #[test]
    fn tabs_round_trip_per_daemon() {
        use crate::tabs::Saved;
        let dir = std::env::temp_dir().join(format!("synaps-desktop-tabs-{}", std::process::id()));
        let path = dir.join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, r##"{"accent": "#2d7bff", "tabs": 3}"##).unwrap();
        assert_eq!(load_tabs(&path, "socket:/a"), None, "not an object: none");
        let a = Saved {
            sessions: vec!["s1".into(), "s2".into()],
            journals: vec!["".into(), "j2".into()],
            labels: vec!["Fix the relay".into(), "".into()],
            active: Some(1),
        };
        let b = Saved { sessions: vec!["t1".into()], active: Some(0), ..Saved::default() };
        save_tabs(&path, "socket:/a", &a).unwrap();
        save_tabs(&path, "socket:/b", &b).unwrap();
        assert_eq!(load_tabs(&path, "socket:/a"), Some(a));
        assert_eq!(load_tabs(&path, "socket:/b"), Some(b));
        assert_eq!(load_tabs(&path, "offline"), None);
        assert_eq!(load_accent(&path).as_deref(), Some("#2d7bff"), "other keys kept");
        // An active index past the list, or not a number: none visible.
        std::fs::write(&path, r#"{"tabs": {"k": {"sessions": ["x", 7], "active": 4}}}"#).unwrap();
        assert_eq!(load_tabs(&path, "k"), Some(Saved { sessions: vec!["x".into()], ..Saved::default() }));
        // An older file (no journals), or journals that do not line up: none.
        std::fs::write(&path, r#"{"tabs": {"k": {"sessions": ["x", "y"], "journals": ["j"], "active": 1}}}"#).unwrap();
        assert_eq!(load_tabs(&path, "k"), Some(Saved { sessions: vec!["x".into(), "y".into()], active: Some(1), ..Saved::default() }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Split panes step 6: on by default; `"panes.split": false` in the
    /// file, or the run's `SYNAPS_DESK_SPLIT=0`, turns them off (the layout
    /// never read), and `=1` on whatever the file says.
    #[test]
    fn split_panes_are_on_unless_the_file_or_the_run_turns_them_off() {
        use crate::tabs::Saved;
        let dir = std::env::temp_dir().join(format!("synaps-desktop-split-flag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        assert!(load_split(&path), "no file: on");
        let union = Saved { sessions: vec!["s1".into(), "s2".into()], active: Some(1), ..Saved::default() };
        let layout = Layout {
            focused: 1,
            tree: LayoutNode::Split {
                axis: crate::split::Axis::Row,
                ratios: vec![0.5, 0.5],
                kids: vec![LayoutNode::Pane(0), LayoutNode::Pane(1)],
            },
            panes: vec![
                Saved { sessions: vec!["s1".into()], active: Some(0), ..Saved::default() },
                Saved { sessions: vec!["s2".into()], active: Some(0), ..Saved::default() },
            ],
        };
        save_layout(&path, "k", &layout).unwrap();
        assert_eq!(layout.union(), union);
        assert!(load_split(&path), "the key missing: on");
        assert_eq!(load_restore(&path, "k", None), (Some(layout.clone()), Some(union.clone())), "on: the layout");
        assert_eq!(load_restore(&path, "k", Some(false)), (None, Some(union.clone())), "SYNAPS_DESK_SPLIT=0: the tabs alone");
        save_settings(&path, &[("panes.split", false.into())]).unwrap();
        assert!(!load_split(&path), "`false`: off");
        assert_eq!(load_restore(&path, "k", None), (None, Some(union.clone())), "off: the tabs alone");
        assert_eq!(load_restore(&path, "k", Some(true)), (Some(layout), Some(union)), "SYNAPS_DESK_SPLIT=1: on");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// split/architecture.md §5: `layout.<key>` round-trips per daemon,
    /// written with `tabs.<key>` as every pane's tabs, a session once (what
    /// an older build reads: the downgrade reopens them all, in one strip);
    /// a launch reads the layout while the tabs are its union, and the tabs
    /// as one pane once an older build changed them.
    #[test]
    fn layout_round_trips_and_writes_the_tabs_union() {
        use crate::split::Axis;
        use crate::tabs::Saved;
        let dir = std::env::temp_dir().join(format!("synaps-desktop-layout-{}", std::process::id()));
        let path = dir.join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, r##"{"accent": "#2d7bff", "tabs": {"socket:/b": {"sessions": ["t1"], "active": 0}}}"##).unwrap();
        let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let layout = Layout {
            focused: 2,
            tree: LayoutNode::Split {
                axis: Axis::Row,
                ratios: vec![0.5, 0.5],
                kids: vec![
                    LayoutNode::Pane(0),
                    LayoutNode::Split { axis: Axis::Column, ratios: vec![0.6, 0.4], kids: vec![LayoutNode::Pane(1), LayoutNode::Pane(2)] },
                ],
            },
            panes: vec![
                Saved { sessions: s(&["s1", "s2"]), journals: s(&["", "j2"]), labels: s(&["Fix the relay", ""]), active: Some(0) },
                Saved { sessions: s(&["s3"]), active: Some(0), ..Saved::default() },
                Saved { sessions: s(&["s2", "s4"]), journals: s(&["j2", ""]), active: Some(1), ..Saved::default() },
            ],
        };
        save_layout(&path, "socket:/a", &layout).unwrap();
        assert_eq!(load_layout(&path, "socket:/a"), Some(layout.clone()));
        assert_eq!(load_layout(&path, "socket:/b"), None);
        // The downgrade: an older build's read of the tabs.
        let union = load_tabs(&path, "socket:/a").expect("the tabs written beside it");
        assert_eq!(union.sessions, s(&["s1", "s2", "s3", "s4"]), "pane order, a session once");
        assert_eq!(union.journals, s(&["", "j2", "", ""]));
        assert_eq!(union.labels, s(&["Fix the relay", "", "", ""]));
        assert_eq!(union.active, Some(3), "the focused pane's visible tab");
        assert_eq!(load_tabs(&path, "socket:/b").map(|t| t.sessions), Some(s(&["t1"])), "other daemons kept");
        assert_eq!(load_accent(&path).as_deref(), Some("#2d7bff"), "other keys kept");
        assert_eq!(load_restore(&path, "socket:/a", Some(true)), (Some(layout.clone()), Some(union.clone())));
        // Split panes off (Shady #55 n2): the layout is not read.
        assert_eq!(load_restore(&path, "socket:/a", Some(false)), (None, Some(union.clone())), "flag off: the tabs alone");
        // An older build's rename, or its tab switch, the sessions the same
        // (Shady #55 L1): its tabs, not the stale layout.
        let renamed = Saved { labels: s(&["Relay v2", "", "", ""]), ..union.clone() };
        save_tabs(&path, "socket:/a", &renamed).unwrap();
        assert_eq!(load_restore(&path, "socket:/a", Some(true)), (None, Some(renamed)), "a rename since");
        let switched = Saved { active: Some(0), ..union.clone() };
        save_tabs(&path, "socket:/a", &switched).unwrap();
        assert_eq!(load_restore(&path, "socket:/a", Some(true)), (None, Some(switched)), "a tab switch since");
        // An older build since: its tabs, not the stale layout.
        let older = Saved { sessions: s(&["s1", "s5"]), active: Some(1), ..Saved::default() };
        save_tabs(&path, "socket:/a", &older).unwrap();
        assert_eq!(load_restore(&path, "socket:/a", Some(true)), (None, Some(older)));
        // A layout with no tabs beside it (written by hand): the layout.
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let mut v2 = v.clone();
        v2["tabs"].as_object_mut().unwrap().remove("socket:/a");
        std::fs::write(&path, v2.to_string()).unwrap();
        assert_eq!(load_restore(&path, "socket:/a", Some(true)).0, Some(layout));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Shady #55 L2: a saved pane with no session tabs is left out; its
    /// share goes to the panes beside it, its focus to the pane before it;
    /// a split left one kid is that kid; none left, the first pane.
    #[test]
    fn a_layout_without_its_empty_panes() {
        use crate::split::Axis;
        use crate::tabs::Saved;
        let s = |v: &[&str]| Saved { sessions: v.iter().map(|s| s.to_string()).collect(), active: Some(0), ..Saved::default() };
        let col = |a: usize, b: usize| LayoutNode::Split {
            axis: Axis::Column,
            ratios: vec![0.6, 0.4],
            kids: vec![LayoutNode::Pane(a), LayoutNode::Pane(b)],
        };
        let three = |panes: Vec<Saved>, focused: usize| Layout {
            focused,
            tree: LayoutNode::Split {
                axis: Axis::Row,
                ratios: vec![0.25, 0.25, 0.5],
                kids: vec![LayoutNode::Pane(0), col(1, 2), LayoutNode::Pane(3)],
            },
            panes,
        };
        // Pane 2 empty and focused: its column is pane 1 alone, the focus pane 1's.
        let l = three(vec![s(&["a"]), s(&["b"]), s(&[]), s(&["c"])], 2).without_empty_panes();
        assert_eq!(
            l,
            Layout {
                focused: 1,
                tree: LayoutNode::Split {
                    axis: Axis::Row,
                    ratios: vec![0.25, 0.25, 0.5],
                    kids: vec![LayoutNode::Pane(0), LayoutNode::Pane(1), LayoutNode::Pane(2)]
                },
                panes: vec![s(&["a"]), s(&["b"]), s(&["c"])],
            }
        );
        // Pane 0 empty and focused: the shares re-summed, the focus the first left.
        let l = three(vec![s(&[]), s(&["b"]), s(&["c"]), s(&["d"])], 0).without_empty_panes();
        assert_eq!(l.focused, 0);
        assert_eq!(
            l.tree,
            LayoutNode::Split { axis: Axis::Row, ratios: vec![0.25 / 0.75, 0.5 / 0.75], kids: vec![col(0, 1), LayoutNode::Pane(2)] }
        );
        // One left: one pane (the restore's one-pane path); none: the first.
        let two = |a: &[&str], b: &[&str]| Layout {
            focused: 1,
            tree: LayoutNode::Split { axis: Axis::Row, ratios: vec![0.5, 0.5], kids: vec![LayoutNode::Pane(0), LayoutNode::Pane(1)] },
            panes: vec![s(a), s(b)],
        };
        assert_eq!(two(&["a"], &[]).without_empty_panes(), Layout { focused: 0, tree: LayoutNode::Pane(0), panes: vec![s(&["a"])] });
        assert_eq!(two(&[], &[]).without_empty_panes(), Layout { focused: 0, tree: LayoutNode::Pane(0), panes: vec![s(&[])] });
        assert_eq!(two(&["a"], &["b"]).without_empty_panes(), two(&["a"], &["b"]), "none empty: as it was");
    }

    /// Room is the only limit (split.md §0, the 2026-11-25 note): a saved
    /// layout is read by its total alone. Six across and four down are
    /// read (a window too small for them folds them); ten panes are not.
    #[test]
    fn a_saved_layout_is_read_by_its_total_alone() {
        let dir = std::env::temp_dir().join(format!("synaps-desktop-layout-total-{}", std::process::id()));
        let path = dir.join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pane = r#"{"sessions": ["s1"], "active": 0}"#;
        let flat = |axis: &str, n: usize| {
            let kids: Vec<String> = (0..n).map(|i| format!(r#"{{"pane": {i}}}"#)).collect();
            let ratios = vec![format!("{}", 1. / n as f64); n];
            let panes = vec![pane; n];
            format!(
                r#"{{"v": 1, "tree": {{"split": "{axis}", "ratios": [{}], "kids": [{}]}}, "panes": [{}]}}"#,
                ratios.join(", "),
                kids.join(", "),
                panes.join(", ")
            )
        };
        for (what, layout, n) in [
            ("six across", flat("row", 6), Some(6)),
            ("four down", flat("column", 4), Some(4)),
            ("nine across", flat("row", 9), Some(9)),
            ("ten panes", flat("row", 10), None),
        ] {
            let file = format!(r#"{{"tabs": {{"k": {{"sessions": ["s1"], "active": 0}}}}, "layout": {{"k": {layout}}}}}"#);
            std::fs::write(&path, file).unwrap();
            assert_eq!(load_layout(&path, "k").map(|l| l.panes.len()), n, "{what}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// split/architecture.md §5: a layout that is not whole is none, and a
    /// launch falls back to `tabs.<key>` as one pane; malformed input never
    /// panics.
    #[test]
    fn a_malformed_layout_falls_back_to_the_tabs() {
        let dir = std::env::temp_dir().join(format!("synaps-desktop-layout-bad-{}", std::process::id()));
        let path = dir.join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pane = r#"{"sessions": ["s1"], "active": 0}"#;
        let two = format!("[{pane}, {pane}]");
        let row = r#"{"split": "row", "ratios": [0.5, 0.5], "kids": [{"pane": 0}, {"pane": 1}]}"#;
        let bad = [
            ("another version", format!(r#"{{"v": 2, "focused": 0, "tree": {row}, "panes": {two}}}"#)),
            ("no version", format!(r#"{{"focused": 0, "tree": {row}, "panes": {two}}}"#)),
            (
                "an unknown axis",
                format!(
                    r#"{{"v": 1, "tree": {{"split": "diagonal", "ratios": [0.5, 0.5], "kids": [{{"pane": 0}}, {{"pane": 1}}]}}, "panes": {two}}}"#
                ),
            ),
            (
                "ratios that do not sum to 1",
                format!(
                    r#"{{"v": 1, "tree": {{"split": "row", "ratios": [0.5, 0.6], "kids": [{{"pane": 0}}, {{"pane": 1}}]}}, "panes": {two}}}"#
                ),
            ),
            (
                "a ratio short",
                format!(
                    r#"{{"v": 1, "tree": {{"split": "row", "ratios": [1.0], "kids": [{{"pane": 0}}, {{"pane": 1}}]}}, "panes": {two}}}"#
                ),
            ),
            (
                "a negative ratio",
                format!(
                    r#"{{"v": 1, "tree": {{"split": "row", "ratios": [1.5, -0.5], "kids": [{{"pane": 0}}, {{"pane": 1}}]}}, "panes": {two}}}"#
                ),
            ),
            (
                "a missing pane index",
                format!(
                    r#"{{"v": 1, "tree": {{"split": "row", "ratios": [0.5, 0.5], "kids": [{{"pane": 0}}, {{"pane": 2}}]}}, "panes": {two}}}"#
                ),
            ),
            (
                "a pane named twice",
                format!(
                    r#"{{"v": 1, "tree": {{"split": "row", "ratios": [0.5, 0.5], "kids": [{{"pane": 0}}, {{"pane": 0}}]}}, "panes": {two}}}"#
                ),
            ),
            ("a pane no leaf names", format!(r#"{{"v": 1, "tree": {{"pane": 0}}, "panes": {two}}}"#)),
            (
                "more than 4 panes",
                format!(
                    r#"{{"v": 1, "tree": {{"split": "row", "ratios": [0.2, 0.2, 0.2, 0.2, 0.2], "kids": [{{"pane": 0}}, {{"pane": 1}}, {{"pane": 2}}, {{"pane": 3}}, {{"pane": 4}}]}}, "panes": [{pane}, {pane}, {pane}, {pane}, {pane}]}}"#
                ),
            ),
            (
                "4 across",
                format!(
                    r#"{{"v": 1, "tree": {{"split": "row", "ratios": [0.25, 0.25, 0.25, 0.25], "kids": [{{"pane": 0}}, {{"pane": 1}}, {{"pane": 2}}, {{"pane": 3}}]}}, "panes": [{pane}, {pane}, {pane}, {pane}]}}"#
                ),
            ),
            ("a pane that is not a tab list", format!(r#"{{"v": 1, "tree": {row}, "panes": [{pane}, 7]}}"#)),
            ("no panes", r#"{"v": 1, "tree": {"pane": 0}, "panes": []}"#.to_string()),
            ("a tree that is a number", format!(r#"{{"v": 1, "tree": 3, "panes": {two}}}"#)),
            ("not an object", "[1, 2]".to_string()),
        ];
        for (what, layout) in bad {
            let file = format!(r#"{{"tabs": {{"k": {{"sessions": ["s1"], "active": 0}}}}, "layout": {{"k": {layout}}}}}"#);
            std::fs::write(&path, file).unwrap();
            assert_eq!(load_layout(&path, "k"), None, "{what}");
            let (l, tabs) = load_restore(&path, "k", Some(true));
            assert!(l.is_none(), "{what}");
            assert_eq!(tabs.map(|t| t.sessions), Some(vec!["s1".to_string()]), "{what}: the tabs, as one pane");
        }
        // Whole: read (a focused index past the panes is the first).
        let file = format!(
            r#"{{"tabs": {{"k": {{"sessions": ["s1"], "active": 0}}}}, "layout": {{"k": {{"v": 1, "focused": 9, "tree": {row}, "panes": {two}}}}}}}"#
        );
        std::fs::write(&path, file).unwrap();
        assert_eq!(load_layout(&path, "k").map(|l| (l.focused, l.panes.len())), Some((0, 2)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_size_round_trips() {
        let dir = std::env::temp_dir().join(format!("synaps-desktop-text-{}", std::process::id()));
        let path = dir.join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load_panels(&path).text_size, DEFAULT_TEXT);
        let p0 = Panels::default();
        let p = Panels { text_size: 20, ..p0 };
        save_panels(&path, p, PanelKeys::changed(p0, p)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v, serde_json::json!({"text_size": 20}));
        assert_eq!(load_panels(&path), p);
        for bad in [r#"{"text_size": 19}"#, r#"{"text_size": "big"}"#, r#"{"text_size": -1}"#, r#"{"text_size": 1e9}"#] {
            std::fs::write(&path, bad).unwrap();
            assert_eq!(load_panels(&path).text_size, DEFAULT_TEXT, "{bad}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_round_trip_and_keep_other_keys() {
        let dir = std::env::temp_dir().join(format!("synaps-desktop-theme-{}", std::process::id()));
        let path = dir.join("nested").join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load_accent(&path), None);
        save_accent(&path, Some(0x2d7bff)).unwrap();
        assert_eq!(load_accent(&path).as_deref(), Some("#2d7bff"));
        std::fs::write(&path, r##"{"other": 7, "accent": "#000000"}"##).unwrap();
        save_accent(&path, Some(0xf4b400)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["other"], 7);
        assert_eq!(v["accent"], "#f4b400");
        // A corrupt file is kept aside, then replaced; an unreadable accent
        // is only a fallback.
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(load_accent(&path), None);
        save_accent(&path, Some(0x35f2ff)).unwrap();
        assert_eq!(load_accent(&path).as_deref(), Some("#35f2ff"));
        assert_eq!(std::fs::read_to_string(path.with_extension("json.bad")).unwrap(), "{not json");
        // No temporary file is left behind.
        let left: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(left.is_empty(), "{left:?}");
        std::fs::write(&path, r#"{"accent": "garbage"}"#).unwrap();
        assert_eq!(resolve_accent(None, None, load_accent(&path).as_deref()).0, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Nit 5: a THIS APP batch a quit's flush wrote again changes nothing
    /// when its own write lands late.
    #[test]
    fn a_stale_settings_write_changes_nothing() {
        let dir = std::env::temp_dir().join(format!("synaps-desktop-stale-{}", std::process::id()));
        let path = dir.join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);
        save_settings(&path, &[("text_size", 15.into()), ("dash_url", "new".into())]).unwrap();
        save_settings_unless(&path, &[("dash_url", "old".into())], || true).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!((v["dash_url"].as_str(), v["text_size"].as_u64()), (Some("new"), Some(15)));
        save_settings_unless(&path, &[("dash_url", "later".into())], || false).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["dash_url"], "later");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The panel states share the accent's file: each save keeps the other's
    /// keys, and anything unreadable means open.
    #[test]
    fn panel_states_round_trip_beside_the_accent() {
        let dir = std::env::temp_dir().join(format!("synaps-desktop-panels-{}", std::process::id()));
        let path = dir.join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load_panels(&path), Panels::default(), "no file: both open");
        save_accent(&path, Some(0xf4b400)).unwrap();
        let both = PanelKeys { rail: true, right: true, ..PanelKeys::default() };
        for p in [
            Panels { rail_collapsed: true, right_collapsed: false, ..Panels::default() },
            Panels { rail_collapsed: false, right_collapsed: true, ..Panels::default() },
            Panels { rail_collapsed: true, right_collapsed: true, ..Panels::default() },
            Panels::default(),
        ] {
            save_panels(&path, p, both).unwrap();
            assert_eq!(load_panels(&path), p);
            assert_eq!(load_accent(&path).as_deref(), Some("#f4b400"), "the accent is kept");
        }
        save_panels(&path, Panels { rail_collapsed: true, right_collapsed: true, ..Panels::default() }, both).unwrap();
        save_accent(&path, Some(0x2d7bff)).unwrap();
        assert_eq!(
            load_panels(&path),
            Panels { rail_collapsed: true, right_collapsed: true, ..Panels::default() },
            "the accent's save keeps them"
        );
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v, serde_json::json!({"accent": "#2d7bff", "rail_collapsed": true, "right_collapsed": true}));
        // Hand edits: not a boolean, or not JSON, means open.
        std::fs::write(&path, r#"{"rail_collapsed": "yes", "right_collapsed": 1}"#).unwrap();
        assert_eq!(load_panels(&path), Panels::default());
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(load_panels(&path), Panels::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The HISTORY buckets the user opened or closed are remembered by
    /// name; unknown names are ignored, anything else is the default.
    #[test]
    fn history_buckets_are_remembered() {
        let dir = std::env::temp_dir().join(format!("synaps-desktop-history-{}", std::process::id()));
        let path = dir.join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load_panels(&path).history_open, Bucket::DEFAULT_OPEN);
        let p = Panels { history_open: Bucket::Today.bit() | Bucket::Older.bit(), ..Panels::default() };
        save_panels(&path, p, PanelKeys::changed(Panels::default(), p)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v, serde_json::json!({"history_open": ["today", "older"]}), "only the key that changed");
        assert_eq!(load_panels(&path), p);
        std::fs::write(&path, r#"{"history_open": ["week", "from_the_future"]}"#).unwrap();
        assert_eq!(load_panels(&path).history_open, Bucket::Week.bit());
        std::fs::write(&path, r#"{"history_open": []}"#).unwrap();
        assert_eq!(load_panels(&path).history_open, 0, "all closed");
        std::fs::write(&path, r#"{"history_open": "today"}"#).unwrap();
        assert_eq!(load_panels(&path).history_open, Bucket::DEFAULT_OPEN);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L4: two windows, each with its own copy of the panels. Each writes
    /// only the key it changed, so neither undoes the other's fold.
    #[test]
    fn two_windows_keep_each_others_folds() {
        let dir = std::env::temp_dir().join(format!("synaps-desktop-panels2-{}", std::process::id()));
        let path = dir.join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);
        let (a0, b0) = (Panels::default(), Panels::default());
        let a = Panels { rail_collapsed: true, ..a0 };
        save_panels(&path, a, PanelKeys::changed(a0, a)).unwrap();
        let b = Panels { right_collapsed: true, ..b0 };
        save_panels(&path, b, PanelKeys::changed(b0, b)).unwrap();
        assert_eq!(load_panels(&path), Panels { rail_collapsed: true, right_collapsed: true, ..Panels::default() });
        save_panels(&path, b, PanelKeys::default()).unwrap();
        assert_eq!(
            load_panels(&path),
            Panels { rail_collapsed: true, right_collapsed: true, ..Panels::default() },
            "nothing changed: nothing written"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Dragged widths: saved as whole pixels, only the one that changed;
    /// a reset saves `null`; anything unreadable is the auto width and a
    /// width out of range is clamped.
    #[test]
    fn panel_widths_round_trip() {
        let dir = std::env::temp_dir().join(format!("synaps-desktop-widths-{}", std::process::id()));
        let path = dir.join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);
        let p0 = Panels::default();
        let a = Panels { rail_width: Some(400), ..p0 };
        save_panels(&path, a, PanelKeys::changed(p0, a)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v, serde_json::json!({"rail_width": 400}), "only the key that changed");
        // Another window drags the right panel: the rail's width stays.
        let b = Panels { right_width: Some(300), ..p0 };
        save_panels(&path, b, PanelKeys::changed(p0, b)).unwrap();
        assert_eq!(load_panels(&path), Panels { rail_width: Some(400), right_width: Some(300), ..p0 });
        // A double-click reset.
        let c = Panels { rail_width: None, ..a };
        save_panels(&path, c, PanelKeys::changed(a, c)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["rail_width"], serde_json::Value::Null);
        assert_eq!(load_panels(&path), Panels { right_width: Some(300), ..p0 });
        std::fs::write(&path, r#"{"rail_width": 9000, "right_width": 10.4}"#).unwrap();
        assert_eq!((load_panels(&path).rail_width, load_panels(&path).right_width), (Some(480), Some(240)));
        std::fs::write(&path, r#"{"rail_width": 300.6, "right_width": "wide"}"#).unwrap();
        assert_eq!((load_panels(&path).rail_width, load_panels(&path).right_width), (Some(301), None));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
