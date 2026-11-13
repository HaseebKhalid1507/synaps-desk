//! Finding a daemon: the run dir and its `daemon[-<profile>].json` registry.
//!
//! Mirrors Synaps' `agent_core::core::config::base_dir`,
//! `events::registry::{registry_dir, daemon_paths_in}` and
//! `daemon::registry::DaemonInfo` (Apache-2.0).

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// `$HOME`, else `%USERPROFILE%`: Synaps' order. On Windows a GUI started
/// from Explorer usually has no `HOME` while a daemon auto-started from a
/// shell (Git Bash, MSYS) may have one, so the two can look in different
/// run dirs; pass an explicit socket or `SYNAPS_BASE_DIR` if they disagree.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}

/// `$SYNAPS_BASE_DIR`, else `$HOME/.synaps-cli` (`%USERPROFILE%` on Windows).
pub fn base_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("SYNAPS_BASE_DIR") {
        return PathBuf::from(p);
    }
    home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".synaps-cli")
}

/// `$SYNAPS_RUNTIME_DIR`, else `<base>/run`.
pub fn run_dir() -> PathBuf {
    std::env::var_os("SYNAPS_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| base_dir().join("run"))
}

/// Same sanitising as Synaps' `sanitize_session_id`.
fn sanitize(raw: &str) -> String {
    raw.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

/// The registry stem for a profile: `daemon` or `daemon-<profile>`.
pub fn stem(profile: Option<&str>) -> String {
    match profile.map(sanitize).filter(|p| !p.is_empty()) {
        Some(p) => format!("daemon-{p}"),
        None => "daemon".to_string(),
    }
}

/// `daemon.json`: pid + profile + socket path. Never credentials.
#[derive(Debug, Clone, Deserialize)]
pub struct DaemonInfo {
    pub pid: u32,
    pub protocol_version: u32,
    #[serde(default)]
    pub daemon_version: String,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub started_at: String,
    pub socket: String,
    #[serde(default = "one")]
    pub generation: u64,
}

fn one() -> u64 {
    1
}

impl DaemonInfo {
    pub fn socket_path(&self) -> PathBuf {
        PathBuf::from(&self.socket)
    }
}

pub fn read_info(path: &Path) -> Option<DaemonInfo> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// The registry entry for `profile` under `dir`, if one is there.
pub fn find_in(dir: &Path, profile: Option<&str>) -> Option<DaemonInfo> {
    read_info(&dir.join(format!("{}.json", stem(profile))))
}

/// The registry entry for `profile` in the default run dir. `None` is the
/// default daemon: pass `std::env::var("SYNAPS_PROFILE")` yourself, or use
/// [`crate::Endpoint::from_env`], to honour `SYNAPS_PROFILE` as the TUI does.
pub fn find(profile: Option<&str>) -> Option<DaemonInfo> {
    find_in(&run_dir(), profile)
}

/// Every `daemon*.json` in `dir` (a liveness check is the connect itself).
pub fn list_in(dir: &Path) -> Vec<DaemonInfo> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<DaemonInfo> = rd
        .flatten()
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n.starts_with("daemon") && n.ends_with(".json") && !n.contains(".tmp")
        })
        .filter_map(|e| read_info(&e.path()))
        .collect();
    out.sort_by(|a, b| a.profile.cmp(&b.profile));
    out
}

/// Where to connect: an explicit socket, else the profile's registry entry,
/// else the conventional `<run>/<stem>.sock` path.
pub fn resolve_socket(socket: Option<&Path>, profile: Option<&str>) -> PathBuf {
    if let Some(s) = socket {
        return s.to_path_buf();
    }
    if let Some(info) = find(profile) {
        return info.socket_path();
    }
    run_dir().join(format!("{}.sock", stem(profile)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems_are_profile_scoped_and_sanitised() {
        assert_eq!(stem(None), "daemon");
        assert_eq!(stem(Some("")), "daemon");
        assert_eq!(stem(Some("desktopproto")), "daemon-desktopproto");
        assert_eq!(stem(Some("work/../evil")), "daemon-work____evil");
    }

    #[test]
    fn reads_a_registry_entry() {
        let dir = std::env::temp_dir().join(format!("synaps-client-disc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("daemon-x.json"),
            r#"{"pid":7,"protocol_version":3,"daemon_version":"0.11.1","profile":"x","started_at":"t","socket":"/r/daemon-x.sock","exe":"/usr/bin/synaps","generation":2,"future":1}"#,
        )
        .unwrap();
        let info = find_in(&dir, Some("x")).unwrap();
        assert_eq!(info.pid, 7);
        assert_eq!(info.generation, 2);
        assert_eq!(info.socket_path(), PathBuf::from("/r/daemon-x.sock"));
        assert_eq!(list_in(&dir).len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }
}
