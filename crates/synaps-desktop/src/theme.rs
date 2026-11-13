//! The accent theme. One user-chosen colour drives every chromatic colour in
//! the window (fills, borders, the alert/deny colour, the gauge, active tabs,
//! the kit's focus ring and selection); the neutrals never change.
//!
//! The variants are derived in OKLCH (perceptual lightness, so a blue and a
//! yellow accent get the same visual weight), gamut-mapped back to sRGB by
//! lowering chroma, and floored on WCAG contrast against the window's
//! backgrounds: accent text reaches 4.5:1, accent fills 3:1, so a navy or
//! near-black accent is lifted instead of vanishing.
//!
//! The palette is a GPUI [`Global`] ([`Accent`]): derived once per change,
//! read by value (`Copy`) in `render`, nothing allocated per frame.
//!
//! Sources, first valid wins: `--accent #RRGGBB`, `$SYNAPS_DESKTOP_ACCENT`,
//! the settings file (`<config dir>/synaps-desktop/settings.json`, written by
//! the in-app picker), then Arasaka red. Anything that does not parse falls
//! through to the next source (and finally the default) with a warning.

use std::path::{Path, PathBuf};

use gpui_kit::component::Theme;
use gpui_kit::{App, Global, Hsla, px, rgb, rgba};

pub const DEFAULT_ACCENT: u32 = 0xe8132f;
pub const ENV_ACCENT: &str = "SYNAPS_DESKTOP_ACCENT";

/// The picker's presets: (tracked label, colour).
pub const PRESETS: &[(&str, u32)] = &[
    ("A R A S A K A", 0xe8132f),
    ("M I L I T E C H", 0xf4b400),
    ("K A N G   T A O", 0xcfa85a),
    ("N E T W A T C H", 0x2d7bff),
    ("K I R O S H I", 0x35f2ff),
    ("V O O D O O", 0x2bd47d),
];

/// The neutrals (hybrid.html `:root`). Never tinted by the accent.
pub mod neutral {
    pub const BG: u32 = 0x070708;
    pub const BG1: u32 = 0x0a0a0c;
    pub const BG2: u32 = 0x0e0e11;
    pub const BG3: u32 = 0x141418;
    pub const LINE: u32 = 0x18181c;
    pub const LINE2: u32 = 0x222228;
    pub const LINE3: u32 = 0x313138;
    pub const INK: u32 = 0xece8e1;
    pub const INK2: u32 = 0xb3afa8;
    pub const INK3: u32 = 0x7a7772;
    pub const INK4: u32 = 0x4d4b47;
    pub const INK5: u32 = 0x2a2927;
    pub const SEG: u32 = 0x151518;
}

/// Minimum WCAG contrast of accent used as text (`hi`, `soft`).
pub const TEXT_CONTRAST: f64 = 4.5;
/// Minimum WCAG contrast of accent fills (dots, bars, gauge segments).
pub const FILL_CONTRAST: f64 = 3.0;
/// Minimum WCAG contrast of accent borders (`mid`).
pub const BORDER_CONTRAST: f64 = 2.0;

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

/// Where the accent came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Flag,
    Env,
    Settings,
    Default,
}

/// The first valid of `flag`, `env`, `settings`; else [`DEFAULT_ACCENT`].
/// Each invalid one is reported in the returned warnings.
pub fn resolve(flag: Option<&str>, env: Option<&str>, settings: Option<&str>) -> (u32, Source, Vec<String>) {
    let mut warnings = Vec::new();
    for (v, src, what) in
        [(flag, Source::Flag, "--accent"), (env, Source::Env, ENV_ACCENT), (settings, Source::Settings, "settings accent")]
    {
        let Some(v) = v else { continue };
        match parse_hex(v) {
            Some(c) => return (c, src, warnings),
            None => warnings.push(format!("{what}: '{}' is not a #RRGGBB colour; ignored", v.chars().take(40).collect::<String>())),
        }
    }
    (DEFAULT_ACCENT, Source::Default, warnings)
}

// ── colour science ─────────────────────────────────────────────────────────

