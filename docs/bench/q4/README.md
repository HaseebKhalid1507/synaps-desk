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
| 25 glyphs scanning (not 5 × 5: the beat's own sites; see r2 for the real 5 × 5): presents / CPU ms/s | 48 / 13.5 | 48 / 13.7 | 48 / 13.3 | 48 / 13.2 |
| Motion Off, 4 s: presents | 0 | 0 | 0 | 0 |

48 presents in 6 s is the 8 Hz beat (≤ 8/s); 13 ms/s is 1.3 % of a core.

## r2 (the review fixes): gates 3, 4 and 5 run as written

Same harness: a headless gamescope at 144 Hz, 1920×1080, one run at a time, each after the load
average fell under 6. Raw: `r2/*.json`. main isn't re-run. Its numbers are r1's above, from the same
harness and machine.

`--bench` (two runs):

| | Q4 r2 run 1 | Q4 r2 run 2 | main (r1, 3 runs) |
|---|---|---|---|
| streaming fps (dropped) | 142.9 (6) | 142.9 (6) | 142.3–142.9 |
| streaming draw p50 / p99 ms | 2.03 / 3.32 | 2.01 / 4.42 | 2.65–2.70 / 4.85–5.05 |
| scroll draw p50 / p99 ms | 1.35 / 3.30 | 1.62 / 3.00 | 2.16–2.18 / 3.90–5.72 |
| wheel spin fps, draw p99 ms | 143.9, 3.70 | 143.6, 3.46 | 143.7–144.0, 3.71–3.94 |
| rail edge drag draw p99 ms | 1.86 | 2.97 | 3.36–3.38 |
| RSS idle / end (MiB) | 169.2 / 233.6 | 169.2 / 233.5 | 168.7–169.1 / 232.7–234.0 |
| PSS idle / end (MiB) | 78.3 / 148.9 | 78.3 / 148.8 | 83.3–83.7 / 147.2–148.5 |
| end, GPU-driver mappings apart: RSS / PSS (MiB) | 156.3 / 125.3 | 156.2 / 125.3 | 155.6–156.9 / 124.1–125.3 |

**Memory (gate 5):** at the end (2,000 rows of history loaded), Q4 r2 is +0.2 % RSS and +0.8 % PSS
over main's mean (+5 % allowed).

`--bench-motion` (two runs). The r1 scenes come first, as main runs them. Then Q4's own cases, on a
fresh transcript at Full:

| case | what | run 1: presents / app frames / CPU ms/s | run 2 |
|---|---|---|---|
| `idle_10s` | nothing running, 10 s | 0 / 0 / 2.51 | 0 / 0 / 2.17 |
| `beat` | a tool runs, 6 s (the 8 Hz beat) | 48 / 0 / 14.15 | 48 / 0 / 14.3 |
| `beat_25_30s` | 25 glyphs scanning (the beat's sites), 30 s | 240 / 0 / 15.51 | 240 / 0 / 15.11 |
| `beat_motion_off` | Motion Off, a tool running, 4 s | 0 / 0 / 2.55 | 0 / 0 / 1.78 |
| `idle_10s_after_beat` | nothing running again, 10 s | 0 / 0 / 2.6 | 0 / 0 / 2.78 |
| `idle_30s_prompt` | **gate 3**: a prompt pending + a demoted worker, 30 s | 0 / 0 / 2.4 | 0 / 0 / 2.14 |
| `held_drain_10s` | the link down, drained and held, 10 s | 20 / 20 / 9.22 | 20 / 20 / 10.02 |
| `hover_sweep` | the pointer down a 12-row rail, 2.4 s | 234 / 234 / 192.9 | 235 / 235 / 185.49 |
| `cross_fade` | a dark → dark theme change, 1 s | 54 / 54 / 105.15 | 54 / 54 / 106.47 |
| `load_5x5_30s` | **gate 4**: 5 sessions × 5 workers, 30 s | 240 / 30 / 18.91 | 240 / 30 / 18.99 |

- **Gate 3 (idle = 0):** `idle_30s_prompt` is 30 s with a confirm prompt pending (after its three
  breaths) and a demoted worker (a re-attach's "started": `demoted_worker: true` in the JSON). It
  gives 0 presents and 0 app frames.
- **Gate 4 (under load):** `load_5x5_30s` is five busy sessions in the rail, each with
  `subagents_running: 5`. The shown session's five workers run in its tray and transcript
  (`workers_running: 5`). That is 240 presents in 30 s, 8/s: the one 8 Hz beat, no more. At about
  19 ms/s (18.9–19.0) it uses 1.9 % of a core (≤ 6 % allowed). Q3's sweep isn't in this branch, so there's no
  "one sweep" row.
- `held_drain_10s` is the link down past its grace and drain, holding. Only the connection word's
  dots step (2 Hz, on the beat).
- `hover_sweep` is the pointer down a 12-row rail at rest (13 items with the head), one move every 16 ms for 2 s.
- `cross_fade` is a dark → dark theme change (`slow`, 360 ms, then settled).
- `load_5x5_30s` shows 30 app frames in 30 s: the five workers' tray clocks tick once a second each
  on the same second.
- The beat's CPU here (14–15 ms/s) is a little above r1's (13 ms/s), with the presents the same (48 in
  6 s, 240 in 30 s). r2's binary didn't change the beat. The machine's load was higher during these
  runs (3–5 against under 2).
- `hover_sweep` and `cross_fade` run at display rate while they move: about 190 and 105 ms/s for
  those 1–2 s. Both stop with the motion, and the following case starts at 0 presents.

## r4: on the merged tree (main `e2db53f` + Q4, rebased), gates 3, 4 and 5 against main `e2db53f`

Same harness as r2: headless gamescope at 144 Hz, 1920×1080, one run at a time, alternating main → Q4.
Each run starts after the 1-minute load average falls under 6 (it was 1.4–5.8). Script: `r4/run.sh`. Raw: `r4/*.json`.
- **main:** `e2db53f`, built from a detached worktree.
- **Q4:** this branch rebased on it, at `fe27428`.
- **`--bench-motion`:** main's is Q1's (idle, beat, 25 glyphs, Off: 31 s). Q4's adds its own cases (151 s).

`--bench-motion`, Q4 (two runs; presents / app frames / CPU ms/s):

| case | what | run 1 | run 2 |
|---|---|---|---|
| `idle_10s` | nothing running, 10 s | 0 / 0 / 2.73 | 0 / 0 / 1.36 |
| `beat` | a tool runs, 6 s (the 8 Hz beat) | 48 / 0 / 12.31 | 48 / 0 / 11.71 |
| `beat_25_30s` | 25 glyphs scanning, 30 s | 240 / 0 / 12.6 | 240 / 0 / 12.19 |
| `beat_motion_off` | Motion Off, a tool running, 4 s | 0 / 0 / 1.38 | 0 / 0 / 2.19 |
| `idle_10s_after_beat` | nothing running again, 10 s | 0 / 0 / 1.79 | 0 / 0 / 1.74 |
| `idle_30s_prompt` | **gate 3**: a prompt pending + a demoted worker, 30 s | **0 / 0** / 1.6 | **0 / 0** / 1.51 |
| `held_drain_10s` | the link down, drained and held | 20 / 20 / 6.07 | 20 / 20 / 6.77 |
| `hover_sweep` | the pointer down a 12-row rail, 2.4 s | 234 / 234 / 219.9 | 232 / 232 / 229.99 |
| `cross_fade` | a dark → dark theme change, 1 s | 54 / 54 / 115.51 | 54 / 54 / 115.71 |
| `load_5x5_30s` | **gate 4**: 5 sessions × 5 workers, 30 s | **240** / 30 / **14.73** | **240** / 30 / **14.31** |

main's `--bench-motion` (Q1's cases), for comparison: idle 10 s 0 / 0 (1.87, 1.76 ms/s); beat 48 presents (10.7,
11.84 ms/s); 25 glyphs 48 presents in 6 s (11.65, 11.39 ms/s); Off 0.

- **Gate 3 (idle = 0 presents): pass.** `idle_30s_prompt` gives 0 presents and 0 app frames over 30 s, in both runs. The JSON confirms the scene: `prompt_pending: true` (a confirm prompt, after its three breaths) and `demoted_worker: true` (a re-attach's "started").
- **Gate 4 (under load): pass.**
  - **The scene:** `load_5x5_30s` is five busy sessions in the rail, each with `subagents_running: 5`. The shown session's five workers run in its tray and transcript (`workers_running: 5`, 10 beat sites). A client only sees the other sessions' workers as their rail rows and counts, so this is the whole 5 × 5 as the desktop draws it.
  - **Presents:** 240 in 30 s, which is 8.0/s (≤ 8 allowed). That is the single 8 Hz beat.
  - **CPU:** 14.3–14.7 ms/s, 1.4–1.5 % of a core (≤ 6 % allowed).
  - **App frames:** 30, the tray's clocks ticking once a second.
  - **Not covered:** the "one sweep" row is Q3's.
- **Gate 5 (memory): pass.** `--bench` `mem_end` is taken after 2,000 rows of history, a scroll and two panel drags:

| | main r1 | main r2 | Q4 r1 | Q4 r2 | Q4 vs main (means) |
|---|---|---|---|---|---|
| RSS idle / end (MiB) | 169.6 / 234.1 | 169.4 / 234.4 | 169.5 / 234.5 | 169.2 / 234.3 | −0.1 % / **+0.09 %** |
| PSS idle / end (MiB) | 92.0 / 156.3 | 91.8 / 156.7 | 91.8 / 156.7 | 91.5 / 156.5 | −0.3 % / **+0.08 %** |
| end, GPU-driver mappings apart: RSS / PSS (MiB) | 156.8 / 126.5 | 157.1 / 126.8 | 157.3 / 126.9 | 157.0 / 126.7 | +0.12 % / +0.08 % |

  - **After the 5 × 5 run:** main's motion bench has no 5 × 5 case, so there's no like-for-like figure. Q4's RSS at the end of its 151 s motion bench (after the 5 × 5) is 199.0 / 200.6 MiB. main's, after its 31 s bench, is 196.8 / 197.0 MiB: +1.5 %, and Q4 ran five more scenes.
- **Rendering under `--bench`** (two runs each):
  - **Streaming:** main 142.9 fps (7 and 5 frames dropped), draw p99 3.64 / 4.06 ms. Q4 142.9 / 143.1 fps (5 and 6 dropped), p99 4.64 / 4.40 ms.
  - **Scroll:** draw p99 main 3.40 / 3.03 ms, Q4 3.32 / 3.00 ms.
