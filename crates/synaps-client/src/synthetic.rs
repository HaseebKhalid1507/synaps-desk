//! Deterministic synthetic load for benches and tests: a long transcript
//! (`api_messages` shape, with code blocks, thinking and tool calls) and a
//! markdown delta stream. No tokens are spent.

use serde_json::{Value, json};

/// xorshift64*: deterministic, dependency-free.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }
}

const WORDS: &[&str] = &[
    "daemon",
    "session",
    "frame",
    "socket",
    "render",
    "layout",
    "virtual",
    "list",
    "token",
    "stream",
    "buffer",
    "thread",
    "channel",
    "markdown",
    "highlight",
    "window",
    "compositor",
    "latency",
    "memory",
    "the",
    "a",
    "of",
    "to",
    "and",
    "is",
    "in",
    "that",
    "for",
    "with",
    "on",
    "we",
    "this",
    "it",
    "as",
];

fn sentence(r: &mut Rng) -> String {
    let n = 6 + r.below(14) as usize;
    let mut s = String::new();
    for i in 0..n {
        if i > 0 {
            s.push(' ');
        }
        let w = WORDS[r.below(WORDS.len() as u64) as usize];
        if r.below(17) == 0 {
            s.push('`');
            s.push_str(w);
            s.push('`');
        } else if r.below(23) == 0 {
            s.push_str("**");
            s.push_str(w);
            s.push_str("**");
        } else {
            s.push_str(w);
        }
    }
    let mut c = s.chars();
    match c.next() {
        Some(f) => format!("{}{}.", f.to_uppercase(), c.as_str()),
        None => s,
    }
}

const RUST: &str = "fn main() {\n    let frames: Vec<u64> = (0..120).collect();\n    let p95 = frames[frames.len() * 95 / 100];\n    println!(\"p95 = {p95}\");\n}\n";
const PY: &str =
    "def handler(event):\n    if event[\"ev\"] == \"stream\":\n        return event[\"event\"].get(\"text\", \"\")\n    return None\n";
const SH: &str = "cargo build --release -p synaps-desktop\nls -la target/release | tail -n 3\n";

fn code(r: &mut Rng) -> String {
    match r.below(3) {
        0 => format!("```rust\n{RUST}```"),
        1 => format!("```python\n{PY}```"),
        _ => format!("```bash\n{SH}```"),
    }
}

/// A markdown answer: paragraphs, a list, sometimes a code block.
pub fn answer(r: &mut Rng) -> String {
    let mut parts = Vec::new();
    for _ in 0..1 + r.below(3) {
        let mut p = String::new();
        for _ in 0..1 + r.below(4) {
            p.push_str(&sentence(r));
            p.push(' ');
        }
        parts.push(p.trim_end().to_string());
    }
    if r.below(3) == 0 {
        parts.push((0..3).map(|_| format!("- {}", sentence(r))).collect::<Vec<_>>().join("\n"));
    }
    if r.below(2) == 0 {
        parts.push(code(r));
        parts.push(sentence(r));
    }
    parts.join("\n\n")
}

/// `n` api messages: user / assistant turns, some with thinking and a tool
/// call (tool_use, then a user tool_result, then a closing answer).
pub fn history(n: usize, seed: u64) -> Vec<Value> {
    let mut r = Rng::new(seed);
    let mut out = Vec::with_capacity(n);
    let mut tool = 0u64;
    while out.len() < n {
        out.push(json!({"role": "user", "content": sentence(&mut r)}));
        if out.len() >= n {
            break;
        }
        let mut blocks = Vec::new();
        if r.below(4) == 0 {
            blocks.push(json!({"type": "thinking", "thinking": format!("{} {}", sentence(&mut r), sentence(&mut r)), "signature": "x"}));
        }
        blocks.push(json!({"type": "text", "text": answer(&mut r)}));
        if r.below(5) == 0 && out.len() + 3 <= n {
            tool += 1;
            let id = format!("toolu_{tool:06}");
            blocks.push(json!({"type": "tool_use", "id": id, "name": "bash", "input": {"command": "ls -la | head -n 3"}}));
            out.push(json!({"role": "assistant", "content": blocks}));
            out.push(json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": "total 8\ndrwxr-xr-x 2 user user 4096 .\n"}]}));
            out.push(json!({"role": "assistant", "content": [{"type": "text", "text": sentence(&mut r)}]}));
        } else {
            out.push(json!({"role": "assistant", "content": blocks}));
        }
    }
    out.truncate(n);
    out
}

/// A long markdown document cut into 3–15 char deltas, like a model stream.
pub fn deltas(total_chars: usize, seed: u64) -> Vec<String> {
    let mut r = Rng::new(seed);
    let mut doc = String::new();
    while doc.len() < total_chars {
        doc.push_str(&answer(&mut r));
        doc.push_str("\n\n");
    }
    let chars: Vec<char> = doc.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let n = (3 + r.below(13) as usize).min(chars.len() - i);
        out.push(chars[i..i + n].iter().collect());
        i += n;
    }
    out
}

