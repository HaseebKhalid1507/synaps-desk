//! Keeping a `Full`-mode history mirror honest, as Synaps'
//! `SocketTransport::absorb` does.
//!
//! The daemon sends the whole `api_messages` in `Attached` and in every
//! `Stream(MessageHistory)`, and after any change a `Conversation` digest
//! (`messages_len` + FNV-1a `messages_hash`). `/clear`, `/resume` and
//! compaction send *only* the digest: a client that does not compare it
//! keeps showing the old conversation, or splices two together.
//!
//! [`HistoryMirror`] keeps the length and the running hash (not the
//! messages), classifies each new history as an append or a replacement,
//! and says when a digest means "re-query `Messages`".

use serde_json::Value;

use crate::protocol::ConversationDigest;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Feed `messages` into a running FNV-1a 64 state, as `wire::messages_hash`
/// does: each message's canonical JSON, then `0x1f`.
fn feed(mut h: u64, messages: &[Value]) -> u64 {
    for m in messages {
        if let Ok(bytes) = serde_json::to_vec(m) {
            for b in bytes {
                h ^= b as u64;
                h = h.wrapping_mul(FNV_PRIME);
            }
        }
        h ^= 0x1f;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

/// `wire::messages_hash`. The daemon serialises maps in sorted key order
/// (its `serde_json` has no `preserve_order`). A parsed message
/// re-serialises sorted here too, or, with `preserve_order` on, in the
/// order it arrived, which is that sorted order: the bytes match either
/// way. Floats parse with `float_roundtrip`, as the daemon's do.
pub fn messages_hash(messages: &[Value]) -> u64 {
    feed(FNV_OFFSET, messages)
}

/// How the folded history changes. Crosses to the UI instead of the raw
/// `MessageHistory`, so the already-folded prefix never does.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum HistoryUpdate {
    /// `messages` follow the first `base_len` messages, which are unchanged.
    Append { base_len: usize, messages: Vec<Value> },
    /// The whole history is `messages` (clear, resume, compaction, a rewrite
    /// that kept or grew the length, or a resync).
    Replace { messages: Vec<Value> },
}

impl HistoryUpdate {
    /// The history length after this update.
    pub fn len(&self) -> usize {
        match self {
            Self::Append { base_len, messages } => base_len + messages.len(),
            Self::Replace { messages } => messages.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Length + running hash of the messages a consumer has folded.
#[derive(Debug, Clone)]
pub struct HistoryMirror {
    len: usize,
    hash: u64,
    /// Cleared when a fresh `Messages` result still disagrees with the
    /// digest on the hash alone: then digests are compared by length only
    /// (rather than re-querying on every round) and every history is a
    /// `Replace` (as `absorb` does), until a digest matches by hash again.
    hash_ok: bool,
}

impl Default for HistoryMirror {
    fn default() -> Self {
        Self { len: 0, hash: FNV_OFFSET, hash_ok: true }
    }
}

impl HistoryMirror {
    pub fn new(messages: &[Value]) -> Self {
        let mut m = Self::default();
        m.reset(messages);
        m
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn hash(&self) -> u64 {
        self.hash
    }

    pub fn hash_trusted(&self) -> bool {
        self.hash_ok
    }

    pub fn reset(&mut self, messages: &[Value]) {
        self.len = messages.len();
        self.hash = messages_hash(messages);
    }

    /// Classify a full `MessageHistory`: an append when its first `len`
    /// messages hash to the mirror's state, else a replacement. While the
    /// hash is distrusted nothing can be told apart: always a replacement.
    pub fn update(&mut self, mut messages: Vec<Value>) -> HistoryUpdate {
        if self.hash_ok && messages.len() >= self.len {
            let prefix = feed(FNV_OFFSET, &messages[..self.len]);
            if prefix == self.hash {
                let base_len = self.len;
                let suffix = messages.split_off(base_len);
                self.hash = feed(prefix, &suffix);
                self.len = base_len + suffix.len();
                return HistoryUpdate::Append { base_len, messages: suffix };
            }
        }
        self.reset(&messages);
        HistoryUpdate::Replace { messages }
    }

    /// The result of a `Messages` query for `digest`: always a replacement.
    pub fn resynced(&mut self, messages: Vec<Value>, digest: Option<&ConversationDigest>) -> HistoryUpdate {
        self.reset(&messages);
        if let Some(d) = digest
            && d.messages_len == self.len
            && d.messages_hash != self.hash
        {
            self.hash_ok = false;
        }
        HistoryUpdate::Replace { messages }
    }

    /// Whether `digest` describes the mirrored messages. A digest that
    /// matches by hash re-arms a distrusted hash.
    pub fn matches(&mut self, digest: &ConversationDigest) -> bool {
        if digest.messages_len != self.len {
            return false;
        }
        if digest.messages_hash == self.hash {
            self.hash_ok = true;
        }
        !self.hash_ok || digest.messages_hash == self.hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msgs(n: usize) -> Vec<Value> {
        (0..n).map(|i| json!({"role": if i % 2 == 0 { "user" } else { "assistant" }, "content": format!("m{i}")})).collect()
    }

    #[test]
    fn running_hash_equals_the_whole_hash() {
        let m = msgs(5);
        assert_eq!(feed(feed(FNV_OFFSET, &m[..2]), &m[2..]), messages_hash(&m));
        assert_eq!(messages_hash(&[]), FNV_OFFSET);
    }

    #[test]
    fn appends_and_rewrites_are_told_apart() {
        let mut mirror = HistoryMirror::new(&msgs(2));
        let up = mirror.update(msgs(4));
        assert_eq!(up, HistoryUpdate::Append { base_len: 2, messages: msgs(4)[2..].to_vec() });
        assert_eq!(mirror.len(), 4);
        // Same length, one message rewritten: a replacement.
        let mut other = msgs(4);
        other[1] = json!({"role": "assistant", "content": "rewritten"});
        assert!(matches!(mirror.update(other.clone()), HistoryUpdate::Replace { .. }));
        assert_eq!(mirror.hash(), messages_hash(&other));
        // Longer, but a different conversation: a replacement, not a splice.
        let resumed: Vec<Value> = (0..6).map(|i| json!({"role": "user", "content": format!("other {i}")})).collect();
        assert!(matches!(mirror.update(resumed), HistoryUpdate::Replace { .. }));
        // Shorter: a replacement.
        assert!(matches!(mirror.update(msgs(1)), HistoryUpdate::Replace { .. }));
    }

    #[test]
    fn digest_mismatch_and_hash_distrust() {
        let m = msgs(3);
        let mut mirror = HistoryMirror::new(&m);
        let good = ConversationDigest { messages_len: 3, messages_hash: messages_hash(&m), ..Default::default() };
        assert!(mirror.matches(&good));
        let same_len_other = ConversationDigest { messages_len: 3, messages_hash: 7, ..Default::default() };
        assert!(!mirror.matches(&same_len_other));
        // The re-query returns these messages, yet the hash still disagrees:
        // stop trusting hashes (length only) instead of re-querying forever.
        let _ = mirror.resynced(m.clone(), Some(&same_len_other));
        assert!(!mirror.hash_trusted());
        assert!(mirror.matches(&same_len_other));
        assert!(!mirror.matches(&ConversationDigest { messages_len: 4, ..Default::default() }));
    }

    /// N1: under distrust no history is append-shaped: a same-length or a
    /// longer rewrite is a `Replace`, never folded onto the old rows. A
    /// digest that matches by hash again restores appends.
    #[test]
    fn distrust_replaces_every_history_until_a_hash_matches() {
        let m = msgs(3);
        let mut mirror = HistoryMirror::new(&m);
        let off = ConversationDigest { messages_len: 3, messages_hash: 7, ..Default::default() };
        let _ = mirror.resynced(m.clone(), Some(&off));
        assert!(!mirror.hash_trusted());

        let mut same_len = m.clone();
        same_len[2] = json!({"role": "user", "content": "repaired"});
        assert_eq!(mirror.update(same_len.clone()), HistoryUpdate::Replace { messages: same_len.clone() });
        let mut longer: Vec<Value> = (0..5).map(|i| json!({"role": "user", "content": format!("rewritten {i}")})).collect();
        assert_eq!(mirror.update(longer.clone()), HistoryUpdate::Replace { messages: longer.clone() });
        assert_eq!(mirror.len(), 5);

        let good = ConversationDigest { messages_len: 5, messages_hash: messages_hash(&longer), ..Default::default() };
        assert!(mirror.matches(&good));
        assert!(mirror.hash_trusted());
        longer.push(json!({"role": "assistant", "content": "next"}));
        assert!(matches!(mirror.update(longer), HistoryUpdate::Append { base_len: 5, .. }));
    }
}
