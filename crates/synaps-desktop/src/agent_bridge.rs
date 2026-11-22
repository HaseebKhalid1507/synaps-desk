//! Native semantic bridge contract. Not a network endpoint or permission grant.
//! The host owns the action allowlist; every invocation needs explicit approval.
use std::collections::BTreeSet;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Scope {
    pub session: String,
    pub surface: String,
    pub revision: u64,
}

#[derive(Debug, Serialize)]
pub(crate) struct Snapshot {
    pub scope: Scope,
    pub actions: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Invocation {
    pub scope: Scope,
    pub action: String,
    pub arguments: Value,
}

pub(crate) struct Bridge {
    scope: Scope,
    actions: BTreeSet<String>,
}

impl Bridge {
    /// Host-created capabilities only. Never populate this from agent markup.
    pub fn new(scope: Scope, actions: impl IntoIterator<Item = String>) -> Self {
        Self { scope, actions: actions.into_iter().collect() }
    }

    /// No generic data-model dump: only host-approved capability names.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot { scope: self.scope.clone(), actions: self.actions.iter().cloned().collect() }
    }

    /// Returns a validated request to a host-owned handler, never executes tools.
    /// Consumes the revision before dispatch: ambiguous failures cannot replay.
    pub fn authorize(
        &mut self,
        bytes: &[u8],
        allow: impl FnOnce(&Invocation) -> bool,
    ) -> Result<Invocation, &'static str> {
        if bytes.len() > 8192 {
            return Err("invocation too large");
        }
        let request: Invocation = serde_json::from_slice(bytes).map_err(|_| "invalid invocation")?;
        if request.scope != self.scope {
            return Err("wrong session, surface, or stale revision");
        }
        if !self.actions.contains(&request.action) {
            return Err("action not exposed by host");
        }
        if !allow(&request) {
            return Err("host denied action or arguments");
        }
        self.scope.revision = self.scope.revision.checked_add(1).ok_or("revision exhausted")?;
        Ok(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bridge() -> Bridge {
        Bridge::new(Scope { session: "local-preview".into(), surface: "plan".into(), revision: 0 }, ["submit_feedback".into()])
    }
    fn request(session: &str, surface: &str, revision: u64, action: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "scope": {"session":session,"surface":surface,"revision":revision},
            "action":action,"arguments":{"selection":"tests"}
        })).unwrap()
    }
    #[test]
    fn action_requires_host_permission_and_fresh_scope() {
        let mut bridge = bridge();
        let good = request("local-preview", "plan", 0, "submit_feedback");
        assert!(bridge.authorize(&good, |_| false).is_err());
        assert_eq!(bridge.snapshot().scope.revision, 0);
        assert!(bridge.authorize(&good, |r| r.arguments["selection"] == "tests").is_ok());
        assert!(bridge.authorize(&good, |_| true).is_err());
    }
    #[test]
    fn rejects_cross_session_cross_surface_and_permission_actions() {
        let mut bridge = bridge();
        for bytes in [
            request("other", "plan", 0, "submit_feedback"),
            request("local-preview", "other", 0, "submit_feedback"),
            request("local-preview", "plan", 0, "approve_tool"),
            vec![b' '; 8193],
        ] {
            assert!(bridge.authorize(&bytes, |_| panic!("must reject before authorization")).is_err());
        }
    }
}

/// A local bridge exercise, deliberately with no executable capabilities.
/// Exercises snapshots/rejections without pretending an agent transport exists.
pub(crate) fn check(path: &str) -> Result<(), String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path).map_err(|e| e.to_string())?
        .take(8193).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let mut bridge = Bridge::new(
        Scope { session: "local-preview".into(), surface: "plan".into(), revision: 0 },
        ["submit_feedback".into()],
    );
    let snapshot = bridge.snapshot();
    let accepted = bridge.authorize(&bytes, |r| {
        r.arguments.as_object().is_some_and(|a| a.len() == 1)
            && r.arguments["selection"].as_str().is_some_and(|s| matches!(s, "implementation" | "tests" | "review"))
    }).map_err(str::to_owned)?;
    println!("{}", serde_json::json!({
        "before": snapshot,
        "accepted": {"action":accepted.action, "arguments":accepted.arguments},
        "after": bridge.snapshot(),
        "executed": false
    }));
    Ok(())
}
