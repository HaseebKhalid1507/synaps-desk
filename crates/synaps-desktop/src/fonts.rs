//! Embedded fonts (OFL, subset with pyftsubset: `assets/fonts/subset.sh`) and
//! the UI's fixed labels. Rajdhani → "Hybrid Display" (wide-tracked caps),
//! IBM Plex Sans → "Hybrid Sans" (text), IBM Plex Mono → "Hybrid Mono"
//! (code). Anything else (CJK in a transcript) falls back to the system's
//! fonts.
//!
//! GPUI has no letter-spacing: display labels are tracked with spaces
//! (Rajdhani's space is 0.24 em, the mock's tracking is 0.2–0.3 em) and
//! written out here as `&'static str`, so a frame never builds them.

use std::borrow::Cow;

use gpui_kit::{App, Styled, px};

pub const DISPLAY: &str = "Hybrid Display";
pub const SANS: &str = "Hybrid Sans";
pub const MONO: &str = "Hybrid Mono";

/// The chrome's type scale (design.md §1.2), in px: nothing under 12. The
/// column's sizes scale with the text step ([`crate::theme::Metrics`]);
/// these are fixed. Usage's and the right panel's big figures (`fig.*`)
/// keep their own sizes.
/// A text size, the only way one is set (clippy's `disallowed-methods`
/// holds every other): never under [`size::FLOOR`] (design.md §1.2). A
/// size under it is a mistake the tests' debug build reports; a release
/// build draws it at the floor.
pub(crate) trait TextPx: Styled + Sized {
    fn text_px(self, size: f32) -> Self {
        debug_assert!(size >= size::FLOOR, "text under {} px: {size}", size::FLOOR);
        #[allow(clippy::disallowed_methods)]
        self.text_size(px(size.max(size::FLOOR)))
    }
}

impl<E: Styled> TextPx for E {}

pub mod size {
    /// No text is smaller.
    pub const FLOOR: f32 = 12.;
    /// Display, small: state words outside runs, chips, field keys,
    /// section heads, confirm words.
    pub const D_S: f32 = 12.;
    /// Display, medium: buttons, card titles, the wordmark.
    pub const D_M: f32 = 13.;
    /// Display, large: view titles.
    pub const D_L: f32 = 16.;
    /// Sans: menu items.
    pub const UI: f32 = 14.;
    /// Sans: rail and tab titles, overlay titles, tooltips, help.
    pub const TITLE: f32 = 13.;
    /// Mono: counts, handles, ages, costs, key hints, notes.
    pub const META: f32 = 12.;
}

const FILES: [&[u8]; 4] = [
    include_bytes!("../assets/fonts/HybridDisplay-SemiBold.ttf"),
    include_bytes!("../assets/fonts/HybridSans-Regular.ttf"),
    include_bytes!("../assets/fonts/HybridSans-SemiBold.ttf"),
    include_bytes!("../assets/fonts/HybridMono-Regular.ttf"),
];

/// Register the embedded faces (borrowed: the bytes stay in the binary).
pub fn register(cx: &mut App) {
    if let Err(e) = cx.text_system().add_fonts(FILES.iter().map(|b| Cow::Borrowed(*b)).collect()) {
        eprintln!("synaps-desktop: embedded fonts not loaded: {e}");
    }
}

