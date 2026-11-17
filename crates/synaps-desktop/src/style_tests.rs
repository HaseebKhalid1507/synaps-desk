//! The house rules (design.md §1.2, §5, §8.9), read off the sources: a
//! backstop to clippy's `disallowed-methods` (`clippy.toml`), which holds
//! them at compile time: no text under 12 px; no opacity but a chart's
//! data; every tooltip on the one overlay box. Line ends and path
//! separators as the platform has them.

use std::path::Path;

/// Every source file but this one: (name, its code with comments and the
/// test module cut).
fn sources() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    for e in std::fs::read_dir(&dir).expect("src") {
        let path = e.expect("an entry").path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        if !name.ends_with(".rs") || name == "style_tests.rs" || name == "ui_tests.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("a source").replace("\r\n", "\n");
        let code = match text.find("#[cfg(test)]\nmod tests") {
            Some(at) => &text[..at],
            None => &text[..],
        };
        let code: Vec<&str> = code.lines().map(|l| if l.trim_start().starts_with("//") { "" } else { l }).collect();
        out.push((name, code.join("\n")));
    }
    assert!(out.len() > 30, "the sources were found: {}", out.len());
    out
}

/// The top-level arguments of the call whose `(` is at `open`, string
/// literals skipped.
fn args(code: &str, open: usize) -> Vec<&str> {
    let b = code.as_bytes();
    let (mut depth, mut start, mut out, mut quoted) = (0, open + 1, Vec::new(), false);
    let mut i = open;
    while i < b.len() {
        match b[i] {
            b'\\' if quoted => i += 1,
            b'"' => quoted = !quoted,
            _ if quoted => {}
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    out.push(code[start..i].trim());
                    return out;
                }
            }
            b',' if depth == 1 => {
                out.push(code[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

fn line_of(code: &str, at: usize) -> usize {
    code[..at].matches('\n').count() + 1
}

/// No text-size literal under 12 px (design.md §1.2): not in
/// `text_px(…)`, nor as the size of the text helpers (`display`, `mono`,
/// `sans`); no size set but by `fonts::TextPx`; no kit size under 12.
#[test]
fn no_text_under_12_px() {
    let mut bad = Vec::new();
    for (name, code) in sources() {
        for call in ["display(", "mono(", "sans(", "text_px("] {
            for (at, _) in code.match_indices(call) {
                // A whole name: not `foo_mono(`.
                if call != "text_px(" && code[..at].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                let open = at + call.len() - 1;
                let a = args(&code, open);
                let size = if call == "text_px(" { a.first() } else { a.get(1) };
                if let Some(v) = size.and_then(|s| s.parse::<f32>().ok())
                    && v < 12.
                {
                    bad.push(format!("{name}:{} {call}… {v}", line_of(&code, at)));
                }
            }
        }
        // Sizes are set only by `fonts::TextPx`.
        if name != "fonts.rs" {
            for set in [".text_size(", ".text_style()"] {
                for (at, _) in code.match_indices(set) {
                    bad.push(format!("{name}:{} {set}", line_of(&code, at)));
                }
            }
        }
        for small in [".text_xs()", ".text_2xs()"] {
            for (at, _) in code.match_indices(small) {
                bad.push(format!("{name}:{} {small}", line_of(&code, at)));
            }
        }
    }
    assert!(bad.is_empty(), "text under 12 px:\n{}", bad.join("\n"));
}

/// `motion::lift`, from its `#[allow]` to its closing brace: in motion.rs
/// the one place opacity (and its lint's allow) may be.
fn lift_span(code: &str) -> Vec<std::ops::Range<usize>> {
    ["lift", "appear"]
        .into_iter()
        .map(|f| {
            let head = format!("#[allow(clippy::disallowed_methods)]\npub(crate) fn {f}");
            let start = code.find(&head).expect("motion::lift and motion::appear, allowed directly above");
            let end = start + code[start..].find("\n}\n").expect("its end") + 2;
            start..end
        })
        .collect()
}

/// No state is drawn with opacity (design.md §5), by any of its names:
/// only a chart's data fades (`chart::fade`), something arriving lifts
/// from 70 % (`motion::lift`, fluid.md law 3), and an overlay enters and
/// leaves (`motion::appear`, fluid.md §3.10): in motion.rs, inside those
/// two only, once each.
#[test]
fn no_opacity_outside_charts() {
    let mut lifts = 0;
    let bad: Vec<String> = sources()
        .into_iter()
        .filter(|(name, _)| name != "chart.rs")
        .flat_map(|(name, code)| {
            let span = (name == "motion.rs").then(|| lift_span(&code));
            let mut bad = Vec::new();
            for o in [".opacity(", ".alpha(", ".fade_out("] {
                for (at, _) in code.match_indices(o) {
                    if span.as_ref().is_some_and(|s| s.iter().any(|s| s.contains(&at))) {
                        lifts += 1;
                    } else {
                        bad.push(format!("{name}:{} {o}", line_of(&code, at)));
                    }
                }
            }
            bad
        })
        .collect();
    assert!(bad.is_empty(), "opacity outside chart.rs and motion::lift:\n{}", bad.join("\n"));
    assert_eq!(lifts, 2, "motion::lift and motion::appear set opacity once each");
}

/// No gradients anywhere (polish.md: "No gradients anywhere, on any card
/// or control"; §6 finish gate 6): every fill is one opaque colour of the
/// palette (the sweep's levels are four opaque steps), and no shadow on a
/// card (GPUI's `shadow_sm()` and kin too; `shadow_none()` takes one away).
#[test]
fn no_gradients_anywhere() {
    let bad: Vec<String> = sources()
        .into_iter()
        .filter(|(name, _)| name != "style_tests.rs")
        .flat_map(|(name, code)| {
            ["linear_gradient", "LinearColorStop", "radial_gradient", "conic_gradient", ".shadow(", "shadow_", "box_shadow(", "BoxShadow"]
                .into_iter()
                .flat_map(|g| {
                    code.match_indices(g)
                        .filter(|(at, _)| !code[*at..].starts_with("shadow_none("))
                        .map(|(at, _)| format!("{name}:{} {g}", line_of(&code, at)))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(bad.is_empty(), "a gradient or a shadow:\n{}", bad.join("\n"));
}

/// No press on a transcript row or a worker chip yet (Q4 leaves "press on
/// tray rows, run headers and chips" for later): Q1's rows and chips
/// (`turns.rs`, `tray.rs`) have hover fills only, so their inks (`err`,
/// `warn`, `think`, `sub_*`) sit on no press fill. Wiring one adds those
/// inks to a press set (`theme.rs` `pressed`) and the set to
/// `press_is_visible_in_every_theme`, then drops its file here.
#[test]
fn rows_and_chips_have_no_press_yet() {
    for (name, code) in sources().into_iter().filter(|(n, _)| ["turns.rs", "tray.rs"].contains(&n.as_str())) {
        for g in ["press_ch", "press_k", "btn_press", "warn_press", ".active(", "fx.fill(", ".lit("] {
            assert!(!code.contains(g), "{name}: a press ({g}): add its inks to a press set");
        }
    }
}

/// One overlay box (design.md §8.9): every tooltip is built by
/// `overlay.rs`, on `card` with its `frame`.
#[test]
fn every_tooltip_is_the_overlay() {
    let bad: Vec<String> = sources()
        .into_iter()
        .filter(|(name, _)| name != "overlay.rs")
        .flat_map(|(name, code)| {
            ["Tooltip::", "Tooltip as", "tooltip::{"]
                .into_iter()
                .flat_map(|t| code.match_indices(t).map(|(at, _)| format!("{name}:{}", line_of(&code, at))).collect::<Vec<_>>())
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(bad.is_empty(), "a tooltip not built by overlay.rs:\n{}", bad.join("\n"));
}

/// The lint's way in, one per rule: `clippy::disallowed_methods` is
/// allowed only where its rule lives (fonts.rs, chart.rs, overlay.rs, and
/// in motion.rs directly above `lift` and `appear`);
/// the groups that hold it (`clippy::style`, `clippy::all`) nowhere.
#[test]
fn the_house_rules_are_allowed_only_where_they_live() {
    let mut bad = Vec::new();
    for (name, code) in sources() {
        let home = ["fonts.rs", "chart.rs", "overlay.rs"].contains(&name.as_str());
        let lift = (name == "motion.rs").then(|| lift_span(&code));
        for lint in ["disallowed_methods", "clippy::style", "clippy::all"] {
            if lint == "disallowed_methods" && home {
                continue;
            }
            for (at, _) in code.match_indices(lint) {
                // The one `#[allow]`, the line above `lift`.
                if lint == "disallowed_methods" && lift.as_ref().is_some_and(|s| s.iter().any(|s| (s.start..s.start + 40).contains(&at))) {
                    continue;
                }
                // A whole name: not `clippy::all_…`.
                if !code[at + lint.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_') {
                    bad.push(format!("{name}:{} {lint}", line_of(&code, at)));
                }
            }
        }
    }
    assert!(bad.is_empty(), "the house rules allowed outside their home:\n{}", bad.join("\n"));
}
