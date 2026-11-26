//! D5's desk side after Shady's review of #101 (`review-pr-desk101-shady.md`):
//! the edit preview's text drawn as D1 draws agent text (P2-9). Each fails on
//! `653c30c`.

use serde_json::{Value, json};

/// P2-9: an edit preview's lines are drawn as D1 draws agent text: a
/// successful edit, write or multi-edit whose text holds bidi controls,
/// isolates, control characters or invisibles reaches the row without
/// them (the requested edit, and the ledger, keep their bytes).
#[test]
fn edit_preview_lines_are_sanitized() {
    let evil = "let s = \"\u{202E}lave\u{202C}\"; \u{2066}x\u{2069} \u{200B}zw\u{0007}bell\u{001B}[31m";
    let tool = |id: &str, name: &str, input: Value| json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": name, "input": input}]});
    let ok = |id: &str| json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": "ok"}]});
    let msgs = vec![
        json!({"role": "user", "content": "go"}),
        tool("t1", "edit", json!({"path": "/w/a.rs", "old_string": "x", "new_string": evil})),
        ok("t1"),
        tool("t2", "write", json!({"path": "/w/a.rs", "content": format!("{evil}\nline two")})),
        ok("t2"),
        tool("t3", "multi_edit", json!({"path": "/w/a.rs", "edits": [{"old_string": "y", "new_string": evil}]})),
        ok("t3"),
    ];
    let mut t = synaps_client::Transcript::new();
    t.apply_history(&msgs);
    let mut ledger = crate::files::Ledger::default();
    ledger.reset(&msgs, None, false);
    let (groups, _) = crate::blocks::edit_preview(t.items(), &ledger, "/w/a.rs");
    assert!(groups.len() >= 2, "the edits drawn: {}", groups.len());
    let bad = |c: char| matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200B}') || (c.is_control() && c != '\t');
    for g in &groups {
        for l in &g.lines {
            assert!(!l.text.chars().any(bad), "a raw character reached the row: {:?}", l.text);
        }
    }
    assert!(groups.iter().flat_map(|g| &g.lines).any(|l| l.text.contains("lave")), "the text itself is drawn");
}