/// Tracked display labels.
pub mod label {
    pub const WORD: &str = "S Y N A P S";
    pub const WORD_2: &str = "D E S K T O P";
    pub const SESSIONS: &str = "S E S S I O N S";
    pub const NEW: &str = "+  N E W";
    pub const NEEDS_YOU: &str = "N E E D S   Y O U";
    pub const NEED_YOU: &str = "N E E D   Y O U";
    pub const JUMP: &str = "J U M P";
    pub const SYSTEMS: &str = "S Y S T E M S";
    pub const CONTEXT: &str = "C O N T E X T";
    pub const TOKENS: &str = "T O K E N S";
    pub const FILES: &str = "F I L E S";
    pub const VIA_SHELL: &str = "V I A   S H E L L";
    pub const IN: &str = "I N";
    pub const OUT: &str = "O U T";
    pub const CACHE: &str = "C A C H E";
    /// Cache writes (`cache_creation`).
    pub const WRITE: &str = "W R I T E";
    pub const CACHE_HIT: &str = "C A C H E   H I T";
    /// The right panel's cache countdown and keep-warm ([`crate::warm`]).
    pub const PROMPT_CACHE: &str = "P R O M P T   C A C H E";
    pub const EXPIRES: &str = "E X P I R E S";
    pub const TTL: &str = "T T L";
    pub const KEEP_WARM: &str = "K E E P   W A R M";
    pub const PINGS: &str = "P I N G S";
    pub const SPENT: &str = "S P E N T";
    pub const NEXT_PING: &str = "N E X T   P I N G";
    pub const COST: &str = "C O S T";
    pub const WINDOW: &str = "W I N D O W";
    pub const AGENT: &str = "A G E N T";
    pub const YOU: &str = "Y O U";
    pub const SYNAPS: &str = "S Y N A P S";
    pub const READY: &str = "R E A D Y";
    pub const THEME: &str = "T H E M E";
    pub const SYNAPS_CONFIG: &str = "S Y N A P S   C O N F I G";
    pub const HALT: &str = "H A L T";
    pub const NAME: &str = "N A M E";
    pub const SEND: &str = "S E N D";
    pub const STEER: &str = "S T E E R";
    pub const STEER_WORKER: &str = "S T E E R   W O R K E R";
    pub const READ_ONLY: &str = "R E A D   O N L Y";
    pub const MAIN: &str = "M A I N";
    // Q2's, kept while its tests stand: Cards D's card speaks Sans (C2).
    #[cfg_attr(not(test), allow(dead_code))]
    pub const AUTHORIZE: &str = "A U T H O R I Z E";
    pub const CANCEL: &str = "C A N C E L";
    pub const ACCENT: &str = "A C C E N T";
    pub const CUSTOM: &str = "C U S T O M";
    pub const APPLY: &str = "A P P L Y";
    pub const CLOSE: &str = "C L O S E";
    pub const SAVE: &str = "S A V E";
    pub const REPLACE: &str = "R E P L A C E";
    pub const RUNNING: &str = "R U N N I N G";
    // A worker's states (design.md §4.2): the words outside run surfaces.
    pub const STARTED: &str = "S T A R T E D";
    pub const CANCELLING: &str = "C A N C E L L I N G";
    pub const COMPLETED: &str = "C O M P L E T E D";
    pub const FAILED: &str = "F A I L E D";
    pub const CANCELLED: &str = "C A N C E L L E D";
    pub const TIMED_OUT: &str = "T I M E D   O U T";
    pub const LAST_SEEN: &str = "L A S T   S E E N";
    pub const GONE: &str = "G O N E";
    pub const WORKERS: &str = "W O R K E R S";
    pub const OPENING: &str = "O P E N I N G";
    pub const LOCKED: &str = "L O C K E D";
    pub const UNREAD: &str = "U N R E A D";
    pub const FAVOURITES: &str = "F A V O U R I T E S";
    pub const REMOVE: &str = "R E M O V E";
    pub const PINNED: &str = "P I N N E D";
    pub const OPEN_GROUP: &str = "O P E N";
    pub const HISTORY: &str = "H I S T O R Y";
    pub const NAMED: &str = "N A M E D";
    pub const TODAY: &str = "T O D A Y";
    pub const THIS_WEEK: &str = "T H I S   W E E K";
    pub const THIS_MONTH: &str = "T H I S   M O N T H";
    pub const OLDER: &str = "O L D E R";
    pub const MORE: &str = "M O R E";
    // The palette (polish.md P16).
    pub const GO_TO: &str = "G O   T O";
    pub const COMMANDS: &str = "C O M M A N D S";
    pub const VIEWS: &str = "V I E W S";
    // The hover readout's keys.
    pub const R_STATE: &str = "S T A T E";
    pub const R_HELD: &str = "H E L D";
    pub const R_TITLE: &str = "T I T L E";
    pub const R_MODEL: &str = "M O D E L";
    pub const R_CWD: &str = "C W D";
    pub const R_SIZE: &str = "S I Z E";
    pub const R_CLIENTS: &str = "C L I E N T S";
    pub const R_LOCK: &str = "L O C K";
    pub const R_ID: &str = "I D";
    pub const R_UPDATED: &str = "U P D A T E D";
    // A worker's hover card's keys (its MODEL is the readout's).
    pub const ELAPSED: &str = "E L A P S E D";
    pub const RAN: &str = "R A N";
    pub const TOOLS: &str = "T O O L S";
    /// A cancel armed (the worker menu's item, a tray row's ×).
    pub const CONFIRM: &str = "C O N F I R M ?";
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether a TrueType font's cmap (format 4) maps `cp` to a glyph.
    fn has_glyph(font: &[u8], cp: u32) -> bool {
        let u16_at = |o: usize| u16::from_be_bytes([font[o], font[o + 1]]) as usize;
        let u32_at = |o: usize| u32::from_be_bytes([font[o], font[o + 1], font[o + 2], font[o + 3]]) as usize;
        let tables = u16_at(4);
        let cmap = (0..tables).map(|i| 12 + 16 * i).find(|&r| &font[r..r + 4] == b"cmap").map(|r| u32_at(r + 8)).expect("cmap");
        let subtables = u16_at(cmap + 2);
        for i in 0..subtables {
            let sub = cmap + u32_at(cmap + 4 + 8 * i + 4);
            if u16_at(sub) != 4 {
                continue;
            }
            let segs = u16_at(sub + 6) / 2;
            let (ends, starts) = (sub + 14, sub + 16 + 2 * segs);
            let (deltas, offsets) = (starts + 2 * segs, starts + 4 * segs);
            for s in 0..segs {
                let (start, end) = (u16_at(starts + 2 * s) as u32, u16_at(ends + 2 * s) as u32);
                if cp < start || cp > end {
                    continue;
                }
                let (delta, ro) = (u16_at(deltas + 2 * s), u16_at(offsets + 2 * s));
                let glyph = if ro == 0 {
                    (cp as usize + delta) & 0xffff
                } else {
                    let g = u16_at(offsets + 2 * s + ro + 2 * (cp - start) as usize);
                    if g == 0 { 0 } else { (g + delta) & 0xffff }
                };
                return glyph != 0;
            }
        }
        false
    }

