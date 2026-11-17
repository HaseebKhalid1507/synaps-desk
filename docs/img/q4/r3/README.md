# Q4 r3: evidence for round 3 (Shady R1–R4, the press decision, design re-gate 1–5)

> **Correction (r4).** The `.mp4` files here are **not lossless**. They were packed from the lossless
> captures with `libx264 -crf 20 -preset slow -pix_fmt yuv444p`, which keeps a keyframe every 250 frames.
> At CRF 20 a slow colour fade is mostly skipped until the next keyframe. In
> `live-reload-storm-loss-back.mp4` the drain therefore freezes part-way and pops at f750, and the restore
> pops at f1000. The numbers below about the drain (the composer bar's 64 → 19 over 716–726, "drained
> and settled since 726", the still at f733) were read from the **raw capture, not from this file**,
> so this file doesn't show them. The r3 raw captures are deleted.
> **`../r4/` replaces the link evidence.** It has a lossless clip, byte-identical to its capture, and
> every number there is read from the file shipped. It also reproduces this artefact from a lossless
> capture with the same packing command. The other r3 clips (press, tab close, theme changes, Usage)
> have the same packing. The re-gate's measurements of them hold, but they aren't lossless.

Recorded on bella with `scripts/capture-linux.sh` (headless gamescope at 144 Hz, 1920×1080, x11grab
lossless at 30 fps, `MESA_VK_WSI_DEBUG=sw`), driven by xdotool, using this branch's release build (`d6d2af4` plus
the reload-width fix, the r3 head). All captures were made after the Q1 worker's `pkill gamescope`. None of them overlapped it. Frame
numbers are the clip's own (30 fps). `*-drive.log` gives each step's wall time.

## The link, live on the desktopproto sandbox (R1, re-gate 1 and 3)

`live-reload-storm-loss-back.mp4` (38 s). The app is attached with `--profile desktopproto`. No auth
lines were added, so there were no model calls. The daemon was stopped after the run. Times are from
`live-drive.log`. Each `synaps daemon reload` call returns when the daemon is back, about 2.3 s.

| t (s) | frame | step | what the window does |
|---|---|---|---|
| 2.0–4.3 | 90–159 | `synaps daemon reload` | the words read `reloading ·`, in the connected words' width. The rail keeps its lists (OPEN, HISTORY), the disc stays lit, NEEDS YOU doesn't move. No tear, no drain, no Scan |
| 8.3–17.4 | 279–553 | a storm of **4** reloads back to back (the client's real schedule: every 250 ms through each) | the same, four times over 9 s |
| 22.4 | 685 | `synaps daemon stop` (a real loss) | `connect …` in the error colour; the colours stay for the 1 s grace |
| | 715–716 | | the tear, two frames |
| | 717–726 | | the drain: 360 ms, every frame of it (see below), then held |
| 31.5 | 962–971 | `synaps daemon start` | back: the Scan in live colour, the palette restores over 360 ms |

- `link-reload-storm-topbar.png`: the top bar's right half at frames 60, 120, 155, 300, 420, 540, 720 and 900. During the reloads, only the words change.
- `link-reload-storm-rail.png`: the rail at frames 60, 120, 300, 540 (reloads: unchanged) and 720, 900 (lost: `Not connected`).
- `link-tear-drain.png` covers frames 690–733 and `link-scan-back.png` covers frames 900–975 (whole window, ¼ size).
- `link-live-vs-drained.png`: frame 660 (connected, live) next to frame 733 (lost, drained and settled since 726). r2's version of this image used frame 450, which was only half drained. This one replaces it.

**The drain runs every frame (re-gate 1).** The app's own trace (`synaps_desktop::link`, one line per
frame) gives the following:

| run | frames | time | longest frame gap | every frame advances |
|---|---|---|---|---|
| drain 0 → 1 | 24 | 334 ms | 20.7 ms | yes |
| restore 1 → 0 | 39 | 334 ms | 14.2 ms | yes |