// ── blocks (synaps-cards docs/blocks.md v1.3) ─────────────────────────

/// A `cards_block` call's input for `ops`.
pub fn block_input(ops: Value) -> Value {
    json!({ "ops": ops })
}

/// The extension's result for a block call of `ops`: its receipt
/// (blocks.md §1.6), which binds it.
pub fn block_receipt(ops: &Value) -> String {
    match cards_spec::block::parse_call(&block_input(ops.clone())) {
        Ok(b) => b.receipt().to_string(),
        Err(e) => format!("cards_block: {e}"),
    }
}

/// An assistant message of `blocks` (text, `tool_use`).
fn said(blocks: Vec<Value>) -> Value {
    json!({"role": "assistant", "content": blocks})
}

fn text(t: &str) -> Value {
    json!({"type": "text", "text": t})
}

fn tool(id: &str, name: &str, input: Value) -> Value {
    json!({"type": "tool_use", "id": id, "name": name, "input": input})
}

fn ops(id: &str, ops: Value) -> Value {
    tool(id, "cards_block", block_input(ops))
}

fn results(rs: Vec<(&str, String)>) -> Value {
    json!({"role": "user", "content": rs.into_iter().map(|(id, r)| json!({"type": "tool_result", "tool_use_id": id, "content": r})).collect::<Vec<_>>()})
}

/// The card [`blocks_history`] asks, and its id.
pub const BLOCKS_CARD: &str = "c4f9x2a";

fn lock_card() -> Value {
    json!({"question": "Which lock wait?", "detail": "p95 of a rotation is 1.1 s; the upstream gives up at 1.5 s.",
        "options": [{"label": "900 ms", "hint": "under p95"}, "1200 ms", "1500 ms"], "default": "1200 ms"})
}

