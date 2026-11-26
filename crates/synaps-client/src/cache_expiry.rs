//! `SessionQuery::CacheExpiry` (`{"query": "cache_expiry"}`, protocol 3,
//! `Welcome.features: cache_expiry`): what a session's own requests show
//! about the provider's prompt cache for its next request. Evidence, not
//! provider state.
//!
//! Mirrors of Synaps' `session/cache_expiry.rs` (the reply and its
//! recommended rendering) and `session/query.rs` (a typed query's error),
//! SynapsCLI #462, field for field and word for word. The reply's enums read
//! an unknown value as `unknown`, an error's unknown code as `internal`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `CacheExpiry.state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CacheState {
    Warm,
    Cold,
    #[serde(other)]
    Unknown,
}

/// `CacheExpiry.reason`. Append-only; an unknown reason reads as `unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CacheReason {
    Expired,
    Uncached,
    MayHaveExpired,
    Unmeasured,
    NoMatchingRequest,
    Busy,
    Parked,
    NotAnthropic,
    HeadUnresolved,
    HistoryUnsettled,
    RolloverPending,
    ProbeFailed,
    #[serde(other)]
    Unknown,
}

/// The `SessionQuery::CacheExpiry` reply. Every field after `state` is
/// optional; durations are whole seconds from the daemon's emit (lower
/// bounds floored, upper bounds ceiled).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CacheExpiry {
    pub state: CacheState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<CacheReason>,
    /// `warm`: about how long the last accepted request's entry has left
    /// under the TTL the server is known to honour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warm_for_secs: Option<u64>,
    /// When this session's last possible refresh of the entry ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lapse_by_secs: Option<u64>,
    /// The TTL `warm_for_secs` counts: the one the server is known to
    /// honour for the history entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof_ttl_secs: Option<u64>,
    /// The TTL requested for the history entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_ttl_secs: Option<u64>,
    /// Cache reads plus writes the provider reported for that request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof_cache_tokens: Option<u64>,
    /// While a keep-warm period is active: the next scheduled attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_ping_in_secs: Option<u64>,
}

impl CacheExpiry {
    pub fn unknown(reason: CacheReason) -> Self {
        Self {
            state: CacheState::Unknown,
            reason: Some(reason),
            warm_for_secs: None,
            lapse_by_secs: None,
            proof_ttl_secs: None,
            requested_ttl_secs: None,
            proof_cache_tokens: None,
            next_ping_in_secs: None,
        }
    }
}

/// The recommended rendering (Synaps `daemon-mode.md`, "Prompt-cache
/// evidence"): the TUI's `/keepwarm status` uses it word for word.
pub fn render_cache_expiry(c: &CacheExpiry) -> String {
    // A lower bound rounds down, an upper bound up.
    let at_least = |s: u64| if s < 60 { format!("{s}s") } else { format!("{}m", s / 60) };
    let at_most = |s: u64| if s < 60 { format!("{s}s") } else { format!("{}m", s.div_ceil(60)) };
    let mut out = match (c.state, c.reason) {
        (CacheState::Warm, _) => match c.warm_for_secs {
            Some(s) => format!("Recent cache activity · about {} left", at_least(s)),
            None => "Recent cache activity".to_string(),
        },
        (CacheState::Unknown, Some(CacheReason::MayHaveExpired)) => {
            let mut s = "May have expired".to_string();
            if let Some(l) = c.lapse_by_secs {
                s.push_str(&format!(" · this session's last possible refresh ends in about {}", at_most(l)));
            }
            s
        }
        (CacheState::Cold, Some(CacheReason::Uncached)) => "Last measured request reported no cache activity".to_string(),
        (CacheState::Cold, _) => "No unexpired evidence from this session".to_string(),
        (CacheState::Unknown, reason) => match reason {
            Some(CacheReason::Parked) => "parked",
            Some(CacheReason::Busy) => "a turn is running",
            Some(CacheReason::HistoryUnsettled) => "last turn didn't finish",
            Some(CacheReason::NoMatchingRequest) => "no matching request yet",
            Some(CacheReason::Unmeasured) => "last request not measured yet",
            Some(CacheReason::RolloverPending) => "next turn starts a new context",
            Some(CacheReason::NotAnthropic) => "not tracked for this provider",
            _ => "couldn't check",
        }
        .to_string(),
    };
    if let Some(tokens) = c.proof_cache_tokens.filter(|_| c.state == CacheState::Warm) {
        out.push_str(&format!(" · {tokens} tokens read or written by the last measured request"));
    }
    if let Some(n) = c.next_ping_in_secs {
        out.push_str(&format!(" · next keep-warm attempt in about {}", at_least(n)));
    }
    out
}

