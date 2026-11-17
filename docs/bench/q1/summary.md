`--bench-motion`, SYNAPS_BENCH_IDLE_SECS=30 (rows at rest over a finished history), 144 Hz headless gamescope, 1920×1080:

| | main r1 | main r2 | Q1 r1 | Q1 r2 |
|---|---|---|---|---|
| idle 30 s: presents / app frames | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |
| idle CPU ms/s | 2.75 | 2.65 | 2.38 | 2.59 |
| a tool runs, 6 s: presents / CPU ms/s | 48 / 15.68 | 48 / 13.79 | 48 / 13.13 | 48 / 16.21 |
| 25 glyphs scanning (5 × 5): presents / CPU ms/s | 48 / 15.48 | 48 / 16.06 | 48 / 14.46 | 48 / 16.9 |
| Motion Off, 4 s: presents | 0 | 0 | 0 | 0 |
| end RSS / PSS MiB (GPU mappings apart) | 119.8 / 88.9 | 120.9 / 89.9 | 119.6 / 88.8 | 120.6 / 89.7 |

`--bench` (stream, 2,000-row history, scroll, spins, drags):

| | main r1 | main r2 | Q1 r1 | Q1 r2 |
|---|---|---|---|---|
| stream: draw p95 ms · dropped | 2.39 · 5 | 2.53 · 6 | 2.56 · 6 | 2.47 · 5 |
| scroll 2,000 rows: draw p95 ms · dropped | 2.23 · 0 | 2.17 · 0 | 2.4 · 0 | 2.2 · 0 |
| idle: RSS / PSS MiB | 91.7 / 60.9 | 91.8 / 60.9 | 92.3 / 61.5 | 92.3 / 61.4 |
| after stream: RSS / PSS MiB | 143.1 / 112.1 | 143.2 / 112.2 | 143.3 / 112.4 | 143.5 / 112.6 |
| 2,000 rows loaded: RSS / PSS MiB | 145.5 / 114.6 | 145.6 / 114.6 | 146.5 / 115.6 | 146.8 / 115.8 |
| after scroll: RSS / PSS MiB | 153.7 / 122.8 | 153.8 / 122.8 | 154.4 / 123.5 | 154.8 / 123.8 |
| end: RSS / PSS MiB | 155.9 / 125.0 | 155.9 / 125.0 | 157.0 / 126.1 | 156.9 / 125.9 |