/// A session whose agent keeps blocks (blocks.md §1.5), every kind and
/// most states, as the extension and the history would hold them: a plan
/// with steps in each state (one failed, one skipped, notes), a status
/// replaced by another (`r1 · replaced`), an edit preview and a proposal,
/// a note, a metric with a series, a file, a group of two (`merge`), a
/// folded diff, a run titled by the agent (`collapse` with `run`), a card
/// turned into its outcome (`card:` replace), a refused call (a plain
/// row), a skipped path (the `skipped` chip). Deterministic: tests, the
/// demo (`--offline`) and the bench read the same.
pub fn blocks_history() -> Vec<Value> {
    let edit = json!({"path": "crates/relay/src/token_store.rs",
        "old_string": "    let cur = self.load(id).await?;\n    let next = self.upstream.rotate(&cur).await?;",
        "new_string": "    let slot = self.slots.entry(id).or_default().clone();\n    let _held = timeout(self.lock_wait, slot.lock()).await\n        .map_err(|_| Error::Timeout)?;\n    let cur = self.load(id).await?;"});
    let o1 = json!([
        {"op":"put","id":"now","kind":"status","title":"Working on","body":{"text":"Reading the relay's token path","detail":"3 of 7 files read","progress":{"n":3,"of":7},"state":"working"}},
        {"op":"put","id":"plan","kind":"plan","title":"Close the refresh race","body":{"steps":[
            {"id":"trace","label":"Trace issuance and verification paths","state":"done"},
            {"id":"lock","label":"Serialize rotate() per session","state":"doing","note":"token_store.rs:118"},
            {"id":"test","label":"Regression test: 32 concurrent refreshes","state":"todo"},
            {"id":"bandaid","label":"Drop the retry band-aid","state":"todo"},
            {"id":"deploy","label":"Deploy to staging","state":"todo"}]}}
    ]);
    let o2 = json!([{"op":"collapse","run":true,"summary":"Explored the relay's refresh path"}]);
    let o3 = json!([
        {"op":"put","id":"edits","kind":"diff","title":"Serialize rotate()","body":{"file":"crates/relay/src/token_store.rs","state":"editing","from":"edits"}},
        {"op":"put","id":"file","kind":"file","body":{"path":"crates/relay/src/token_store.rs","line":118,"note":"TokenStore::rotate","state":"editing"}},
        {"op":"patch","id":"now","set":{"text":"Serializing rotate() per session","detail":"one lock per session id"}},
        {"op":"patch","id":"plan","state":{"lock":"done","test":"doing"}}
    ]);
    let o4 = json!([
        {"op":"put","id":"cause","kind":"note","title":"Root cause","body":{"text":"Two refreshes read the same token before either rotates it. The second rotation **invalidates** the first one's token, so the client that won the race gets a dead token back.\n\nThe retry band-aid hid it: a retry reads the *new* token."}},
        {"op":"put","id":"rot","kind":"metric","title":"Upstream rotations per burst","body":{"label":"rotations per 32 refreshes","value":"1.0","was":"2.0","series":[2,2,2,1.5,1,1],"state":"ok"}},
        {"op":"put","id":"retry","kind":"diff","title":"Drop the band-aid","body":{"file":"crates/relay/src/session.rs","state":"proposed","hunks":[
            {"id":"h1","at":"Session::refresh()","lines":["-        match self.relay.refresh(&self.token).await {","-            Err(Error::Stale) => self.relay.refresh(&self.token).await,","-            r => r,","-        }","+        self.relay.refresh(&self.token).await"],"state":"open"},
            {"id":"h2","at":"mod retry;","lines":["-mod retry;"],"state":"open"}]}}
    ]);
    let o5 = json!([{"op":"replace","id":"card:c4f9x2a","kind":"note","body":{"text":"Lock wait set to **900 ms**: under p95, and well before the upstream gives up."}}]);
    let o6 = json!([
        {"op":"merge","into":"why","title":"Why the cutoff belongs to the session","ids":["cause","rot"]},
        {"op":"replace","id":"now","kind":"status","title":"Working on","body":{"text":"Running the regression test","detail":"32 concurrent refreshes","progress":{"n":31,"of":32},"state":"working"},"why":"the lock is in"},
        {"op":"patch","id":"plan","state":{"test":"failed","bandaid":"skipped","deploy":"todo"},"set":{"steps.test.note":"1 of 32 rotated twice"}},
        {"op":"collapse","id":"edits"}
    ]);
    let o7 = json!([{"op":"patch","id":"plan","remove":["steps.nope"]}]);
    let refused = json!([{"op":"put","id":"x","kind":"plan","body":{"steps":[{"id":"a","label":"A","state":"finished"}]}}]);
    let card = lock_card();
    let Ok(cards_spec::Input::Create { doc, wait }) = cards_spec::parse_input(&card) else { unreachable!("the demo card validates") };
    let id = cards_spec::CardId::parse(BLOCKS_CARD).expect("a card id");
    let showing = format!(
        "{}\nIts answer will come as a user message starting \"[card {BLOCKS_CARD}]\".",
        cards_spec::Envelope::showing(id.clone(), &doc, wait)
    );
    let answer = cards_spec::message::build(
        &id,
        &doc,
        &cards_spec::Reply::Answer(cards_spec::parse_answer(&doc, &json!({"choice": "900 ms"})).expect("an answer")),
    );
    vec![
        json!({"role": "user", "content": "The refresh race is back on staging: two rotations inside one second for the same session. Fix it properly this time and drop the retry band-aid."}),
        said(vec![text("Looking at the relay's refresh path first."), ops("toolu_b01", o1.clone())]),
        results(vec![("toolu_b01", block_receipt(&o1))]),
        said(vec![
            tool("toolu_r01", "read", json!({"path": "crates/relay/src/token_store.rs"})),
            tool("toolu_r02", "grep", json!({"pattern": "rotate\\(", "path": "crates/relay"})),
            tool("toolu_r03", "read", json!({"path": "crates/relay/src/session.rs"})),
            ops("toolu_b02", o2.clone()),
        ]),
        results(vec![
            ("toolu_r01", "pub async fn rotate(&self, id: SessionId) -> Result<Token> {\n    let cur = self.load(id).await?;".into()),
            ("toolu_r02", "crates/relay/src/token_store.rs:118:    pub async fn rotate(\ncrates/relay/src/session.rs:61:        self.store.rotate(self.id)".into()),
            ("toolu_r03", "impl Session {\n    pub async fn refresh(&self) -> Result<Token> {".into()),
            ("toolu_b02", block_receipt(&o2)),
        ]),
        said(vec![text("Two refreshes race on `rotate()`. One lock per session closes it."), tool("toolu_e01", "edit", edit), ops("toolu_b03", o3.clone())]),
        results(vec![("toolu_e01", "Edited crates/relay/src/token_store.rs".into()), ("toolu_b03", block_receipt(&o3))]),
        said(vec![ops("toolu_b04", o4.clone()), tool("toolu_c01", "cards_ui", card.clone())]),
        results(vec![("toolu_b04", block_receipt(&o4)), ("toolu_c01", showing)]),
        json!({"role": "user", "content": answer}),
        said(vec![
            text("900 ms it is."),
            ops("toolu_b05", o5.clone()),
            ops("toolu_b06", o6.clone()),
            ops("toolu_b07", o7.clone()),
            ops("toolu_b08", refused),
        ]),
        results(vec![
            ("toolu_b05", block_receipt(&o5)),
            ("toolu_b06", block_receipt(&o6)),
            ("toolu_b07", block_receipt(&o7)),
            ("toolu_b08", "cards_block: ops[0].body.steps[0].state: \"finished\" is not a plan state (todo, doing, done, failed, skipped)".into()),
        ]),
        said(vec![text("The lock is in and the band-aid is gone. One refresh in 32 still rotated twice: looking at the cutoff next.")]),
    ]
}