/// Why a typed query has no answer. Append-only; an unknown code reads as
/// `internal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum QueryErrorCode {
    /// Too many jobs, or the blocking budget is spent: ask again later.
    Busy,
    /// The job ran past the daemon's deadline (5 s).
    Timeout,
    /// A newer ask of the same query from the same requester replaced it.
    Superseded,
    /// What the answer was computed from changed before it was sent: ask
    /// once more.
    Stale,
    /// The session cannot answer (a failed wake, ending).
    Unavailable,
    #[serde(other)]
    Internal,
}

/// A typed query's failure: `QueryResult.value` is then
/// `{"error": {"code": .., "message": ..}}`, never the success struct.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryError {
    pub code: QueryErrorCode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl QueryError {
    pub fn new(code: QueryErrorCode) -> Self {
        Self { code, message: None }
    }

    pub fn with_message(code: QueryErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: Some(message.into()) }
    }

    /// The `QueryResult.value` that carries this error.
    pub fn to_value(&self) -> Value {
        serde_json::json!({ "error": self })
    }
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = serde_json::to_value(self.code).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        match &self.message {
            Some(m) => write!(f, "{code}: {m}"),
            None => f.write_str(&code),
        }
    }
}

/// Read a typed query's `QueryResult.value`: its success struct, or the
/// error it carries (a bare `{"error": "text"}` reads as `internal`).
pub fn decode_query_reply<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T, QueryError> {
    if let Some(e) = value.get("error") {
        return Err(serde_json::from_value(e.clone())
            .unwrap_or_else(|_| QueryError { code: QueryErrorCode::Internal, message: e.as_str().map(str::to_string) }));
    }
    serde_json::from_value(value.clone()).map_err(|e| QueryError::with_message(QueryErrorCode::Internal, e.to_string()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn says(c: &CacheExpiry) -> (CacheState, Option<CacheReason>) {
        (c.state, c.reason)
    }

    fn reply(v: Value) -> CacheExpiry {
        serde_json::from_value(v).unwrap()
    }

    /// Synaps' `the_wire_is_append_only_and_renders_as_documented`, its
    /// answers as the daemon sends them.
    #[test]
    fn the_wire_is_append_only_and_renders_as_documented() {
        let v = reply(json!({"state": "lukewarm", "reason": "meteor"}));
        assert_eq!(says(&v), (CacheState::Unknown, Some(CacheReason::Unknown)));
        let wire = json!({"state": "warm", "warm_for_secs": 3000, "lapse_by_secs": 3000,
            "proof_ttl_secs": 3600, "requested_ttl_secs": 3600, "proof_cache_tokens": 20000});
        let warm = reply(wire.clone());
        assert_eq!(serde_json::to_value(&warm).unwrap(), wire, "the same bytes back");
        assert_eq!(
            render_cache_expiry(&warm),
            "Recent cache activity · about 50m left · 20000 tokens read or written by the last measured request"
        );
        let band = reply(json!({"state": "unknown", "reason": "may_have_expired", "lapse_by_secs": 3299,
            "proof_ttl_secs": 300, "requested_ttl_secs": 3600, "proof_cache_tokens": 5}));
        assert_eq!(render_cache_expiry(&band), "May have expired · this session's last possible refresh ends in about 55m");
        assert_eq!(render_cache_expiry(&CacheExpiry::unknown(CacheReason::Parked)), "parked");
    }

    /// Every state and reason the daemon documents, decoded and said as
    /// the recommended rendering says it.
    #[test]
    fn each_state_decodes_and_renders() {
        for (wire, state, reason, said) in [
            (
                json!({"state": "cold", "reason": "expired"}),
                CacheState::Cold,
                Some(CacheReason::Expired),
                "No unexpired evidence from this session",
            ),
            (
                json!({"state": "cold", "reason": "uncached"}),
                CacheState::Cold,
                Some(CacheReason::Uncached),
                "Last measured request reported no cache activity",
            ),
            (
                json!({"state": "unknown", "reason": "no_matching_request", "next_ping_in_secs": 200}),
                CacheState::Unknown,
                Some(CacheReason::NoMatchingRequest),
                "no matching request yet · next keep-warm attempt in about 3m",
            ),
            (
                json!({"state": "unknown", "reason": "unmeasured"}),
                CacheState::Unknown,
                Some(CacheReason::Unmeasured),
                "last request not measured yet",
            ),
            (json!({"state": "unknown", "reason": "busy"}), CacheState::Unknown, Some(CacheReason::Busy), "a turn is running"),
            (json!({"state": "unknown", "reason": "parked"}), CacheState::Unknown, Some(CacheReason::Parked), "parked"),
            (
                json!({"state": "unknown", "reason": "not_anthropic"}),
                CacheState::Unknown,
                Some(CacheReason::NotAnthropic),
                "not tracked for this provider",
            ),
            (
                json!({"state": "unknown", "reason": "head_unresolved"}),
                CacheState::Unknown,
                Some(CacheReason::HeadUnresolved),
                "couldn't check",
            ),
            (
                json!({"state": "unknown", "reason": "history_unsettled"}),
                CacheState::Unknown,
                Some(CacheReason::HistoryUnsettled),
                "last turn didn't finish",
            ),
            (
                json!({"state": "unknown", "reason": "rollover_pending"}),
                CacheState::Unknown,
                Some(CacheReason::RolloverPending),
                "next turn starts a new context",
            ),
            (json!({"state": "unknown", "reason": "probe_failed"}), CacheState::Unknown, Some(CacheReason::ProbeFailed), "couldn't check"),
            (json!({"state": "unknown"}), CacheState::Unknown, None, "couldn't check"),
            (json!({"state": "warm", "warm_for_secs": 42}), CacheState::Warm, None, "Recent cache activity · about 42s left"),
            (json!({"state": "warm"}), CacheState::Warm, None, "Recent cache activity"),
        ] {
            let c = reply(wire.clone());
            assert_eq!(says(&c), (state, reason), "{wire}");
            assert_eq!(render_cache_expiry(&c), said, "{wire}");
        }
        // `state` is the one required field.
        assert!(serde_json::from_value::<CacheExpiry>(json!({"reason": "expired"})).is_err());
        assert!(decode_query_reply::<CacheExpiry>(&json!({})).is_err_and(|e| e.code == QueryErrorCode::Internal));
    }

    /// Synaps' `an_error_is_its_own_shape_and_an_unknown_code_is_internal`.
    #[test]
    fn an_error_is_its_own_shape_and_an_unknown_code_is_internal() {
        let v = QueryError::with_message(QueryErrorCode::Busy, "full").to_value();
        assert_eq!(v, json!({"error": {"code": "busy", "message": "full"}}));
        let e = decode_query_reply::<Value>(&json!({"error": {"code": "meteor"}})).unwrap_err();
        assert_eq!(e.code, QueryErrorCode::Internal);
        let e = decode_query_reply::<Value>(&json!({"error": "old text"})).unwrap_err();
        assert_eq!((e.code, e.message.as_deref()), (QueryErrorCode::Internal, Some("old text")));
        assert_eq!(decode_query_reply::<u32>(&json!(3)).unwrap(), 3);
        for (code, want) in [
            ("busy", QueryErrorCode::Busy),
            ("timeout", QueryErrorCode::Timeout),
            ("superseded", QueryErrorCode::Superseded),
            ("stale", QueryErrorCode::Stale),
            ("unavailable", QueryErrorCode::Unavailable),
            ("internal", QueryErrorCode::Internal),
        ] {
            let e = decode_query_reply::<CacheExpiry>(&json!({"error": {"code": code}})).unwrap_err();
            assert_eq!((e.code, e.to_string()), (want, code.to_string()));
        }
        // An error is never read as a reply, even one with a `state`.
        assert!(decode_query_reply::<CacheExpiry>(&json!({"state": "warm", "error": {"code": "stale"}})).is_err());
    }
}
