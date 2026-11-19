# Cards D: evidence (round 1, head 178939d)

> **Media:** the screenshots and clips named here are in the private repo [synaps-desk-evidence](https://github.com/HaseebKhalid1507/synaps-desk-evidence/tree/main/docs/img/cards-d), at the same paths. See [EVIDENCE.md](../../EVIDENCE.md).

Round 2 (the finish gate, Shady's review, H2 "Turning lattice") is in [`r2/`](r2/README.md); where they
differ, round 2 is current.

Built from `cards4/proposal.md` (direction D, approved as is). Head of `design/cards-d`; binary
`bella:~/.local/bin/synaps-desktop-hybrid.cardsd`. Every run: a headless gamescope on bella (144 Hz,
`MESA_VK_WSI_DEBUG=sw` for the stills and clips), an absolute `XDG_CONFIG_HOME` under `~/.cache`,
the `--offline` demo or the **desktopproto** sandbox daemon. bella's real
`~/.config/synaps-desktop/settings.json` kept its mtime (`1795008295` before and after).

## Side-by-sides (main `9b0d263` left, PR right, `--offline`)

`sbs-{nord,night-city,default,paper}-{1920x1080,1280x720}.png`.

## States at rest

`states-sheet-nord.png`, top to bottom: a folded run (`N tool calls`, the tally, the facts), a done
call (the tick, quiet), spawn rows, a failed call (`Failed`, the cross, the error well), a waiting call
(`Waiting for you`), an open run card (its rule, the rows, a failed call's well, the diff), the live
authorize card (`Allow this?`, the well, Deny ghost / Allow accent), and the receipts in the call's
row (`Allowed 08:58 · 15s` in `dim`, `Denied 09:00` in `err`).

## Live on the desktopproto sandbox (a real prompt, answered from the app)

The sandbox profile with `tools.activation_confirm = prompt` (removed afterwards) and the local
`qwen3:14b` model (the sandbox's Anthropic login had expired). The model calls `activate_tools`; the
daemon asks; the app answers.

| Still | What |
|---|---|
| `live-full-asked-allow.png` | the card asks: `Allow this?`, `asked 08:58`, the daemon's text in the well, Deny / Allow |
| `live-full-hover-allow.png`, `live-full-hover-deny.png` | hover on each |
| `live-full-allow-pressed.png` | the fold in flight: the call's orbit, the receipt lit `Allowed 08:58 · 15s`, the composer's orbit (RUNNING) |
| `live-full-allowed.png` | at rest: the tick, the receipt quiet |
| `live-full-asked-deny.png`, `live-full-denied.png` | the same, denied: `Denied 09:00` in `err`, the slash |
| `live-{reduced,off}-{asked,allowed,asked2,denied}.png` | Reduced and Off: same states, no motion |

Frame strips (`frames/`, every Nth frame of the lossless clips):

- `live-ask-ping-full-f622-662.png`: the card arrives in its call's run, the flush and the dot's ping, once.
- `live-allow-fold-full-f1538-1584.png`: Allow pressed, the card folds under its call row on `glide`, the
  receipt lands lit and decays, the orbit closes to the tick.
- `live-deny-fold-full-f4294-4324.png`: the same for Deny.
- `run-unfold-4calls-full-f55-78.png`, `run-fold-4calls-full-f243-253.png`: the "4 tool calls" run unfolds and
  folds (17 and 7 frames at 60 fps). Round 1 labelled these `call-*`; they are the run (finish gate).
- `run-unfold-full-f368-398.png`, `run-fold-full-f525-537.png`: a folded run opens (its rows staggered in,
  25 frames) and closes (9).
- `call-arrives-full-f90-110.png`, `turn-end-full-f1788-1820.png` (the settle pass; no sweep). Round 1's orbit
  and tick strips were mis-cropped and are removed; the orbit is replaced by H2's lattice, and round 2's strips
  are in [`r2/`](r2/README.md).

Reduced and Off (`fold-reduced`, `fold-off`): every fold and unfold is one changed frame (spatial ends at
once at Reduced; nothing moves at Off). The life scene at Reduced changes on 42 runs of frames, at Off on 17
(the clocks and the state changes only).

## Lossless clips (not in the repo: 330 MB)

`~/Jawz/workspace/synaps-desktop/cardsd-evidence/clips/` on avante, all 1920×1080, 60 fps,
libx264rgb `qp=0` (High 4:4:4 Predictive, gbrp; checked in each header):

| Clip | Level | What |
|---|---|---|
| `live-full-ask-allow.mkv` (90 s) | Full | live: ask → hover → Allow fold; ask → hover → Deny fold |
| `live-full-allow.mkv` (150 s) | Full | live: `/model`, then a turn of three bash calls, queued → running → done, one failing (`Exit 2`, its well open); `live-full-bash-exit2.png` is its frame at 60 s |
| `live-reduced-ask.mkv`, `live-off-ask.mkv` (100 s) | Reduced, Off | live: ask → Allow, ask → Deny |
| `life-{full,reduced,off}.mkv` (33 s) | each | `SYNAPS_BENCH_LIFE=4 --bench-motion`: a call queued → running (orbit) → done (tick, clock roll), workers spawn, a failure (flush, cross), a prompt, the turn's end |
| `fold-{full,reduced,off}.mkv` (14 s) | each | `--offline`: a failed call's well open/close, a folded run open/close (stagger) |

`runs.py` beside them lists every run of changed frames in a band (`python3 runs.py <clip> x0,y0,x1,y1`).

## Bench vs main (`--bench-motion`, bella, headless gamescope, `dri3`; main `9b0d263` and the PR alternating)

| case | cardsd-1 | cardsd-2 | cardsd-3 | main-1 | main-2 | main-3 |
|---|---|---|---|---|---|---|
| idle_30s_prompt.frames_presented | 0 | 0 | 0 | 0 | 0 | 0 |
| idle_10s_after_beat.frames_presented | 0 | 0 | 0 | 0 | 0 | 0 |
| beat_25_30s.frames_presented | 900 | 900 | 900 | 900 | 900 | 899 |
| beat_25_30s.cpu_ms_per_s | 45.38 | 63.47 | 39.25 | 67.42 | 46.5 | 41.2 |
| load_5x5_30s.frames_presented | 899 | 900 | 899 | 240 | 240 | 240 |
| load_5x5_30s.cpu_ms_per_s | 52.83 | 48.44 | 43.41 | 17.17 | 19.31 | 16.91 |
| load_5x5_sweep_30s.frames_presented | 900 | 900 | 900 | 900 | 899 | 900 |
| load_5x5_sweep_30s.cpu_ms_per_s | 53.05 | 63.85 | 41.27 | 47.21 | 45.49 | 45.95 |
| load_5x5_sweep_30s.orbits | 6 | 6 | 6 | None | None | None |
| load_5x5_sweep_30s.sweeps | None | None | None | 1 | 1 | 1 |
| mem_end.anon_kb | 74020 | 73928 | 73904 | 76068 | 74160 | 73952 |
| mem_end.uss_kb | 117948 | 117136 | 100584 | 120004 | 118240 | 117724 |
| mem_after_load_5x5.anon_kb | 69212 | 69164 | 69148 | 69804 | 68880 | 68376 |
| mem_after_load_5x5.uss_kb | 98632 | 112460 | 108400 | 99596 | 112996 | 112460 |

`cpu_ms_per_s` is the app's CPU per second of the case. The JSON is in
`~/Jawz/workspace/synaps-desktop/cardsd-evidence/bench/`. The PR's JSON says `orbits` where main's says `sweeps`.

## Not covered

- Chips stepping (build step 7) and the event row (step 6): not in this PR (#21 and #22 are open).

## Round 3 (N1, S1, S3; `SYNAPS_BENCH_FOLDS=1`)

Lossless clips off the repo: `cardsd-evidence/r4/clips/f3-{full,reduced,off}.mkv` (1920×1080) and
`l3-{full,reduced,off}.mkv` (1280×720), 60 fps captures of a 144 Hz headless gamescope. The bench's
fold script: the turn ends (auto-fold, f≈772), the run unfolds (f≈925), the header goes a third of the
way down the view and the run folds by `toggle_run` (f≈1135), it unfolds with `scroll_to_end` (f≈1280),
folds at the list's end (f≈1404), and the last call's body opens and closes at the end (f≈1675, f≈1795).

| Still / strip | What it shows |
|---|---|
| `r3-fold-click-mid-f1140-1180.png` | a click fold mid-screen: f1140, 1150, 1160, 1180. The header holds; the block above it moves 0 px every frame f1140–1175 (`blockmove.py`). |
| `r3-fold-turn-end-f772-796.png` | the auto-fold at the turn's end, every 4th frame: about 22 px a frame over about 22 frames, peak 44 px in one capture frame (f779, a capture slot spanning two intervals; its neighbours are about 22). |
| `r3-unfold-stagger-f1280-1298.png` | an unfold from mid-screen, every 2nd frame: the rows arrive one after another. The column also scrolls, because the bench calls `scroll_to_end` in the same update (f1284's 84 px). |
| `r3-states-1280.png`, `r3-folded-1280.png`, `r3-outline-1280.png` | 1280×720 stills: the states run, folded, and the Outline Allow (`SYNAPS_DESK_ALLOW=outline`). |

The body opening and closing at the end (S1) moves no band of the column ≥ 8 px in any frame.

## Round 4 (Shady #23 r3 H2, H1, M2; design gate r3 N3, S3, S1, S5, N2)

Lossless clips off the repo: `cardsd-evidence/r5/clips/f5-full.mkv` (1920×1080) and `l5-full.mkv`
(1280×720), Full, 60 fps captures of a 144 Hz headless gamescope, each with the bench's marks
(`*.png.log`, `FOLDS <what> <s>`). The bench's fold script now: the run unfolds at the end; a
fold a third of the way down; an unfold there **with no scroll** (N2); at the end, a fold (N3);
an unfold as the list is sent to its end, then a fold with the rows still under the view (N3);
the last call's body opened and closed at the end (S1); a new turn's call streaming into the
held room while the reader scrolls up 0.5 px a frame for 3 s, then stops (H2). Every fold here
is a click (`toggle_run`); the turn-end fold (f≈776) is the only tail-following one.

| Still / strip | What it shows |
|---|---|
| `r4-needs-you-1280-in-first-3s.png` | 1280, about 0.3 s after the prompt came: the rail row, the tab strip, the stub and the composer foot all draw the lattice plus (S3); no framed box anywhere |
| `r4-needs-you-plus-8x-rail-tab-composer.png` | those glyphs at 8×: the breathing ones dim the centre dot |
| `r4-needs-you-ping-f144-162-breath-f150-330.png` | top: the tab strip's ping, f144–162, one ring wave then the still plus; bottom: the composer foot breathing, every 10th frame f150–330 (its centre dot) |
| `r4-N3-fold-end-header-holds-f1464-1516.png` | the fold at the end (every 4th frame): the header and all above it hold (block match: 0 px, f1460–1520); the card's edge rides up over its rows |
| `r4-N3-fold-rows-under-view-f1735-1787.png` | unfolded as the list went to its end, folded with rows under the view: the header holds (0 px, f1730–1790) |
| `r4-N2-unfold-stagger-no-scroll-f1284-1312.png` | the unfold mid-screen with no scroll (every 2nd frame): the rows arrive one after another under a still header |
| `r4-S1-body-opens-shows-first-lines-f2034-2074.png` | the last call's body opened at the very bottom: the row holds while it opens under the view, then the list glides just far enough to show it: 39 px over f2056–2070 (the design gate's measure; r4's "~9 px" was wrong). In r4 the glide began 9 still frames (150 ms) after the reveal landed (f2046 → f2056); round 5 starts it as the reveal settles (85 % in) |
| `r4-H2-slow-scroll-while-streaming-f2330-2500-2690-2840.png` | output streaming while the reader scrolls up 0.5 px a frame: the column only moves up (block match f2330–2840: 215 one-px steps, all one way, never toward the end), and after the scroll stops (f≈2690) nothing moves |
| `r7-square-run-diff-1920.png` | round 7 (Haseeb): square tool cards. `--offline` at 1920, scrolled up: the run card, its rows and the diff wells, every corner 0 px |
| `r7-square-approval-1920.png` | the approval card and its command box, square; Deny, Allow and the checkbox keep theirs (5 px, 3 px) |
| `r7-square-1280.png` | the same at 1280 |

S5, the 1280 turn-end fold with its header off screen (`l5-full` f782–840, block match on the
column): 4, 28, 18, 32, 22, 11, 45, 11, 42, 19, 9, 36, 16, 7, 28, 12, 16, 10, 5, 17, 3, 14, … Still
uneven. Why it's the capture and not the app: the fold's spring can't move faster than 1617 px/s
(its response is `h / 700` for any room under 1050 px, and its peak speed is `h · 2π / (e · r)`),
which is 27 px in 16.7 ms. A late app frame pauses the fold; it never skips ahead. So the 45, 42
and 36 px steps mean those capture slots spanned 28 ms or more of app time, and the small steps
next to them (11, 9, 7) were short slots: x11grab's grab times jitter on a loaded machine (load ≈ 5
here). The app's own steps are bounded by the ui test (≤ 13 px a 144 Hz frame, ≤ 32 px across a
50 ms hitch). Not proven frame by frame: the container's timestamps are synthetic.