/// The plan [`rail_history`] keeps: 20 steps, the first 6 done, the 7th
/// doing (blocks.md §6.2's "a full 20-step plan at rail width").
fn rail_plan() -> Value {
    let labels = [
        "Trace issuance and verification paths",
        "Find every caller of rotate()",
        "Reproduce the race with 32 refreshes",
        "Read the upstream's rotation contract",
        "Measure p95 of a rotation",
        "Pick a lock wait under the upstream's cutoff",
        "Serialize rotate() per session",
        "Regression test: 32 concurrent refreshes",
        "Drop the retry band-aid",
        "Check the cutoff belongs to the session",
        "Audit the token cache's eviction",
        "Load test at 10× staging traffic",
        "Update the relay runbook",
        "Note the lock in the design doc",
        "Review with the relay owners",
        "Deploy to staging",
        "Watch staging for a day",
        "Deploy to production",
        "Watch production for a day",
        "Close the incident",
    ];
    let steps: Vec<Value> = labels
        .iter()
        .enumerate()
        .map(|(k, l)| {
            let state = match k {
                0..6 => "done",
                6 => "doing",
                _ => "todo",
            };
            json!({"id": format!("s{:02}", k + 1), "label": l, "state": state})
        })
        .collect();
    json!({"steps": steps})
}

