# Q3 Life: main vs Q3 (bella, headless gamescope)

## Round 3 (`r3/`): the merged tree, main `9121dba`

The harness is round 2's:
- headless gamescope `-r 144`, 1920×1080, `--refresh-hz 144`;
- one run at a time, each started after the 1-minute load average fell under 6;
- `r3/run.sh` runs Q3 and main alternately: three `--bench-motion` runs each, then three `--bench` runs each.

`r3/log` records the load and bella's swap use before and after every run. Swap stayed at 8.5–9.2 GB and never grew
during a run. Every run's RSS at its end is above its idle RSS, so no run was paged out and none is skipped.

- **main:** `9121dba`. Its tree is identical to `c81ab14`, and the binary is Q2's release build of that tree
  (`synaps-desktop-hybrid.q2`, sha256 `e8ee38d9…`; provenance in `docs/img/q3/r3/README.md`).
- **Q3 r3:** `699f1b2`, release build sha256 `b4fb51d4…`. Gate 5's like-for-like 5 × 5 sample is from the head
  `f72f5ad` (`r3/q3r3g-*`), which adds only that sample.

`--bench-motion`, presents / app frames / CPU ms per s:

| case | main #1 | main #2 | main #3 | Q3 r3 #1 | Q3 r3 #2 | Q3 r3 #3 |
|---|---|---|---|---|---|---|
| `idle_10s` | 0 / 0 / 2.11 | 0 / 0 / 2.23 | 0 / 0 / 2.31 | 0 / 0 / 2.09 | 0 / 0 / 2.15 | 0 / 0 / 2.43 |
| `beat` | 48 / 0 / 15.15 | 48 / 0 / 15.33 | 48 / 0 / 13.43 | 180 / 0 / 39.12 | 180 / 0 / 39.11 | 180 / 0 / 42.8 |
| `beat_25_30s` | 240 / 0 / 14.94 | 240 / 0 / 15.81 | 240 / 0 / 14.06 | 900 / 0 / 49.73 | 900 / 0 / 46.5 | 900 / 0 / 46.81 |
| `beat_motion_off` | 0 / 0 / 2.31 | 0 / 0 / 2.13 | 0 / 0 / 2.43 | 0 / 0 / 3.08 | 0 / 0 / 2.45 | 0 / 0 / 2.8 |
| `idle_10s_after_beat` | 0 / 0 / 1.96 | 0 / 0 / 2.42 | 0 / 0 / 2.43 | 0 / 0 / 2.46 | 0 / 0 / 2.33 | 0 / 0 / 2.16 |
| `idle_30s_prompt` | 0 / 0 / 2.35 | 0 / 0 / 2.18 | 0 / 0 / 1.84 | 0 / 0 / 2.03 | 0 / 0 / 2.19 | 0 / 0 / 2.18 |
| `held_drain_10s` | 20 / 20 / 9.91 | 20 / 20 / 11.14 | 20 / 20 / 8.45 | 20 / 20 / 8.05 | 20 / 20 / 9.03 | 20 / 20 / 7.52 |
| `load_5x5_30s` | 240 / 30 / 18.94 | 240 / 30 / 18.02 | 240 / 30 / 17.12 | 240 / 30 / 18.29 | 240 / 30 / 18.93 | 240 / 30 / 19.63 |
| `load_5x5_sweep_30s` | n/a | n/a | n/a | 900 / 30 / 49.24 | 900 / 30 / 50.05 | 900 / 30 / 47.85 |
| `load_5x5_fanout_30s` | n/a | n/a | n/a | 986 / 805 / 117.78 | 975 / 804 / 121.51 | 970 / 803 / 121.58 |
| `load_5x5_fanout_tools_30s` | n/a | n/a | n/a | 968 / 808 / 121.16 | 981 / 811 / 121.9 | 980 / 809 / 117.51 |

**Gate 3 (idle = 0 presents): pass.** `idle_30s_prompt` gives 0 presents and 0 app frames in all six runs, and the JSON
shows `prompt_pending: true` and `demoted_worker: true`.

**Gate 4, no sweep: pass.** `load_5x5_30s` gives 240 presents in 30 s (8/s) at 18.3–19.6 ms/s. Main does the same scene
at 17.1–18.9.

