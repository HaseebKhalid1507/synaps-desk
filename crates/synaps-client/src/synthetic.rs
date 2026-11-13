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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{ItemKind, Transcript};

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
