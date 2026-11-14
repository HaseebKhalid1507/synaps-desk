//! Embedded fonts (OFL, subset with pyftsubset: `assets/fonts/subset.sh`) and
//! the UI's fixed labels. Rajdhani → "Hybrid Display" (wide-tracked caps),
//! IBM Plex Sans → "Hybrid Sans" (text), IBM Plex Mono → "Hybrid Mono"
//! (code), and IBM Plex Sans JP cut down to ONLY the glyphs of
//! the sub-labels below (`JP_LABELS`) → "Hybrid Labels JP". Anything else (CJK in a transcript)
//! falls back to the system's fonts.
//!
//! GPUI has no letter-spacing: display labels are tracked with spaces
//! (Rajdhani's space is 0.24 em, the mock's tracking is 0.2–0.3 em) and
//! written out here as `&'static str`, so a frame never builds them.

use std::borrow::Cow;

use gpui_kit::App;

pub const DISPLAY: &str = "Hybrid Display";
pub const SANS: &str = "Hybrid Sans";
pub const MONO: &str = "Hybrid Mono";
pub const JP: &str = "Hybrid Labels JP";

const FILES: [&[u8]; 5] = [
    include_bytes!("../assets/fonts/HybridDisplay-SemiBold.ttf"),
    include_bytes!("../assets/fonts/HybridSans-Regular.ttf"),
    include_bytes!("../assets/fonts/HybridSans-SemiBold.ttf"),
    include_bytes!("../assets/fonts/HybridMono-Regular.ttf"),
    include_bytes!("../assets/fonts/HybridLabelsJP-Regular.ttf"),
];

/// The display face has `c`: `subset.sh`'s `DISPLAY` ranges, less what
/// Rajdhani never drew. A title with anything else is set in the text face,
/// not patched per glyph.
pub fn display_covers(c: char) -> bool {
    matches!(c as u32, 0x20..=0x7e | 0xa0..=0xff | 0x2013..=0x2014 | 0x2018..=0x201d | 0x2022 | 0x2026 | 0x20ac | 0x2212)
        && !matches!(c, '¤' | 'ª' | '¬' | '\u{ad}' | '²' | '³' | 'µ' | '¹' | 'º' | '¼' | '½' | '¾' | '‛')
}

/// Register the embedded faces (borrowed: the bytes stay in the binary).
pub fn register(cx: &mut App) {
    if let Err(e) = cx.text_system().add_fonts(FILES.iter().map(|b| Cow::Borrowed(*b)).collect()) {
        eprintln!("synaps-desktop: embedded fonts not loaded: {e}");
    }
}

