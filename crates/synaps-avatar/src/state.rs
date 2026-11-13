//! Persisted placement: the avatar's ground point (feet) in root pixels, so a changed
//! `--height` still stands on the same spot.

use std::path::PathBuf;

fn path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("synaps-avatar/state.json")
}

pub fn load() -> Option<(i32, i32)> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path()).ok()?).ok()?;
    let g = v.get("ground")?.as_array()?;
    Some((g.first()?.as_i64()? as i32, g.get(1)?.as_i64()? as i32))
}

pub fn save(ground: (i32, i32)) {
    let p = path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let tmp = p.with_extension("json.tmp");
    let body = serde_json::json!({ "ground": [ground.0, ground.1] }).to_string();
    if std::fs::write(&tmp, body).is_ok() {
        std::fs::rename(&tmp, &p).ok();
    }
}