**Gate 4, the composer sweeping over the 5 × 5: pass.** `load_5x5_sweep_30s` gives 900 presents in 30 s (30/s) at
47.9–50.1 ms/s (4.8–5.0 % of a core). App frames are 30 in 30 s, so the rest are beat-layer paints. The JSON shows
`sweeps: 1`, `state: Running` and 18 sites.

**Gate 4 with P5 rolls from a live fan-out (Shady R2-M1): FAIL, 11.8–12.2 % of a core.**
- **The scene, `load_5x5_fanout_30s`:** the sweep scene, plus the shown session's five workers each calling a tool every
  1.5 s, staggered (one change every 0.3 s, 100 in 30 s). Each change moves a worker's tool count and its step, and
  both roll.
  - 970–986 presents in 30 s (32–33/s): the 30 Hz grid, plus the changes' own frames.
  - 803–805 app frames.
  - **117.8–121.6 ms/s.**
- **The live line isn't the cost.** In `load_5x5_fanout_tools_30s` the step is held, so the live line never changes and
  only the tool count rolls. It gives 968–981 presents, 808–811 app frames and **117.5–121.9 ms/s**, the same.
  - The cost is the count rolls' app frames: about 27 a second (a roll is about 200 ms of 30 Hz app frames, and the
    rolls overlap).
  - Stopping the live line's roll would change nothing, so it is not done.
- **Round 3 already moved rolls to the 30 Hz grid** (`b1e3c17`). Before that (`de95f1c`, one run, not kept in `r3/`)
  they ran at the display's rate: **259.7 / 261.9 ms/s** (3,347 presents and 3,101 app frames in 30 s).
- **Estimate, not measured:** with no rolls at all, the same changes would cost about the sweep scene's 49 ms/s plus one
  app frame per change. That is about 3.3/s at the roughly 3 ms per app frame of `held_drain_10s`, so around 59 ms/s,
  right at the gate.

