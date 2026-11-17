# Q4 chrome motion: main vs Q4 (bella, headless gamescope)

bella had no graphical session during these runs, so `--bench` / `--bench-motion` ran in a headless
gamescope (`-r 144`, 1920×1080), one at a time, alternating main → Q4, each after the 1-minute load
average fell under 6 (other agents' builds share the machine). `--refresh-hz 144`. Raw: `*.json`.

- **main** = `c54ebd8`, release; **Q4** = this branch, release.

| | main r1 | main r2 | main r3 | Q4 r1 | Q4 r2 | Q4 r3 |
|---|---|---|---|---|---|---|
| streaming fps (dropped) | 142.9 (5) | 125.7 (92)¹ | 142.3 (8) | 142.5 (6) | 142.3 (10) | 142.6 (6) |
| streaming draw p50 / p99 ms | 2.65 / 4.85 | 3.17 / 12.87¹ | 2.70 / 5.05 | 2.58 / 5.38 | 2.70 / 5.77 | 2.72 / 5.11 |
| scroll draw p50 / p99 ms | 2.18 / 5.72 | 2.25 / 4.38 | 2.16 / 3.90 | 2.22 / 3.99 | 2.26 / 3.87 | 2.16 / 3.91 |
| wheel spin fps, draw p99 ms | 143.9, 3.82 | 143.7, 3.94 | 144.0, 3.71 | 109.7, 10.86¹ | 143.7, 3.75 | 143.9, 3.90 |
| rail edge drag draw p99 ms | 3.38 | 3.59 | 3.36 | 11.45¹ | 3.49 | 3.46 |
| RSS idle / end (MiB) | 168.9 / 234.0 | 169.1 / 233.0 | 168.7 / 232.7 | 169.1 / 233.2 | 169.0 / 233.6 | 169.1 / 233.7 |
| PSS idle / end (MiB) | 83.5 / 148.5 | 83.7 / 147.5 | 83.3 / 147.2 | 83.7 / 147.7 | 83.6 / 148.0 | 83.6 / 148.0 |
| end, GPU-driver mappings apart: RSS / PSS (MiB) | 156.9 / 125.3 | 155.8 / 124.4 | 155.6 / 124.1 | 156.1 / 124.5 | 156.5 / 124.8 | 156.5 / 124.9 |

¹ One-off load spikes, one on each side (main r2's stream, Q4 r1's spin and drag); the other runs agree.

**Memory:** Q4's end RSS and PSS are within 0.5 % of main's (+5 % allowed) (the 2,000-row history is loaded by then).

`--bench-motion` (two runs each):

| | main r1 | main r2 | Q4 r1 | Q4 r2 |
|---|---|---|---|---|
| idle 10 s: presents / app frames | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |
| idle CPU ms/s | 2.23 | 2.35 | 2.13 | 2.20 |
| a tool runs, 6 s: presents / app frames / CPU ms/s | 48 / 0 / 12.6 | 48 / 0 / 13.0 | 48 / 0 / 13.2 | 48 / 0 / 12.8 |
| 25 glyphs scanning (≈ 5 × 5): presents / CPU ms/s | 48 / 13.5 | 48 / 13.7 | 48 / 13.3 | 48 / 13.2 |
| Motion Off, 4 s: presents | 0 | 0 | 0 | 0 |

48 presents in 6 s is the 8 Hz beat (≤ 8/s); 13 ms/s is 1.3 % of a core.
