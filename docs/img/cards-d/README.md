# Cards D: evidence

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
- `call-unfold-full-f55-78.png`, `call-fold-full-f243-253.png`: a failed call's well opens and closes (17 and
  7 frames at 60 fps).
- `run-unfold-full-f368-398.png`, `run-fold-full-f525-537.png`: a folded run opens (its rows staggered in,
  25 frames) and closes (9).
- `call-arrives-full-f90-110.png`, `orbit-full-f332-348-4x.png`, `tick-writeon-glyph-4x-f1692-1712.png`
  (a success's tick lit `ok`, settling to `dim`), `turn-end-full-f1788-1820.png` (the settle pass; no sweep).

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