/// Every Japanese sub-label. `subset.sh` keeps exactly these glyphs.
#[cfg(test)]
const JP_LABELS: &[&str] = &[JP_SESSIONS, JP_CONTEXT, JP_TOKENS, JP_FILES, JP_MARK_1, JP_MARK_2, JP_PINNED, JP_OPEN, JP_HISTORY];
pub const JP_SESSIONS: &str = "セッション";
/// The rail's groups: PINNED, OPEN, HISTORY.
pub const JP_PINNED: &str = "固定";
pub const JP_OPEN: &str = "稼働";
pub const JP_HISTORY: &str = "履歴";
pub const JP_CONTEXT: &str = "文脈";
pub const JP_TOKENS: &str = "トークン";
pub const JP_FILES: &str = "変更";
/// 荒坂, the rail's mark, one glyph per line.
pub const JP_MARK_1: &str = "荒";
pub const JP_MARK_2: &str = "坂";

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
    pub const COST: &str = "C O S T";
    pub const WINDOW: &str = "W I N D O W";
    pub const AGENT: &str = "A G E N T";
    pub const YOU: &str = "Y O U";
    pub const READY: &str = "R E A D Y";
    pub const THEME: &str = "T H E M E";
    pub const HALT: &str = "H A L T";
    pub const NAME: &str = "N A M E";
    pub const SEND: &str = "S E N D";
    pub const STEER: &str = "S T E E R";
    pub const AUTHORIZATION: &str = "A U T H O R I Z A T I O N   R E Q U I R E D";
    pub const SECRET: &str = "S E C R E T   R E Q U E S T E D";
    pub const AUTHORIZE: &str = "A U T H O R I Z E";
    pub const DENY: &str = "D E N Y";
    pub const SUBMIT: &str = "S U B M I T";
    pub const CANCEL: &str = "C A N C E L";
    pub const ACCENT: &str = "A C C E N T";
    pub const CUSTOM: &str = "C U S T O M";
    pub const APPLY: &str = "A P P L Y";
    pub const CLOSE: &str = "C L O S E";
    pub const SAVE: &str = "S A V E";
    pub const REPLACE: &str = "R E P L A C E";
    pub const RUNNING: &str = "R U N N I N G";
    pub const OPENING: &str = "O P E N I N G";
    pub const LOCKED: &str = "L O C K E D";
    pub const UNREAD: &str = "U N R E A D";
    pub const PINNED: &str = "P I N N E D";
    pub const OPEN_GROUP: &str = "O P E N";
    pub const HISTORY: &str = "H I S T O R Y";
    pub const NAMED: &str = "N A M E D";
    pub const TODAY: &str = "T O D A Y";
    pub const THIS_WEEK: &str = "T H I S   W E E K";
    pub const THIS_MONTH: &str = "T H I S   M O N T H";
    pub const OLDER: &str = "O L D E R";
    pub const MORE: &str = "M O R E";
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
    fn every_jp_label_glyph_is_embedded_and_nothing_more() {
        let jp = FILES[4];
        let mut wanted: Vec<char> = JP_LABELS.iter().flat_map(|l| l.chars()).collect();
        wanted.sort();
        wanted.dedup();
        for c in &wanted {
            assert!(has_glyph(jp, *c as u32), "{c} missing from the JP subset");
        }
        // Not a CJK font: a few KB, and a common kanji it has no label for is absent.
        assert!(jp.len() < 16 * 1024, "{} bytes", jp.len());
        assert!(!has_glyph(jp, '日' as u32));
        assert!(!has_glyph(jp, 'A' as u32));
        // subset.sh's list is the same set.
        let script = include_str!("../assets/fonts/subset.sh");
        let line = script.lines().find(|l| l.starts_with("JP_LABELS=")).expect("JP_LABELS in subset.sh");
        let mut listed: Vec<char> = line.trim_start_matches("JP_LABELS=").trim_matches('"').chars().collect();
        listed.sort();
        listed.dedup();
        assert_eq!(listed, wanted);
    }

    #[test]
    fn latin_faces_cover_the_ui_glyphs() {
        for (i, font) in FILES[..4].iter().enumerate() {
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

    /// N4: `display_covers` is exactly what the embedded display face has
    /// (Latin-1 less a few signs; Latin Extended-A, U+0100 on, is not in it).
    #[test]
    fn display_covers_what_the_display_face_has() {
        for cp in 0x20..0x3000u32 {
            let Some(c) = char::from_u32(cp) else { continue };
            assert_eq!(display_covers(c), has_glyph(FILES[0], cp), "U+{cp:04X}");
        }
        assert!(display_covers('é') && !display_covers('ł') && !display_covers('ő') && !display_covers('ş'));
    }

    #[test]
    fn tracked_labels_are_tracked_caps() {
        for l in [label::SESSIONS, label::AUTHORIZATION, label::NEEDS_YOU, label::CONTEXT] {
            assert!(l.chars().all(|c| c == ' ' || c == '+' || c.is_ascii_uppercase()), "{l}");
            assert!(!l.contains("  ") || l.contains("   "), "{l}: word gaps are three spaces");
        }
    }
}
