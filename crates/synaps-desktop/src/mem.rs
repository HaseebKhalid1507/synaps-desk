//! Process memory from `/proc/self/smaps` (Linux). GPU-driver mappings
//! (`/dev/dri`, `/dev/nvidia*`, the driver and Vulkan/GL libraries) are
//! summed separately so their share of RSS/PSS is visible.

use serde_json::{Value, json};

#[derive(Default, Clone, Copy)]
struct Acc {
    rss: u64,
    pss: u64,
    uss: u64,
    anon: u64,
}

fn is_gpu(path: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "/dev/dri", "/dev/nvidia", "libnvidia", "libGLX_nvidia", "libEGL_nvidia", "libcuda", "libvulkan",
        "nvidia_icd", "libVkLayer", "libdrm", "iris_dri", "radeonsi", "libgallium", "intel_icd", "libvulkan_",
    ];
    PATTERNS.iter().any(|p| path.contains(p))
}

/// `{rss_kb, pss_kb, uss_kb, anon_kb, gpu: {...}, other: {...}}`, or `null`.
pub fn sample() -> Value {
    #[cfg(target_os = "linux")]
    {
        let Ok(s) = std::fs::read_to_string("/proc/self/smaps") else { return Value::Null };
        let (mut total, mut gpu) = (Acc::default(), Acc::default());
        let mut cur_gpu = false;
        for line in s.lines() {
            let mut it = line.split_whitespace();
            let Some(first) = it.next() else { continue };
            if !first.ends_with(':') {
                // A mapping header: "addr perms offset dev inode [path]".
                let path = line.split_whitespace().nth(5).unwrap_or("");
                cur_gpu = is_gpu(path);
                continue;
            }
            let kb: u64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            let slot = |a: &mut Acc| match first {
                "Rss:" => a.rss += kb,
                "Pss:" => a.pss += kb,
                "Private_Clean:" | "Private_Dirty:" => a.uss += kb,
                "Anonymous:" => a.anon += kb,
                _ => {}
            };
            slot(&mut total);
            if cur_gpu {
                slot(&mut gpu);
            }
        }
        let j = |a: Acc| json!({"rss_kb": a.rss, "pss_kb": a.pss, "uss_kb": a.uss, "anon_kb": a.anon});
        let other = Acc { rss: total.rss - gpu.rss, pss: total.pss - gpu.pss, uss: total.uss - gpu.uss, anon: total.anon - gpu.anon };
        let mut v = j(total);
        v["gpu_driver_mappings"] = j(gpu);
        v["excluding_gpu_driver"] = j(other);
        // smaps_rollup as the cross-check the brief asks for.
        if let Ok(r) = std::fs::read_to_string("/proc/self/smaps_rollup") {
            let get = |k: &str| {
                r.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<u64>().ok())
            };
            v["rollup"] = json!({"rss_kb": get("Rss:"), "pss_kb": get("Pss:"),
                "uss_kb": get("Private_Clean:").zip(get("Private_Dirty:")).map(|(a, b)| a + b)});
        }
        v
    }
    #[cfg(not(target_os = "linux"))]
    {
        Value::Null
    }
}

/// Seconds between the kernel starting this process and now (10 ms
/// resolution: clock ticks), Linux only.
pub fn process_age_ms() -> Option<f64> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
        let after = &stat[stat.rfind(')')? + 2..];
        // Field 22 overall = index 19 after the ")" (state is field 3).
        let start_ticks: f64 = after.split_whitespace().nth(19)?.parse().ok()?;
        let uptime: f64 = std::fs::read_to_string("/proc/uptime").ok()?.split_whitespace().next()?.parse().ok()?;
        Some((uptime - start_ticks / 100.0) * 1000.0)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}