/// A session whose agent molds the rail (blocks.md §4, D6): sections in
/// both slots, more than the cap of 4 expanded wants, every kind, and a
/// group. `upper`: a 20-step plan, a status, a long note, a file; `end`:
/// a metric with a series, a proposed diff of 14 lines, and a group of two
/// notes (created folded). In drawing order the four places go to the
/// plan, the status, the note and the file; the metric and the diff are
/// the end slot's `+2 more`. Deterministic: tests, the demo and the
/// bench read the same.
pub fn rail_history() -> Vec<Value> {
    let note = "Two refreshes read the same token before either rotates it. The second rotation invalidates the first one's token, \
        so the client that won the race gets a dead token back.\n\nThe retry band-aid hid it: a retry reads the new token, so the \
        failure only showed as a slow refresh. Under load the retries stacked up behind the upstream's limiter and some \
        refreshes gave up at the 1.5 s cutoff.\n\nThe fix serializes rotate() per session id. A refresh that finds the lock \
        held waits for it, then reads the token the holder wrote, so it never rotates again. The lock wait has to stay \
        under the upstream's cutoff: p95 of a rotation is 1.1 s, so 900 ms leaves room for the read after it.\n\nThe \
        regression test runs 32 refreshes at once and counts the upstream rotations: one, every time.";
    let o1 = json!([
        {"op":"put","id":"plan","kind":"plan","title":"Close the refresh race","target":"rail","slot":"upper","body":rail_plan()},
        {"op":"put","id":"now","kind":"status","title":"Working on","target":"rail","slot":"upper","body":{"text":"Running the regression test","detail":"32 concurrent refreshes","progress":{"n":31,"of":32},"state":"working"}},
        {"op":"put","id":"cause","kind":"note","title":"Root cause","target":"rail","slot":"upper","body":{"text":note}},
        {"op":"put","id":"file","kind":"file","target":"rail","slot":"upper","body":{"path":"crates/relay/src/token_store.rs","line":118,"note":"TokenStore::rotate","state":"editing"}}
    ]);
    let o2 = json!([
        {"op":"put","id":"rot","kind":"metric","title":"Upstream rotations per burst","target":"rail","slot":"end","body":{"label":"rotations per 32 refreshes","value":"1.0","was":"2.0","series":[2,2,2,2,1.5,2,1.5,1,1,1,1,1],"state":"ok"}},
        {"op":"put","id":"retry","kind":"diff","title":"Drop the band-aid","target":"rail","slot":"end","body":{"file":"crates/relay/src/session.rs","state":"proposed","hunks":[
            {"id":"h1","at":"Session::refresh()","lines":["-        match self.relay.refresh(&self.token).await {","-            Err(Error::Stale) => {","-                tracing::warn!(\"stale token, retrying\");","-                self.relay.refresh(&self.token).await","-            }","-            r => r,","-        }","+        self.relay.refresh(&self.token).await"],"state":"open"},
            {"id":"h2","at":"mod retry;","lines":["-mod retry;","-pub use retry::Retry;"],"state":"open"},
            {"id":"h3","at":"Session::new()","lines":["-            retry: Retry::default(),"],"state":"open"}]}},
        {"op":"put","id":"n1","kind":"note","title":"Trace","target":"rail","slot":"end","body":{"text":"Both refreshes load generation 41, both rotate: 42 and 43."}},
        {"op":"put","id":"n2","kind":"note","title":"Upstream","target":"rail","slot":"end","body":{"text":"The upstream revokes 42 as 43 is issued."}},
        {"op":"merge","into":"findings","title":"What the trace showed","ids":["n1","n2"]}
    ]);
    vec![
        json!({"role": "user", "content": "The refresh race is back on staging. Fix it properly, and keep the plan where I can see it."}),
        said(vec![text("Putting the plan in the rail."), ops("toolu_rl01", o1.clone())]),
        results(vec![("toolu_rl01", block_receipt(&o1))]),
        said(vec![tool("toolu_rl02", "read", json!({"path": "crates/relay/src/session.rs"})), ops("toolu_rl03", o2.clone())]),
        results(vec![
            ("toolu_rl02", "impl Session {\n    pub async fn refresh(&self) -> Result<Token> {".into()),
            ("toolu_rl03", block_receipt(&o2)),
        ]),
        said(vec![text("The lock is in. Running the regression test next.")]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{ItemKind, Transcript};

    /// Every block call of [`rail_history`] binds (its result a receipt,
    /// never a refusal), and its board places its sections as the doc says.
    #[test]
    fn the_rail_history_binds_and_places_both_slots() {
        let h = rail_history();
        for m in &h {
            for r in m["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "tool_result") {
                let text = r["content"].as_str().unwrap_or_default();
                assert!(!text.starts_with("cards_block:"), "refused: {text}");
            }
        }
        let (board, _) = cards_spec::board::Board::of_history(&h);
        let names = |slot| board.committed().list(cards_spec::board::Place::Rail(slot)).iter().map(|r| r.to_string()).collect::<Vec<_>>();
        assert_eq!(names(cards_spec::block::Slot::Upper), ["plan", "now", "cause", "file"]);
        assert_eq!(names(cards_spec::block::Slot::End), ["rot", "retry", "findings"]);
        assert_eq!(rail_history(), h, "deterministic");
    }

    #[test]
    fn history_is_deterministic_and_folds() {
        let h = history(2000, 7);
        assert_eq!(h.len(), 2000);
        assert_eq!(h, history(2000, 7));
        let mut t = Transcript::new();
        t.apply_history(&h);
        assert!(t.len() >= 2000 / 2);
        assert!(t.items().iter().any(|i| i.kind == ItemKind::Tool));
        assert!(t.items().iter().any(|i| i.text.contains("```")));
    }

    #[test]
    fn deltas_cover_the_document() {
        let d = deltas(5000, 1);
        assert!(d.iter().map(String::len).sum::<usize>() >= 5000);
        assert!(d.iter().all(|s| !s.is_empty() && s.chars().count() <= 15));
    }
}
