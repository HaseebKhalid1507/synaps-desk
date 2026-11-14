# Transcript redesign + theme roles: A/B against main 3574aa7

Headless gamescope on bella (`--backend headless`, 60 Hz), never the real display. `--bench`, one
run per launch, main and branch alternating. 1360×860: 3 runs each (`main*`, `new*`); 1920×1080 and
2560×1440: 1 run each (`wm*`, `wn*`). Load 2.1–4.5. Medians of 3, draw times in ms.

| Scenario (1360×860) | main fps | branch fps | main p50 / p95 / p99 | branch p50 / p95 / p99 | over budget |
|---|---|---|---|---|---|
| stream (200 deltas/s) | 59.8 | 59.8 | 2.28 / 3.37 / 5.29 | 2.16 / 3.65 / 6.22 | 0 / 0 |
| scroll (400-turn transcript) | 60.0 | 60.0 | 2.13 / 3.46 / 4.24 | 1.88 / 3.29 / 4.10 | 0 / 0 |
| drag rail edge (400 turns) | 59.6 | 59.6 | 4.55 / 5.98 / 6.59 | **2.03 / 2.80 / 3.58** | 0 / 0 |
| drag right edge (400 turns) | 60.0 | 60.0 | 4.61 / 6.38 / 7.16 | **2.10 / 2.69 / 4.12** | 0 / 0 |

| Drag, p50 / p99 / max | main | branch |
|---|---|---|
| 1920×1080 rail | 4.49 / 7.57 / 9.04 | 1.66 / 2.88 / 3.52 |
| 1920×1080 right | 4.23 / 10.53 / 10.80 | 1.63 / 3.12 / 3.26 |
| 2560×1440 rail | 4.63 / 7.78 / 8.05 | 2.28 / 3.17 / 3.97 |
| 2560×1440 right | 3.98 / 5.80 / 6.77 | 2.17 / 2.86 / 3.27 |

RSS (MiB, every run identical within 1): idle 163 / 163, after stream 208 / 208, 400 turns loaded
210 / 210, end 219 / 212 (branch lower: the drag scenarios no longer re-wrap the transcript).

Idle CPU, `--offline` at 1920×1080, 30 s after a 9 s settle, two runs each: main 4 and 4 ticks,
branch 5 and 5 ticks (CLK_TCK 100, so 0.13–0.17 %), RSS 178 / 178 MiB. Nothing animates.

Binary (release, stripped): main 35,281,504 B, branch 35,424,552 B (+143,048 B, +0.4 %). The
palette table itself is 20 × 280 B ≈ 5.6 KB of data; the rest is the new code.
