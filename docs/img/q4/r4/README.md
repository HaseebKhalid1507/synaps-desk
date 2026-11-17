# Q4 r4: the drain, from the shipped file

**Every number below was read from `live-loss-back.mp4`, the file in this folder.** Decode it with
`ffmpeg -i live-loss-back.mp4 -f rawvideo -pix_fmt rgb24 -`. Frame numbers are the file's own (0–379).

## The clip

- **Run:** live, on the desktopproto sandbox on bella (never a live daemon).
- **Capture:** `scripts/capture-linux.sh`: headless gamescope at 144 Hz, 1920×1080, x11grab at 30 fps,
  `MESA_VK_WSI_DEBUG=sw`, `CAPTURE_HOVER=0` (nothing moves the pointer).
- **Binary:** this branch's release build at `fe27428`, rebased on main `e2db53f`.
- **Script:**
  1. a reload;
  2. a storm of 4 reloads;
  3. `synaps daemon stop` (a real loss);
  4. 7 s later, `synaps daemon start`.
- **What's here:** the capture's frames 640–1019 (12.7 s): the loss, held, and back.
- **Packing: lossless.**
  - `libx264rgb -qp 0`, RGB (`gbrp`), no chroma subsampling, no rate control.
  - All 380 frames are byte-identical to the raw capture's frames 640–1019 (checked frame by frame).

## Why r3's clip showed a stall: the packing, not the app

r3's `live-reload-storm-loss-back.mp4` was packed with `libx264 -crf 20 -preset slow -pix_fmt yuv444p`. That is lossy, with a keyframe every 250 frames. The r3 README called the captures "lossless", which was true of the raw `.mkv` but not of the packed files.

At CRF 20 the encoder treats the drain's small per-frame colour steps as below its threshold. It keeps the old pixels (skipped blocks) and only catches up on the next keyframe. The big changes still get encoded: the beat glyph and the tear. That is why the re-gate saw:
- fresh frames;
- a frozen composer bar;
- the elements at different stages of the drain;
- a 39 KB frame among 90 B ones.

The pops land on the keyframes: f500 in r2, f750 and f1000 in r3.

**Reproduced.** This run's raw capture, packed with r3's exact command, gives the re-gate's numbers:
- (633,1000) is `#81a0c3` from f700 to f749;
- it is `#979da6` from **f750**;
- it is back to `#84a0c6` only at **f1000**.

The same pixel in the raw capture moves on every frame from f718 to f726, and back from f962 to f972. These are capture frame numbers; subtract 640 for this file's.

**In the app:**
- **Render:** this run's debug log (`SYNAPS_DESKTOP_LOG=debug`, `synaps_desktop::link`) shows the drain computed on 40 frames in its 360 ms, 7 or 14 ms apart (one gap of 21 ms). The `sw` present costs a frame now and then. Under `dri3`, a bench run of a loss (`SYNAPS_BENCH_LINK=4 --bench-motion`, offline) draws it every 7 ms.
- **Paint:** the new test `the_drain_is_painted_every_frame` reads the composer bar's quad from the scene GPUI hands the renderer, on every 7 ms frame of a loss and a return. That is the cached app view and its beat layer, opened the way the app opens them. The painted colour is what render computed, every frame, and it changes on every frame from the first to the last.

## Re-gate checks, from this file

**The tear:** the connection words' cell sits 3 px low at f74–75 and 2 px high at f76, and is back at f77.

**The drain:** columns are the composer bar's pixel (633,1000), then mean chroma (the re-gate's method: max−min of R, G, B over the pixels coloured at f60), then the pixels that changed since the last frame. **"Pixels changed"** counts a pixel only when its R, G and B
differences sum to more than 6, so the drain's first sub-unit steps read 0 there: counting any difference, f78 is 891
and f79 is 7461.

| frame | bar (633,1000) | wordmark | SESSIONS | theme square | disc | bar | pixels changed |
|---|---|---|---|---|---|---|---|
| f77 | `#81a1c1` | 67.3 | 74.9 | 64.0 | 88.7 | 64.0 | 12744 (the tear) |
| f78 | `#81a1c1` | 67.3 | 74.8 | 64.0 | 87.7 | 64.0 | 0 |
| f79 | `#83a1c0` | 66.4 | 73.5 | 61.0 | 84.8 | 61.0 | 0 |
| f80 | `#85a1be` | 65.1 | 71.2 | 57.0 | 79.3 | 57.0 | 72 |
| f81 | `#88a0ba` | 62.7 | 67.2 | 50.0 | 70.8 | 50.0 | 1363 |
| f82 | `#8ea0b3` | 58.5 | 60.5 | 37.0 | 54.8 | 37.0 | 1701 |
| f83 | `#929fae` | 55.6 | 55.7 | 28.0 | 42.4 | 28.0 | 1582 |
| f84 | `#949fac` | 54.3 | 53.8 | 24.0 | 34.7 | 24.0 | 416 |
| f85 | `#959faa` | 53.6 | 52.3 | 21.0 | 31.8 | 21.0 | 0 |
| f86 | `#969fa9` | 52.9 | 51.4 | 19.0 | 30.1 | 19.0 | 0 |
| f87–f321 | `#969fa9` | 52.9 | 51.4 | 19.0 | 29.2 | 19.0 | 0 on every frame |