**Clocks:** exactly 30 app frames in 30 s in `load_5x5_30s` (all six runs) and in `load_5x5_sweep_30s` (Q3's three).

**Gate 5, memory** (GPU-driver mappings apart, KiB, anon / USS):
- **After 2,000 rows of history** (`--bench` `mem_end`, three runs each, alternating):
  - main: 93,892 / 126,104, 93,784 / 126,372, 93,452 / 125,840;
  - Q3: 93,260 / 125,600, 93,868 / 126,304, 93,788 / 126,240;
  - **means: anon −0.07 %, USS −0.05 %. Pass.** Streaming ran at 142.7–143.1 fps on main and 143.3–143.5 on Q3.
- **After the 5 × 5** (`--bench-motion`): main's `mem_end` is taken right after `load_5x5_30s`, where its script ends.
  Q3's `mem_end` comes after another 90 s of Q3-only cases (the sweep scene and both fan-outs), so it is not
  comparable: 64,108–65,988 anon against main's 57,396–57,496. The like-for-like sample, `mem_after_load_5x5`, taken
  at the same point (`f72f5ad`, three runs, after main's three):
  - main `mem_end`: 57,396 / 90,692, 57,496 / 89,988, 57,480 / 90,752;
  - Q3 `mem_after_load_5x5`: 59,148 / 94,032, 58,952 / 94,468, 58,572 / 94,544;
  - **means: anon +2.5 %, USS +4.3 %. Both are within +5 %: pass.** Two caveats:
    - these Q3 runs (`r3/run-mem.sh`, `r3/log-q3r3g`) ran after main's three, not alternating with them;
    - their USS at idle, before any scene, was already 4.5 MB over both main's and Q3 `699f1b2`'s (64,080–64,828
      against 59,736–60,560), with anon equal (30,096–30,240). From idle to after the 5 × 5, USS grew 29,968 on Q3
      and 30,620 on main, so the USS gap is that idle offset (private file-backed pages of this build's runs), not
      growth. Anon grew 1,452 KiB more on Q3 (+5 % of the growth, +2.5 % of the total).

## Round 2 (`r2/`): the per-row sweeps cut

Harness as below: headless gamescope `-r 144`, 1920×1080, `--refresh-hz 144`, one run at a time, each after the 1-minute
load average fell under 6. `r2/run.sh` ran main and Q3 alternately (`--bench-motion` three times each), then `--bench`
once each. Raw JSON and `log` are in `r2/`. A Q2 worker and other builds shared the machine.

- **main:** `4f87411` (Q4's release build `synaps-desktop-hybrid.q4`, sha256 `61e724e6…`, as below).
- **Q3 r2:** `f9be0a6`, release build sha256 `b0d69edb…`.

`--bench-motion`, presents / app frames / CPU ms per s:

| case | main #1 | main #2 | main #3 | Q3 r2 #1 | Q3 r2 #2 | Q3 r2 #3 |
|---|---|---|---|---|---|---|
| `idle_10s` | 0 / 0 / 4.31 | 0 / 0 / 1.82 | 0 / 0 / 2.08 | 0 / 0 / 1.86 | 0 / 0 / 2.2 | 0 / 0 / 1.74 |
| `beat` | 48 / 0 / 15.22 | 48 / 0 / 14.68 | 48 / 0 / 13.05 | 180 / 0 / 35.94 | 180 / 0 / 41.02 | 180 / 0 / 41.93 |
| `beat_25_30s` | 240 / 0 / 13.9 | 240 / 0 / 15.16 | 240 / 0 / 13.68 | 900 / 0 / 50.94 | 900 / 0 / 47.76 | 900 / 0 / 44.64 |
| `beat_motion_off` | 0 / 0 / 1.98 | 0 / 0 / 2.37 | 0 / 0 / 1.54 | 0 / 0 / 3.05 | 0 / 0 / 1.85 | 0 / 0 / 2.17 |
| `idle_10s_after_beat` | 0 / 0 / 1.99 | 0 / 0 / 1.75 | 0 / 0 / 2.29 | 0 / 0 / 2.73 | 0 / 0 / 2.02 | 0 / 0 / 1.58 |
| `idle_30s_prompt` | 0 / 0 / 2.73 | 0 / 0 / 1.78 | 0 / 0 / 2.17 | 0 / 0 / 2.2 | 0 / 0 / 2.26 | 0 / 0 / 1.7 |
| `held_drain_10s` | 20 / 20 / 8.63 | 20 / 20 / 8.13 | 20 / 20 / 11.37 | 20 / 20 / 10.57 | 20 / 20 / 12.11 | 20 / 20 / 7.46 |
| `load_5x5_30s` | 240 / 30 / 20.41 | 240 / 30 / 17.92 | 240 / 30 / 21.51 | 240 / 30 / 17.97 | 240 / 30 / 19.41 | 240 / 30 / 17.85 |
| `load_5x5_sweep_30s` | n/a | n/a | n/a | 900 / 30 / 52.42 | 900 / 30 / 50.96 | 900 / 30 / 47.97 |

New case: `load_5x5_sweep_30s` is gate 4's worst case after the cut. It is `load_5x5_30s`'s scene with gate 3's prompt
answered and the shown session's turn running, so the composer sweeps over the 25 rail and transcript spinners. The
JSON confirms it in all three runs: `sweeps: 1`, `state: "Running"`, 18 beat sites. (`load_5x5_30s` is shown with
gate 3's prompt still pending: NEEDS YOU, so no sweep, on main and Q3 alike.)

**Gate 3 (idle = 0 presents): pass.** `idle_30s_prompt`: 0 presents and 0 app frames in all three runs
(`prompt_pending: true`, `demoted_worker: true`).

**Gate 4 (≤ 8/s with no sweep on screen; ≤ 30/s with one, at ≤ 6 % of a core): pass.**
- **No sweep:** `load_5x5_30s` gives 240 presents in 30 s (8/s, the one beat) at 17.9–19.4 ms/s, against main's
  17.9–21.5 for the same scene. With the per-row sweeps cut, the 5 × 5 is back to main's cost. r1's head was
  900/30 s and 50–54 ms/s with five live lines sweeping.
- **The composer sweeping over 5 × 5:** `load_5x5_sweep_30s` gives 900 presents in 30 s (30.0/s) at 48.0–52.4 ms/s,
  which is 4.8–5.2 % of a core. `app_frames` stays at 30 in 30 s (the clocks' one per second), so the 30/s are
  beat-layer paints.
- `beat` and `beat_25_30s` run a turn, so the composer sweeps: 180/6 s and 900/30 s (30/s) at 36–51 ms/s.

**Clocks (Shady M2):** `load_5x5_30s` and `load_5x5_sweep_30s` counted exactly 30 app frames in 30 s in all six Q3 r2
runs. Shady's M2 (a clock read at the frame's late wall time skips a second when its next second falls inside the
frame's lateness) accounts for r1's single 20. The fix is in, and the 20 has not come back in those six counts.

**Gate 5, memory** (GPU-driver mappings apart, KiB, RSS / PSS):
- **After 2,000 rows of history** (`--bench` `mem_end`, back to back): main 161,096 / 136,419, Q3 r2 160,376 / 135,675.
  That is **RSS −0.4 %, PSS −0.5 %: pass.** Streaming ran at 142.9 fps on main and 143.1 on Q3.
- **After the 5 × 5 run** (`--bench-motion` `mem_end`): this round, bella was swapping (13 GB in swap). Five of the six
  runs ended with most of their pages swapped out (RSS 34–60 MB), so they can't be compared. The one intact Q3 r2 run
  (#3: 127,856 / 103,240) against round 1's back-to-back main (r3: 125,688 / 100,114, the same binary) is RSS +1.7 %,
  PSS +3.1 %, within +5 %. These are runs from two different rounds, so PSS here is only indicative; RSS is the steadier
  figure.

## Round 1

This uses the same harness as Q4's `docs/bench/q4/r4`: a headless gamescope at `-r 144`, 1920×1080,
`--refresh-hz 144`. One run at a time, each started after the 1-minute load average fell under 6. The scripts
are `run.sh` (r1: main and `c1e5f4c`), `run2.sh` (`e8f7afb` ×2), `run3.sh` (main and `e8f7afb`
back to back) and `run4.sh` (`e8f7afb` once more). The raw JSON and `log` (load at each start) are in `raw/`.
A Q2 worker was benchmarking on the same machine at times.

- **main:** `4f87411`. The binary is Q4's release build `synaps-desktop-hybrid.q4`, of a tree identical to it
  (see `docs/img/q3/README.md`).
- **Q3 `c1e5f4c`:** the build the clips were recorded with.
- **Q3 `e8f7afb` (r1's head, "Q3 #1–4"):** with a sweep on screen, the spinners ride the sweep's 30 Hz ticks instead of
  adding beats between them. A clock's second and the grid's tick within 2 ms are one wake.

### `--bench-motion`: presents / app frames / CPU ms per s

| case | main r1 | main r2 | main r3 | Q3 c1e5f4c | Q3 #1 | Q3 #2 | Q3 #3 | Q3 #4 |
|---|---|---|---|---|---|---|---|---|
| `idle_10s` | 0 / 0 / 2.05 | 0 / 0 / 1.69 | 0 / 0 / 1.72 | 0 / 0 / 1.49 | 0 / 0 / 1.69 | 0 / 0 / 1.62 | 0 / 0 / 2.05 | 0 / 0 / 1.61 |
| `beat` | 48 / 0 / 12.37 | 48 / 0 / 14.41 | 48 / 0 / 12.98 | 216 / 0 / 44.3 | 181 / 0 / 35.26 | 180 / 0 / 39.58 | 180 / 0 / 43.09 | 180 / 0 / 50.58 |
| `beat_25_30s` | 240 / 0 / 12.98 | 240 / 0 / 14.26 | 240 / 0 / 12.87 | 1078 / 0 / 50.25 | 899 / 0 / 45.6 | 900 / 0 / 46.28 | 900 / 0 / 46.4 | 900 / 0 / 48.3 |
| `beat_motion_off` | 0 / 0 / 1.76 | 0 / 0 / 1.72 | 0 / 0 / 1.88 | 0 / 0 / 2.42 | 0 / 0 / 2.14 | 0 / 0 / 1.98 | 0 / 0 / 2.11 | 0 / 0 / 1.79 |
| `idle_10s_after_beat` | 0 / 0 / 1.97 | 0 / 0 / 1.98 | 0 / 0 / 1.81 | 0 / 0 / 2.42 | 0 / 0 / 2.08 | 0 / 0 / 1.73 | 0 / 0 / 2.17 | 0 / 0 / 2.39 |
| `idle_30s_prompt` | 0 / 0 / 1.96 | 0 / 0 / 1.82 | 0 / 0 / 1.9 | 0 / 0 / 1.86 | 0 / 0 / 2.03 | 0 / 0 / 1.84 | 0 / 0 / 2.12 | 0 / 0 / 2.21 |
| `held_drain_10s` | 20 / 20 / 8.17 | 20 / 20 / 8.31 | 20 / 20 / 8.42 | 20 / 20 / 8.65 | 20 / 20 / 7.83 | 20 / 20 / 8.1 | 20 / 20 / 7.25 | 20 / 20 / 10.44 |
| `load_5x5_30s` | 240 / 30 / 15.34 | 240 / 30 / 18.46 | 240 / 30 / 17.04 | 1109 / 30 / 57.14 | 900 / 30 / 50.86 | 900 / 30 / 50.18 | 901 / 20 / 51.13 | 900 / 30 / 53.99 |
| `load_5x5_nosweep_30s` | n/a | n/a | n/a | 240 / 0 / 13.06 | 240 / 0 / 13.21 | 240 / 0 / 13.25 | 240 / 0 / 13.12 | 240 / 0 / 13.9 |

What the cases are:
- `beat`: a `bash` call runs for 6 s. On Q3 that is the snake plus the composer's sweep. (r1 claimed a bash-row
  sweep after 1 s as well; the bench's call had no timestamp, so that row never swept. The sweep is cut in r2.)
- `beat_25_30s`: 25 calls run for 30 s, and on Q3 the composer sweeps. (r1 also claimed their row sweeps; that was never
  checked, and they are cut in r2.)
- `idle_30s_prompt`: a prompt is pending (after its breaths) and a worker is demoted, for 30 s.
- `load_5x5_30s`: 5 sessions × 5 workers. The shown session's 5 live lines sweep.
- `load_5x5_nosweep_30s`: the same 5 busy sessions spin in the rail, with the shown session at rest.

**Gate 3 (idle = 0 presents): pass.** `idle_30s_prompt` gave 0 presents and 0 app frames in all five Q3 runs. The
JSON confirms the scene: `prompt_pending: true` and `demoted_worker: true`.

**Gate 4, no sweep: pass.** 240 presents in 30 s is 8 a second, the one beat. 13.1–13.9 ms/s is 1.3–1.4 % of a core,
under main's 15–18 ms/s for its 5 × 5.

**Gate 4, sweeps visible (≤ 30/s at ≤ 6 % of a core): pass at the head.**
- `e8f7afb` gave 900–901 presents in 30 s, which is 30.0 a second, at 50.2–54.0 ms/s (5.0–5.4 % of a core).
- The JSON shows `sweeps: 5`: five live lines on one timer.
- **`c1e5f4c` failed the rate:** 1109 presents, 37 a second. The 8 Hz beats fell between the 30 Hz ticks (125 ms is
  3.75 ticks), so each beat cost a paint of its own. `e8f7afb` fixes that.
- **Open:** run #3's `load_5x5_30s` counted 20 app frames where every other run, main's included, counted 30. That
  run was the most loaded (its `hover_sweep` and `cross_fade` CPU are the highest), and run #4, unloaded, counted
  30 again. I have not found the cause. If it is real, a worker clock skipped a second there.

**The sweep's cost:** `beat` goes from 48 presents in 6 s (main) to 180, and from 12–14 to 35–51 ms/s. That is P2's
own estimate, measured (polish.md: about 50 ms/s, 5 %). It is paid only while a turn runs on screen.

### Memory, gate 5 (GPU-driver mappings apart, KiB: RSS / PSS)

r3 ran main and Q3 back to back. PSS splits shared pages by how many processes map them, so it moves with whatever
else is running. Between r2 and r3 the idle PSS of the same binary moved by up to 22 MiB (whole process) with RSS
flat. So only r3's pairs are compared.

| | main (r3) | Q3 `e8f7afb` (r3) | Δ |
|---|---|---|---|
| after the 5 × 5 run (`--bench-motion` `mem_end`) | 125,688 / 100,114 | 127,564 / 99,226 | RSS +1.5 %, PSS −0.9 % |
| after 2,000 rows of history (`--bench` `mem_end`) | 161,240 / 135,394 | 160,704 / 134,820 | RSS −0.3 %, PSS −0.4 % |

**Pass: within +5 %.** Streaming ran at 142.7 fps on main and 143.3 fps on Q3 (r3).