fn channels(c: u32) -> [f64; 3] {
    [((c >> 16) & 0xff) as f64 / 255.0, ((c >> 8) & 0xff) as f64 / 255.0, (c & 0xff) as f64 / 255.0]
}

fn pack(c: [f64; 3]) -> u32 {
    let q = |x: f64| (x.clamp(0.0, 1.0) * 255.0).round() as u32;
    (q(c[0]) << 16) | (q(c[1]) << 8) | q(c[2])
}

fn to_linear(c: f64) -> f64 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn to_gamma(c: f64) -> f64 {
    if c <= 0.003_130_8 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// WCAG 2 relative luminance.
pub fn luminance(c: u32) -> f64 {
    let [r, g, b] = channels(c).map(to_linear);
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// WCAG 2 contrast ratio (1..=21).
pub fn contrast(a: u32, b: u32) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

#[derive(Clone, Copy, Debug)]
struct Lch {
    l: f64,
    c: f64,
    h: f64,
}

fn to_oklch(c: u32) -> Lch {
    let [r, g, b] = channels(c).map(to_linear);
    let l = 0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b;
    let m = 0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b;
    let s = 0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b;
    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
    let lab_l = 0.210_454_255_3 * l + 0.793_617_785 * m - 0.004_072_046_8 * s;
    let a = 1.977_998_495_1 * l - 2.428_592_205 * m + 0.450_593_709_9 * s;
    let bb = 0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766 * s;
    Lch { l: lab_l, c: a.hypot(bb), h: bb.atan2(a) }
}

/// Linear sRGB of an OKLCH colour (may be out of gamut).
fn oklch_linear(o: Lch) -> [f64; 3] {
    let (a, b) = (o.c * o.h.cos(), o.c * o.h.sin());
    let l = (o.l + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powi(3);
    let m = (o.l - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powi(3);
    let s = (o.l - 0.089_484_177_5 * a - 1.291_485_548 * b).powi(3);
    [
        4.076_741_662_1 * l - 3.307_711_591_3 * m + 0.230_969_929_2 * s,
        -1.268_438_004_6 * l + 2.609_757_401_1 * m - 0.341_319_396_5 * s,
        -0.004_196_086_3 * l - 0.703_418_614_7 * m + 1.707_614_701 * s,
    ]
}

/// OKLCH → sRGB, keeping lightness and hue and lowering chroma until the
/// colour fits the gamut.
fn from_oklch(o: Lch) -> u32 {
    let fits = |c: [f64; 3]| c.iter().all(|&x| (-1e-6..=1.0 + 1e-6).contains(&x));
    let o = Lch { l: o.l.clamp(0.0, 1.0), ..o };
    let mut lin = oklch_linear(o);
    if !fits(lin) {
        let (mut lo, mut hi) = (0.0, o.c);
        for _ in 0..28 {
            let mid = (lo + hi) / 2.0;
            if fits(oklch_linear(Lch { c: mid, ..o })) { lo = mid } else { hi = mid }
        }
        lin = oklch_linear(Lch { c: lo, ..o });
    }
    pack(lin.map(|x| to_gamma(x.clamp(0.0, 1.0))))
}

/// Raise lightness until `o` reaches `min` contrast against every one of
/// `backgrounds` (all darker than the result), or white.
fn floor_contrast(mut o: Lch, min: f64, backgrounds: &[u32]) -> u32 {
    loop {
        let c = from_oklch(o);
        if o.l >= 1.0 || backgrounds.iter().all(|&b| contrast(c, b) >= min) {
            return c;
        }
        o.l = (o.l + 0.005).min(1.0);
    }
}

/// The accent's variants as sRGB, before they become GPUI colours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Derived {
    /// Fills: the accent itself, lifted to [`FILL_CONTRAST`] if too dark.
    pub accent: u32,
    /// Accent text (alerts, deny, needs-you): brighter, ≥ [`TEXT_CONTRAST`].
    pub hi: u32,
    /// Borders (the authorization card): darker, ≥ [`BORDER_CONTRAST`].
    pub mid: u32,
    /// The accent-tinted near-black fill (the authorization card).
    pub bg: u32,
    /// Low-chroma accent text (deleted diff lines), ≥ [`TEXT_CONTRAST`].
    pub soft: u32,
    /// `accent` had to be lifted to stay visible.
    pub lifted: bool,
}

impl Derived {
    pub fn of(accent: u32) -> Self {
        let base = to_oklch(accent & 0xff_ffff);
        let bg = from_oklch(Lch { l: 0.15, c: (base.c * 0.14).min(0.035), ..base });
        let text_bgs = [neutral::BG, neutral::BG2, neutral::BG3, bg];
        let fill = floor_contrast(base, FILL_CONTRAST, &[neutral::BG]);
        let hi_l = if base.l < 0.8 { (base.l + 0.07).min(0.97) } else { base.l };
        Derived {
            accent: fill,
            hi: floor_contrast(Lch { l: hi_l, ..base }, TEXT_CONTRAST, &text_bgs),
            mid: floor_contrast(Lch { l: base.l * 0.72, c: base.c * 0.78, ..base }, BORDER_CONTRAST, &[neutral::BG]),
            bg,
            soft: floor_contrast(Lch { l: base.l.max(0.70), c: base.c * 0.42, ..base }, TEXT_CONTRAST, &text_bgs),
            lifted: fill != accent & 0xff_ffff,
        }
    }
}

/// Every colour the window paints with, as GPUI colours. `Copy`: `render`
/// takes it by value from the [`Accent`] global.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub accent: Hsla,
    pub hi: Hsla,
    pub mid: Hsla,
    pub accent_bg: Hsla,
    pub soft: Hsla,
    /// `accent` at 10 %: deleted diff lines, the picker's selection.
    pub tint: Hsla,
    pub bg: Hsla,
    pub bg1: Hsla,
    pub bg2: Hsla,
    pub bg3: Hsla,
    pub line: Hsla,
    pub line2: Hsla,
    pub line3: Hsla,
    pub ink: Hsla,
    pub ink2: Hsla,
    pub ink3: Hsla,
    pub ink4: Hsla,
    pub ink5: Hsla,
    pub seg: Hsla,
    /// Added diff lines: ink at 4.5 %.
    pub add: Hsla,
    /// The authorization card's command box.
    pub well: Hsla,
}

fn hsla(c: u32) -> Hsla {
    rgb(c).into()
}

impl Palette {
    pub fn of(d: &Derived) -> Self {
        use neutral::*;
        Palette {
            accent: hsla(d.accent),
            hi: hsla(d.hi),
            mid: hsla(d.mid),
            accent_bg: hsla(d.bg),
            soft: hsla(d.soft),
            tint: rgba((d.accent << 8) | 0x1a).into(),
            bg: hsla(BG),
            bg1: hsla(BG1),
            bg2: hsla(BG2),
            bg3: hsla(BG3),
            line: hsla(LINE),
            line2: hsla(LINE2),
            line3: hsla(LINE3),
            ink: hsla(INK),
            ink2: hsla(INK2),
            ink3: hsla(INK3),
            ink4: hsla(INK4),
            ink5: hsla(INK5),
            seg: hsla(SEG),
            add: rgba((INK << 8) | 0x0b).into(),
            well: rgba(0x0000_0073).into(),
        }
    }
}

/// The current accent, its derivation and palette.
#[derive(Clone, Copy, Debug)]
pub struct Accent {
    pub hex: u32,
    pub derived: Derived,
    pub p: Palette,
}

impl Global for Accent {}

impl Accent {
    pub fn new(hex: u32) -> Self {
        let derived = Derived::of(hex);
        Accent { hex: hex & 0xff_ffff, derived, p: Palette::of(&derived) }
    }
}

/// The palette, by value (no allocation).
pub fn palette(cx: &App) -> Palette {
    cx.global::<Accent>().p
}

/// Install (or change) the accent: the global, then the kit's theme (inputs,
/// markdown, scrollbars, focus ring), which refreshes every window. A no-op
/// when it is already this colour.
pub fn apply(hex: u32, cx: &mut App) {
    let hex = hex & 0xff_ffff;
    if cx.has_global::<Accent>() && cx.global::<Accent>().hex == hex {
        return;
    }
    let a = Accent::new(hex);
    cx.set_global(a);
    sync_kit(&a, cx);
}

/// Code highlighting in neutrals only (the mock's `.k` / `.s` / `.c`): no
/// syntax colour competes with the accent.
fn highlight_theme() -> Option<std::sync::Arc<gpui_kit::component::highlighter::HighlightTheme>> {
    use neutral::*;
    const STRING: u32 = 0xa39e95;
    let styles: [(&str, u32, Option<u16>); 33] = [
        ("keyword", INK, Some(600)),
        ("title", INK, Some(600)),
        ("function", INK, None),
        ("type", INK, None),
        ("constructor", INK, None),
        ("tag", INK, None),
        ("text.literal", INK, None),
        ("text.code.span", INK, None),
        ("link_text", INK, None),
        ("attribute", INK2, None),
        ("property", INK2, None),
        ("variable", INK2, None),
        ("label", INK2, None),
        ("number", INK2, None),
        ("boolean", INK2, None),
        ("constant", INK2, None),
        ("enum", INK2, None),
        ("variant", INK2, None),
        ("string", STRING, None),
        ("string.regex", STRING, None),
        ("string.special", STRING, None),
        ("string.special.symbol", STRING, None),
        ("string.escape", INK3, None),
        ("operator", INK3, None),
        ("punctuation", INK3, None),
        ("punctuation.bracket", INK3, None),
        ("punctuation.delimiter", INK3, None),
        ("punctuation.list_marker", INK3, None),
        ("punctuation.special", INK3, None),
        ("link_uri", INK3, None),
        ("comment", INK4, None),
        ("comment_doc", INK4, None),
        ("embedded", INK2, None),
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
    for (key, color) in
        [("editor.background", BG2), ("editor.foreground", INK2), ("editor.line_number", INK5), ("editor.active_line_number", INK3)]
    {
        style.insert(key.into(), to_hex(color).into());
    }
    style.insert("syntax".into(), syntax.into());
    let v = serde_json::json!({"name": "Synaps Hybrid", "appearance": "dark", "style": style});
    serde_json::from_value(v).ok().map(std::sync::Arc::new)
}

/// The kit theme follows the palette: neutrals for surfaces and text, the
/// accent for every chromatic slot.
fn sync_kit(a: &Accent, cx: &mut App) {
    let p = a.p;
    let light_accent = luminance(a.derived.accent) > 0.4;
    let highlight = highlight_theme();
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
        c.background = p.bg;
        c.foreground = p.ink;
        c.border = p.line2;
        c.input = p.line3;
        c.ring = p.accent;
        c.caret = p.ink;
        c.selection = p.accent.opacity(0.35);
        c.muted = p.bg3;
        c.muted_foreground = p.ink3;
        c.secondary = p.bg3;
        c.secondary_hover = p.bg2;
        c.secondary_active = p.bg2;
        c.secondary_foreground = p.ink;
        c.accent = p.bg2;
        c.accent_foreground = p.ink;
        c.popover = p.bg1;
        c.popover_foreground = p.ink;
        c.primary = p.accent;
        c.primary_hover = p.hi;
        c.primary_active = p.mid;
        c.primary_foreground = if light_accent { p.bg } else { p.ink };
        c.danger = p.accent;
        c.danger_hover = p.hi;
        c.danger_active = p.mid;
        c.danger_foreground = p.ink;
        c.warning = p.hi;
        c.success = p.ink2;
        c.info = p.ink2;
        c.link = p.hi;
        c.link_hover = p.ink;
        c.link_active = p.ink;
        c.scrollbar = gpui_kit::transparent_black();
        c.scrollbar_thumb = p.line3;
        c.scrollbar_thumb_hover = p.ink4;
        c.sidebar = p.bg;
        c.sidebar_border = p.line;
        c.sidebar_accent = p.bg2;
        c.sidebar_foreground = p.ink;
        c.list = p.bg;
        c.list_hover = p.bg2;
        c.list_active = p.bg2;
        c.list_active_border = p.accent;
        c.tab_active = p.bg;
        c.tab_active_foreground = p.ink;
        c.progress_bar = p.accent;
        c.slider_bar = p.accent;
        c.switch = p.accent;
    });
}

/// The settings file: `<config dir>/synaps-desktop/settings.json`.
/// Linux/BSD: `$XDG_CONFIG_HOME` or `~/.config`; macOS: `~/Library/Application
/// Support`; Windows: `%APPDATA%`.
pub fn settings_path() -> Option<PathBuf> {
    let env_dir = |k: &str| std::env::var_os(k).map(PathBuf::from).filter(|p| p.is_absolute());
    let dir = if cfg!(windows) {
        env_dir("APPDATA")?
    } else if cfg!(target_os = "macos") {
        env_dir("HOME")?.join("Library").join("Application Support")
    } else {
        env_dir("XDG_CONFIG_HOME").or_else(|| env_dir("HOME").map(|h| h.join(".config")))?
    };
    Some(dir.join("synaps-desktop").join("settings.json"))
}

/// The saved accent as written (unparsed: [`resolve`] checks it).
pub fn load_accent(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("accent")?.as_str().map(str::to_string)
}

/// Save the accent, keeping the file's other keys; written to a temporary
/// file first, then renamed over the old one.
pub fn save_accent(path: &Path, hex: u32) -> std::io::Result<()> {
    let mut v = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    v["accent"] = serde_json::Value::String(to_hex(hex));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&v).unwrap_or_default() + "\n")?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn garbage_falls_back_in_order_then_to_the_default() {
        assert_eq!(resolve(None, None, None).0, DEFAULT_ACCENT);
        let (c, src, warn) = resolve(Some("not a colour"), None, None);
        assert_eq!((c, src), (DEFAULT_ACCENT, Source::Default));
        assert_eq!(warn.len(), 1);
        let (c, src, warn) = resolve(Some("#zz0000"), Some("bogus"), Some("#2d7bff"));
        assert_eq!((c, src, warn.len()), (0x2d7bff, Source::Settings, 2));
        assert_eq!(resolve(Some("#f4b400"), Some("#2d7bff"), Some("#35f2ff")).1, Source::Flag);
        assert_eq!(resolve(None, Some("#2d7bff"), Some("#35f2ff")), (0x2d7bff, Source::Env, vec![]));
        // A huge garbage value is not echoed whole.
        let long = "x".repeat(10_000);
        assert!(resolve(Some(&long), None, None).2[0].len() < 120);
    }

    #[test]
    fn contrast_matches_wcag() {
        assert!((contrast(0x000000, 0xffffff) - 21.0).abs() < 1e-9);
        assert!((contrast(0x777777, 0x777777) - 1.0).abs() < 1e-9);
        // The mock's own red as text is just under 4.5:1 on the background,
        // which is why text uses `hi`.
        assert!(contrast(0xe8132f, neutral::BG) < TEXT_CONTRAST);
        assert!(contrast(0xff3f56, neutral::BG) > TEXT_CONTRAST);
    }

    #[test]
    fn oklch_round_trips() {
        for c in [0x000000, 0xffffff, 0xe8132f, 0x2d7bff, 0x35f2ff, 0x808080, 0x0a1a40, 0xf4b400] {
            let back = from_oklch(to_oklch(c));
            let d = channels(c).iter().zip(channels(back)).map(|(a, b)| (a - b).abs()).fold(0.0, f64::max);
            assert!(d <= 1.5 / 255.0, "{c:06x} -> {back:06x}");
        }
    }

    fn check_floors(c: u32) -> Derived {
        let d = Derived::of(c);
        for bg in [neutral::BG, neutral::BG2, neutral::BG3, d.bg] {
            assert!(contrast(d.hi, bg) >= TEXT_CONTRAST, "{c:06x}: hi {:06x} on {bg:06x}", d.hi);
            assert!(contrast(d.soft, bg) >= TEXT_CONTRAST, "{c:06x}: soft {:06x} on {bg:06x}", d.soft);
        }
        assert!(contrast(d.accent, neutral::BG) >= FILL_CONTRAST, "{c:06x}: fill {:06x}", d.accent);
        assert!(contrast(d.mid, neutral::BG) >= BORDER_CONTRAST, "{c:06x}: mid {:06x}", d.mid);
        // The card fill stays a near-black.
        assert!(luminance(d.bg) < 0.02, "{c:06x}: bg {:06x}", d.bg);
        d
    }

    #[test]
    fn every_variant_meets_its_contrast_floor() {
        for &(_, c) in PRESETS {
            check_floors(c);
        }
        let mut r = 0x1234_5678u32;
        for _ in 0..500 {
            r ^= r << 13;
            r ^= r >> 17;
            r ^= r << 5;
            check_floors(r & 0xff_ffff);
        }
        for c in [0x000000, 0xffffff, 0x000080, 0x0a1a40, 0x101010, 0x400000, 0x003300, 0xffff00, 0x00ffff] {
            check_floors(c);
        }
    }

    #[test]
    fn dark_accents_are_lifted_bright_ones_kept() {
        for dark in [0x000000, 0x0a1a40, 0x000080, 0x1a0000] {
            let d = check_floors(dark);
            assert!(d.lifted, "{dark:06x}");
            assert_ne!(d.accent, dark);
        }
        for &(_, c) in PRESETS {
            let d = Derived::of(c);
            assert!(!d.lifted, "{c:06x}");
            assert_eq!(d.accent, c);
        }
        // A lifted navy keeps its hue: still blue.
        let d = Derived::of(0x0a1a40);
        let [r, _, b] = channels(d.hi);
        assert!(b > r, "{:06x}", d.hi);
    }

    #[test]
    fn default_red_is_close_to_the_mock() {
        let d = Derived::of(DEFAULT_ACCENT);
        let near = |a: u32, b: u32, tol: f64| channels(a).iter().zip(channels(b)).all(|(x, y)| (x - y).abs() <= tol);
        assert_eq!(d.accent, 0xe8132f);
        assert!(near(d.hi, 0xff3f56, 0.12), "hi {:06x}", d.hi);
        assert!(near(d.mid, 0x8f0c1d, 0.08), "mid {:06x}", d.mid);
        assert!(near(d.bg, 0x130609, 0.04), "bg {:06x}", d.bg);
    }

    #[test]
    fn settings_round_trip_and_keep_other_keys() {
        let dir = std::env::temp_dir().join(format!("synaps-desktop-theme-{}", std::process::id()));
        let path = dir.join("nested").join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load_accent(&path), None);
        save_accent(&path, 0x2d7bff).unwrap();
        assert_eq!(load_accent(&path).as_deref(), Some("#2d7bff"));
        std::fs::write(&path, r##"{"other": 7, "accent": "#000000"}"##).unwrap();
        save_accent(&path, 0xf4b400).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["other"], 7);
        assert_eq!(v["accent"], "#f4b400");
        // A corrupt file is replaced, and an unreadable accent is only a fallback.
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(load_accent(&path), None);
        save_accent(&path, 0x35f2ff).unwrap();
        assert_eq!(load_accent(&path).as_deref(), Some("#35f2ff"));
        std::fs::write(&path, r#"{"accent": "garbage"}"#).unwrap();
        assert_eq!(resolve(None, None, load_accent(&path).as_deref()).0, DEFAULT_ACCENT);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn code_highlighting_is_neutral() {
        assert!(highlight_theme().is_some(), "the highlight theme JSON no longer deserializes");
    }

    #[test]
    fn palette_is_derived_once_and_copy() {
        let a = Accent::new(0x2d7bff);
        let b = a;
        assert_eq!(a.p, b.p);
        assert_eq!(a.p, Palette::of(&Derived::of(0x2d7bff)));
        assert_ne!(Accent::new(0xe8132f).p.accent, a.p.accent);
        // Neutrals never follow the accent.
        assert_eq!(Accent::new(0xe8132f).p.ink, a.p.ink);
        assert_eq!(Accent::new(0xe8132f).p.bg2, a.p.bg2);
    }
}