- **It leaves on time.** The tear's last frame is f76. The drain shows from f78 (the disc, SESSIONS), and the bar leaves the live colour at **f79**, two frames after the tear.
- **It reaches grey within the limit.** The bar is drained at **f86**: 8 frames, 267 ms, against the limit of about 12.
- **Every element moves together on every frame**, then holds. Nothing changes between f87 and f321.
- **Largest step:** on the bar's chroma, 13 of 45 (29 %), at f81→f82; in RGB, 9.2 of 32.0 (29 %). The re-gate asked for ¼ or less.
  - The curve itself sets this. A 360 ms `inout` drain moves up to 2.86× its mean rate, and a 33 ms capture frame samples about 26 % of the travel at its steepest. Colour mixing in linear light adds a little more.
  - At display rate (7 ms frames) every step is under 8 % of the travel. `the_drain_is_painted_every_frame` asserts this on the painted quad.
  - Getting under ¼ at 30 fps means a longer or gentler drain, which is a spec change (motion.md §5.3). I left it as specified.

**The restore,** after the hello (the Scan's line starts down the window at f322):

| frame | bar (633,1000) | wordmark | SESSIONS | theme square | disc | bar | pixels changed |
|---|---|---|---|---|---|---|---|
| f323 | `#969fa9` | 52.9 | 51.4 | 19.0 | 19.2 | 19.0 | 95773 (the Scan) |
| f324 | `#959faa` | 53.6 | 52.3 | 21.0 | 21.0 | 21.0 | 187814 |
| f325 | `#949fab` | 54.0 | 53.2 | 23.0 | 23.8 | 23.0 | 188007 |
| f326 | `#929fae` | 55.6 | 55.7 | 28.0 | 28.5 | 28.0 | 188160 |
| f327 | `#8f9fb2` | 57.7 | 59.3 | 35.0 | 35.4 | 35.0 | 188678 |
| f328 | `#8aa2b6` | 62.4 | 66.6 | 49.0 | 50.5 | 46.7¹ | 188786 |
| f329 | `#85a1bd` | 64.7 | 70.5 | 56.0 | 59.8 | 54.1 | 163357 |
| f330 | `#83a1bf` | 66.0 | 72.7 | 60.0 | 64.4 | 59.4 | 134160 |
| f331 | `#81a1c1` | 67.3 | 74.8 | 64.0 | 67.1 | 64.0 | 97509 |

¹ The Scan's line crosses the bar's box on that frame.

- **It moves together on every frame.** f324–f331: 8 frames, every element together. The largest step on the bar is 11.7 of 45 (26 %).
- **No pop afterwards.** The pixels changed per frame fall 46012, 34482, 9594, 5745, 3825, 1911 over f332–f337 (the Scan's glow fading). After that, one frame moves: f341, 1910 pixels, a 1 px row across the whole window at y 1079 (the bottom edge). Nothing else moves to f379.
- **The disc ends at a different colour, and that is correct.** It was the loss's red (`err`) from f51, then drained red; after the hello it is the connected colour. 88.7 → 68.1 is a change of hue, not a short restore.

**NEEDS YOU** (the stub's left edge, the top bar's first lit pixel between x 640 and 1280) is at **x 839 on every frame, f0–f379**. That covers the loss's words (`reconnecting ··`, then the client's error), the tear, the drain, the Scan and the restore. The words keep the connected words' width on a loss now, padded or cut with `…` (`chrome.rs hold_width`; test `needs_you_holds_still_through_a_loss`, which fails without the fix: 387.5 vs 398.0).
This clip's window has **no session attached** ("No open sessions", "not attached"). With one attached, the
session's own connection re-attaches after the link's hello, and "reattaching ···" now holds the same width
until it is attached again (r5, Shady #15 M1: the same test sends `Event::Reattaching` and `Attached { reconnect }`).

## Stills

- `drain-top-f76-87.png`: the top-left band (disc, wordmark, SESSIONS), f76–f87, one row per frame.
- `restore-top-f320-331.png`: the same band, f320–f331.
