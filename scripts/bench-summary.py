#!/usr/bin/env python3
"""Summarise synaps-desktop --bench/--diag JSON files (one line per scenario)."""
import json, sys
for p in sys.argv[1:]:
    d = json.load(open(p))
    print(f"== {p}")
    for k in ["main_to_first_frame_ms", "launch_to_first_frame_ms", "exec_to_main_ms", "startup_marks_ms", "total_s", "daemon_turn"]:
        if k in d:
            print(f"  {k}: {d[k]}")
    for k, s in d.items():
        if isinstance(s, dict) and "fps" in s:
            g = lambda n, f: (s.get(n) or {}).get(f)
            print(f"  {k:32} fps {s['fps']:6} dropped {s.get('dropped_frames')} draw p50/95/99 {g('draw_ms','p50')}/{g('draw_ms','p95')}/{g('draw_ms','p99')}"
                  f" interval p50/95/99 {g('frame_interval_ms','p50')}/{g('frame_interval_ms','p95')}/{g('frame_interval_ms','p99')}"
                  f" phases50 {s.get('draw_phases_p50_ms')} submit50 {g('present_submit_ms','p50')}")
    for k, m in d.items():
        if k.startswith("mem_") and isinstance(m, dict):
            print(f"  {k:28} rss {m['rss_kb']//1024} MiB  pss {m['pss_kb']//1024}  uss {m['uss_kb']//1024}  gpu-maps rss {m['gpu_driver_mappings']['rss_kb']//1024} pss {m['gpu_driver_mappings']['pss_kb']//1024}  non-gpu uss {m['excluding_gpu_driver']['uss_kb']//1024}")
