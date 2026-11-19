# Q5 panels: bench

> **Media:** the screenshots and clips named here are in the private repo [synaps-desk-evidence](https://github.com/HaseebKhalid1507/synaps-desk-evidence/tree/main/docs/img), at the same paths. See [EVIDENCE.md](../../EVIDENCE.md).

bella, headless gamescope at 144 Hz, `--refresh-hz 144`, settings `{"text_size": 16, "theme": "nord"}`. Release builds
made the same way (`dbuild … build --release --locked`):
- **main** = `9b0d263`, sha256 `91e0c957…0c727`; for `--bench-panels` (Q5's new scene), **main-bench** = `9b0d263` with
  only the bench commit (`5a181f3`'s `bench.rs` + `main.rs`) cherry-picked, sha256 `7dd0415c…1b62f`;
- **Q5** = this branch at `adcce6d`, sha256 `33ea4d6e…738b7bd` (`bella:~/.local/bin/synaps-desktop-hybrid.q5`; the
  release build of the PR's head is byte-identical).

`bench.sh` runs main / Q5 alternating, three runs each, each waiting for a load under 6; every run's config is a
scratch folder under `~/.cache/q5` (an absolute `XDG_CONFIG_HOME`). bella's real `~/.config/synaps-desktop/settings.json`
kept its mtime (`1794981241`, 00:54:01) through every run. `log` has each run's load and the system's swap-ins during
it (`pswpin`). Raw: `{main,q5}-panels-{size}-{1,2,3}.json`, `{main,q5}-motion-{1,2,3}.json`; tables: `table.py`.

The clips that go with these numbers are not here: the lossless clips (and `clips.txt` manifests) live off the
repo, in the maintainer's `synaps-desktop/q5-evidence/` folder, at the same paths as under `docs/img/q5/` (see
`docs/img/q5/README.md`; the `--bench-panels` scene filmed is `r2/r2-bench-panels.mp4`, previewed in the repo as
`docs/img/q5/r2/r2-bench-panels-preview.mp4`). The raw JSON here is complete.

## `--bench-panels`: a fold, an unfold, the composer growing (the brief's scene)

A 400-turn synthetic transcript (1,061 rows) at its end, following. Six actions in turn: Ctrl+B (rail folds), Ctrl+B
(unfolds), the right panel the same, the composer's draft set to two lines, then cleared. For each, the 0.8 s from
the action (presents, the app's draw time per frame, frames over the 144 Hz budget, dropped refreshes, the process's
CPU) and the 1 s after it (presents and app frames: what keeps going).

**1280×720** (means of 3; draw = the app's draw time per presented frame, ms)

| action | main presents | Q5 presents | main draw p95 / max | Q5 draw p95 / max | over budget, dropped (3 runs) main / Q5 | main CPU ms | Q5 CPU ms | after (presents, app frames) main / Q5 |
|---|---|---|---|---|---|---|---|---|
| rail_fold | 1 | 87 | 25.14 / 25.14 | 1.92 / 20.77 | 3, 0 / 3, 9 | 28.23 | 196.87 | 0, 0 / 0, 0 |
| rail_unfold | 2 | 71.67 | 4.09 / 4.09 | 1.71 / 5.5 | 0, 0 / 0, 0 | 9.7 | 145.25 | 0, 0 / 0, 0 |
| right_fold | 1 | 90 | 3.57 / 3.57 | 1.84 / 2.83 | 0, 0 / 0, 0 | 6.84 | 165.98 | 0, 0 / 0, 0 |
| right_unfold | 2 | 72 | 5.14 / 5.14 | 1.92 / 4.5 | 0, 0 / 0, 0 | 10.45 | 147.8 | 0, 0 / 0, 0 |
| composer_grow | 1 | 27 | 2.79 / 2.79 | 1.98 / 4.45 | 0, 0 / 0, 0 | 6.74 | 51.69 | 0, 0 / 0, 0 |
| composer_shrink | 1 | 27 | 2.74 / 2.74 | 2.27 / 3.17 | 0, 0 / 0, 0 | 5.86 | 47.44 | 0, 0 / 0, 0 |

**1920×1080** (means of 3; draw = the app's draw time per presented frame, ms)

| action | main presents | Q5 presents | main draw p95 / max | Q5 draw p95 / max | over budget, dropped (3 runs) main / Q5 | main CPU ms | Q5 CPU ms | after (presents, app frames) main / Q5 |
|---|---|---|---|---|---|---|---|---|
| rail_fold | 1 | 90 | 2.5 / 2.5 | 3.2 / 4.34 | 0, 0 / 0, 1 | 5.44 | 226.06 | 0, 0 / 0, 0 |
| rail_unfold | 1 | 72 | 2.49 / 2.49 | 2.34 / 4.51 | 0, 0 / 0, 0 | 5.56 | 178.04 | 0, 0 / 0, 0 |
| right_fold | 1 | 90 | 2.39 / 2.39 | 2.23 / 3.73 | 0, 0 / 0, 0 | 5.2 | 208.29 | 0, 0 / 0, 0 |
| right_unfold | 1 | 72 | 2.79 / 2.79 | 2.11 / 3.4 | 0, 0 / 0, 0 | 5.58 | 168.44 | 0, 0 / 0, 0 |
| composer_grow | 1 | 27 | 3.11 / 3.11 | 1.81 / 3.64 | 0, 0 / 0, 0 | 6.32 | 57.87 | 0, 0 / 0, 0 |
| composer_shrink | 1 | 27 | 2.98 / 2.98 | 2.59 / 3.08 | 0, 0 / 0, 0 | 6.19 | 60.25 | 0, 0 / 0, 0 |

- **Back to 0 after every action:** every Q5 run had 0 presents and 0 app frames in the second after each action
  (main the same).
- **Frames in flight:** a fold is ~87–90 presents (the spring on `sheet`, then the strip's 70 → 100 % lift over
  `quick`), an unfold 72, the composer 27 (`flick`).
- **The wrap freeze keeps a fold's layout cost flat.** Q5's draw p95 in flight is 1.7–3.2 ms (main draws each
  action in one or two frames of 1.9–5.1 ms, the 1280 rail fold's 25 ms apart). The one exception is by design: **at
  1280×720 the rail folding widens the pane past a wrap width, so the column rewraps once, in the frame the fold
  lands** (`column_rewraps_at_most_once_per_panel_motion` pins it there). That frame is Q5's max, 20.8 ms; it is
  main's only frame of the fold (25.1 ms), the same rewrap of 1,061 rows. In Q5 it is one frame over budget per fold,
  3 refreshes dropped each time. At 1920 nothing rewraps: no frame over budget, one dropped refresh in three runs.
- **CPU:** Q5 spends 145–226 ms of CPU over a panel's 0.8 s (about 2.2 ms per presented frame); main 5–28 ms (one
  or two frames). The composer's growth or shrink: 47–60 ms (main 6–7 ms).

## `--bench-motion` (1920×1080): gate 3 (idle) and gate 5 (memory)

KiB, GPU-driver mappings apart (`mem.rs`). Swap-ins: system-wide pages swapped in during the run.

| run | swap-ins | idle_30s_prompt presents / app frames / layer paints / cpu ms/s | pending, demoted | mem_idle anon / USS | after 5×5 anon / USS | mem_end anon / USS |
|---|---|---|---|---|---|---|
| main-motion-1 | 96702 | 0 / 0 / 0 / 2.29 | yes, yes | 30,076 / 65,236 | 60,356 / 97,072 | 66,536 / 103,252 |
| q5-motion-1 | 35847 | 0 / 0 / 0 / 2.22 | yes, yes | 30,152 / 65,536 | 55,776 / 92,312 | 63,756 / 100,292 |
| main-motion-2 | 3025 | 0 / 0 / 0 / 2.19 | yes, yes | 30,144 / 65,212 | 58,996 / 95,760 | 64,536 / 101,300 |
| q5-motion-2 | 5374 | 0 / 0 / 0 / 2.21 | yes, yes | 30,152 / 65,592 | 58,928 / 96,244 | 65,312 / 102,632 |
| main-motion-3 | 291 | 0 / 0 / 0 / 2.21 | yes, yes | 30,116 / 65,600 | 59,028 / 95,948 | 65,252 / 102,116 |
| q5-motion-3 | 307 | 0 / 0 / 0 / 2.05 | yes, yes | 30,244 / 65,524 | 59,012 / 96,024 | 65,216 / 102,228 |

**Gate 3 (30 s idle with a pending prompt and a demoted worker = 0 frames): pass.** All six runs had 0 presents, 0
app frames and 0 layer paints, with `prompt_pending: true` and `demoted_worker: true`. (`idle_10s` too: 0 and 0.)

**Gate 5 (memory within +5 % of main): pass.** Every run's RSS at its end is above its idle RSS. **Series 1 ran while
bella was swapping** (96,702 and 35,847 pages swapped in system-wide during main-1 and Q5-1; series 2 and 3: 291 to
5,374), and Q5-1's anon after the 5 × 5 load is 3,150 KiB or more under every other run's: paged out. So the series-1 pair is
skipped (the brief: skip runs that page out). Means over series 2 and 3:
- after the 5 × 5: anon **−0.07 %**, USS **+0.29 %**;
- at the end: anon **+0.57 %**, USS **+0.71 %**.

Over all three series, for the record: after the 5 × 5 anon −2.61 %, USS −1.45 %; at the end anon −1.04 %, USS −0.49 %.