The capture agrees. The window's mean chroma per frame falls 26.7 → 9.7 over frames 717–726 and rises
9.7 → 40.2 over 962–972 (the restore lands on the connected screen, which shows more colour). The composer's
accent bar drains with the rest: its blue−red goes 64 → 19 over 716–726 and back to 64 by 972.
`the_drain_and_the_restore_run_every_frame` checks that every frame steps, at 144 Hz.

**About the stall the re-gate saw.** *(Withdrawn in r4: the stall the re-gate measured is the packing,
see the correction at the top. The x11grab explanation below was a guess, and it was wrong for this clip.)*
One of my test captures (`trace3`, not packed here) showed no drain in the clip at all, while the app's
frame log shows it drew all 52 drain frames 7 ms apart (10382–10743 ms).

How much greys, using r2's method on frames 660 and 733 (3-px horizontal mean, chroma > 60): 342
live, 2 drained (99.4 %). At chroma > 45: 667 → 28 (95.8 %). The mean chroma of the coloured pixels goes
60.6 → 29.9.

## Press (the press decision): `full-press-nord.mp4`, `full-press-paper.mp4`, `press-<theme>.png`

The pointer is on a tab. The drive is: hover the inactive tab (f63), hold it down (f87), let go off it
(f111); then the same on the active tab (f142, f166, f190). In the strips, f55 = rest, f80 = hover,
f100 = press (inactive), f180 = press (active). Fill at (330, 75) and (770, 75):

| | rest | hover | press | press : rest | press : hover |
|---|---|---|---|---|---|
| nord, inactive tab | `#101216` | `#1e2024` | `#181a1d` | 1.075 | 1.069 |
| paper, inactive tab | `#e8ebf0` | `#d9dce2` | `#e1e3e8` | 1.074 | 1.070 |
| nord, active tab (rest = hover) | `#1e2024` | `#1e2024` | `#181a1d` | 1.069 | 1.069 |
| paper, active tab | `#d9dce2` | `#d9dce2` | `#e1e3e8` | 1.070 | 1.070 |

The threshold is **1.05:1** (`PRESS_MIN`, about a third of a `STEP`) off both the hover fill and the
rest fill. `press_is_visible_in_every_theme` checks it for 20 themes × 7 accents × 4 roles, along with
every text and mark floor drawn on a press. Where text floors block the step past hover, press sits
halfway between hover and rest, about 1.07 off each, as in nord and paper here.

## Tab close (re-gate 2): `full-tab-close.mp4`, `full-tab-close.png`

Three tabs; the middle one is shown and closed by its × at f151, and its right neighbour slides in
under the indicator. The indicator's blue bar (y 52) and fill (y 62), per frame:

| frames | bar | fill |
|---|---|---|
| 140–149 (before) | 548–782 | 548–782 |
| 150–158 (the neighbour slides in) | **548–782** | 548–783 |
| 160, 165 | 548–782 | 548–782 |

The bar never shrinks or pops: the indicator is drawn above the sliding tab. In r2 it shrank to about
2 px at f149–157.

## Cross-fade: prose colours (re-gate 5): `full-theme-changes.mp4`, `full-theme-prose-crossfade.png`

nord → night-city (f57) → dracula (f96) → paper (f135, a polarity change: instant) → nord. Per-frame
change in the transcript's code span, link, prose, code block and ground. All of them start on the same frame
and end on the same frame: 58–67 for night-city and 97–106 for dracula. None of them changes after
the ground has settled. The sheet shows frames 57–67, every second frame.

## Usage reveal (re-gate 4): `full-usage-reveal.mp4`, `full-usage-reveal.png`

The offline demo now shows Usage from the fixture board (`usage::demo_board`, offline only). Ctrl+Alt+U
at f30. The KPI row and the charts' frames are there at f31. The bars rise from the baseline, the
left columns leading (oldest days first), and the lines draw on. The chart area's mean difference from
its end state, per frame: f29 23.0 (the old view), f31 11.4, f33 7.0, f35 2.6, f37 0.7, f39 0.16, f41
0.03, f43 0. That is about 400 ms. `chart_reveal_skipped_when_reduced` checks the left lead, the line, and
that Reduced and Off show it whole.