    #[test]
    fn latin_faces_cover_the_ui_glyphs() {
        for (i, font) in FILES.iter().enumerate() {
            for c in ('!'..='~').chain(['·', '…', '—', '–', '€', '’', '“', '”']) {
                assert!(has_glyph(font, c as u32), "face {i}: {c}");
            }
        }
        for font in &FILES[1..4] {
            for c in ['→', '↓', '✓', '×', '−'] {
                assert!(has_glyph(font, c as u32), "{c}");
            }
        }
        // Tool output keeps its columns: box drawing and block elements in mono.
        let mono = FILES[3];
        for c in ['─', '│', '┌', '┐', '└', '┘', '├', '┤', '┬', '┴', '┼', '═', '║', '╭', '╰', '█', '▌', '░', '▒', '▓']
        {
            assert!(has_glyph(mono, c as u32), "mono: {c}");
        }
        let total: usize = FILES.iter().map(|f| f.len()).sum();
        assert!(total < 200 * 1024, "{total} bytes embedded");
    }

    #[test]
    fn tracked_labels_are_tracked_caps() {
        for l in [label::SESSIONS, label::AUTHORIZE, label::NEEDS_YOU, label::CONTEXT] {
            assert!(l.chars().all(|c| c == ' ' || c == '+' || c.is_ascii_uppercase()), "{l}");
            assert!(!l.contains("  ") || l.contains("   "), "{l}: word gaps are three spaces");
        }
    }
}
