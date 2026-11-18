# Q5 r2: `--bench-panels` vs main

bella, headless gamescope at 144 Hz, `--refresh-hz 144`, settings `{"text_size": 16, "theme": "nord"}`, alternating
main and Q5, 3 runs each (`bench.sh`; a run's load and system-wide swap-ins in `log`). Release builds made the same
way (`dbuild … build --release --locked`):
- **main-bench** = `9b0d263` with only the bench commit's `bench.rs` and `main.rs` (`5a181f3`), rebuilt for this round:
  sha256 `8134c8bf…5762`. (r1's main-bench binary was removed with r1's scratch.)
- **Q5 r2** = this PR's head, sha256 `abba38dee70931216a71e7c3464b15ef07c543ce246df93823c663058bcd3b24`.

The scene: 1,061 rows of transcript; Ctrl+B (fold), Ctrl+B (unfold), the same for the right panel, the draft set to
three lines, then cleared. For each action: the 0.8 s from it, then the 1 s after it.

The clips that go with these numbers are not here: the lossless clips (and `clips.txt` manifests) live off the
repo, in the maintainer's `synaps-desktop/q5-evidence/` folder, at the same paths as under `docs/img/q5/` (see
`docs/img/q5/README.md`; the `--bench-panels` scene filmed is `r2/r2-bench-panels.mp4`, previewed in the repo as
`docs/img/q5/r2/r2-bench-panels-preview.mp4`). The raw JSON here is complete.

**1280x720** (means of 3 runs; draw = the app's draw time per presented frame, ms; r1 = this PR's first round, `../`)

| action | main presents | Q5 r2 presents (r1) | main draw p95 / max | Q5 r2 draw p95 / max | over budget (3 runs) main / Q5 r2 | main CPU ms | Q5 r2 CPU ms (r1) | the 1 s after: presents, app frames main / Q5 r2 |
|---|---|---|---|---|---|---|---|---|
| rail_fold | 1 | 67.3 (87) | 24.75 / 25.74 | 2.37 / 24.94 | 3 / 3 | 27.0 | 144.6 (196.9) | 0, 0 / 0, 0 |
| rail_unfold | 2 | 52.7 (71.7) | 3.45 / 5.59 | 2.53 / 4.28 | 0 / 0 | 8.4 | 102.0 (145.3) | 0, 0 / 0, 0 |
| right_fold | 1 | 71 (90) | 3.04 / 3.66 | 2.32 / 3.39 | 0 / 0 | 5.5 | 141.2 (166.0) | 0, 0 / 0, 0 |
| right_unfold | 2 | 52.7 (72) | 3.74 / 4.94 | 2.72 / 6.22 | 0 / 0 | 8.8 | 97.3 (147.8) | 0, 0 / 0, 0 |
| composer_grow | 1 | 27 (27) | 3.57 / 4.49 | 1.81 / 3.17 | 0 / 0 | 7.6 | 47.9 (51.7) | 0, 0 / 0, 0 |
| composer_shrink | 1 | 27 (27) | 2.89 / 3.78 | 2.0 / 2.38 | 0 / 0 | 5.7 | 52.0 (47.4) | 0, 0 / 0, 0 |

**1920x1080** (means of 3 runs; draw = the app's draw time per presented frame, ms; r1 = this PR's first round, `../`)

| action | main presents | Q5 r2 presents (r1) | main draw p95 / max | Q5 r2 draw p95 / max | over budget (3 runs) main / Q5 r2 | main CPU ms | Q5 r2 CPU ms (r1) | the 1 s after: presents, app frames main / Q5 r2 |
|---|---|---|---|---|---|---|---|---|
| rail_fold | 1 | 71 (90) | 2.79 / 4.48 | 2.99 / 4.22 | 0 / 0 | 5.6 | 184.2 (226.1) | 0, 0 / 0, 0 |
| rail_unfold | 1 | 53 (72) | 3.09 / 3.63 | 2.43 / 3.83 | 0 / 0 | 5.7 | 115.1 (178.0) | 0, 0 / 0, 0 |
| right_fold | 1 | 71 (90) | 2.19 / 2.24 | 2.37 / 4.78 | 0 / 0 | 4.7 | 152.5 (208.3) | 0, 0 / 0, 0 |
| right_unfold | 1 | 52.7 (72) | 3.19 / 3.96 | 3.07 / 6.84 | 0 / 0 | 5.7 | 123.6 (168.4) | 0, 0 / 0, 0 |
| composer_grow | 1 | 27 (27) | 3.04 / 3.52 | 2.08 / 4.05 | 0 / 0 | 6.3 | 55.1 (57.9) | 0, 0 / 0, 0 |
| composer_shrink | 1 | 27 (27) | 2.52 / 3.14 | 2.65 / 4.36 | 0 / 0 | 5.4 | 56.5 (60.3) | 0, 0 / 0, 0 |

- **What changed since r1:** a panel's width is on whole pixels, and a frame is asked for only when the drawn width will
  differ (`Motion::spring_px`; the slow tail of the spring moves under a pixel a frame, and r1 drew those frames
  identical). A fold now presents 67–71 frames (r1 87–90), an unfold 53 (r1 72).
- **CPU per fold or unfold:** 97–184 ms over the 0.8 s (r1 145–226 ms; main 5–27 ms, one or two frames). Q5 draws
  ~2 ms of CPU per presented frame, about what main's single frame costs.
- **Over budget:** at 1280 the rail fold widens the pane past a wrap width, and the column re-wraps once, in the
  landing frame. That frame is Q5's max (24.9 ms), the same rewrap as main's only frame of the fold (25.7 ms). One frame
  per fold over budget, as main. Nothing else is over budget.
- **`dropped_frames` is not jank here**, so it is not in the table: the bench counts refresh intervals with no present,
  and a pixel spring presents only when a width changes (a 280-px fold on `sheet` draws each width once). Raw values
  are in the JSON.
- **After:** no presents and no app frames in the second after any action, either build.
- **Swap-ins:** bella was swapping in some runs (`log`: up to 59,304 pages in `q5-panels-1920x1080-1`). The per-action
  means above include them.
