# Q5 r4: evidence

Captured on bella with headless gamescope at 144 Hz (`scripts/capture-linux.sh`, with an absolute `XDG_CONFIG_HOME`
under `~/.cache/q5r4`), `--offline`, nord, 1280×720. Recorded with x11grab at 60 fps and re-encoded losslessly:
libx264rgb, qp 0, gbrp, High 4:4:4 Predictive. Every clip's per-frame md5 equals the raw capture's (`clips.txt`;
recomputed from the committed files, they match).

The binary is the release build of `de9435f` (the rebased branch plus r4's fixes), sha256 `2924cdcf…c0a45a88`. bella's
load was 7–8 from other work while filming, so the 60 fps frames are spaced unevenly, as in r3.

| file | what | script |
|---|---|---|
| `r4-{full,reduced,off}.mp4` | 1,620 frames. Three favourites (docs-sync, relay-refresh = NEEDS YOU, ci-cache): FAVOURITES folded, unfolded; a double click (a fold turned round); folded, a double click (an unfold turned round), unfolded; the rail's edge dragged out and let go; the transcript wheeled up; Ctrl+J | `clip.sh` (drive times `r4-clip-*-drive.txt`) |
| `r4-r2drive-{full,reduced,off}.mp4` | 1,920 frames. r2's drive again (`r2/clip.sh`): Ctrl+B ×2, Ctrl+Alt+B ×2 at 1280, the edge, Ctrl+J, NAMED folded and unfolded mid-rail | `r2drive.sh` |
| `bar-trace.txt` | relay-refresh's warn bar (x 0, `(235,203,139)`), every change over the whole clip | `bar.py` |
| `thumb-1280.txt` | the transcript's thumb in every clip | `../r3/thumb.py` |
| `named-rules.txt` | the rail's section rules through NAMED's fold, r2 against r4 | `rules.py` |
| `sheets/*.png` | frame sheets (frame numbers in the names) | below |

## C / R3-M1: the unfold and the turn-rounds keep the kept row

`bar-trace.txt`, Full. The bar's first run is relay-refresh in FAVOURITES. It is 52 px tall (`282-333`) in every frame
of the clip. It is never missing, and in each phase it moves only one way:
- **Fold** f108–f133: 282 → 230, up only.
- **Unfold** f192–f197: 230, 233, 239, 247, 266, 274, 282. Down only, whole in every frame. docs-sync opens above it,
  then ci-cache under it (`sheets/full-favs-unfold-f190-213.png`). r3 had docs-sync in its slot at f197 and
  relay-refresh gone.
- **Fold turned round** (double click) f276–f295: up 282 → 240 (f289), then down 241, 243, 256, 260, 277, 282. No
  frame drops it. r3 jumped 42 px to a 12 px sliver at f296 (`sheets/full-favs-fold-turned-round-f276-299.png`).
- **Unfold turned round** f459–f497: down 230 → 282 (f464), holds, then up to 230 at f497
  (`sheets/full-favs-unfold-turned-round-f456-502.png`, every second frame).
- **Reduced and Off:** one frame each way, for all six moves (`bar-trace.txt`).

## S6: the thumb holds through the edge's spring-back

`r4-full.mp4`: the edge is dragged out over f655–f662 and springs back to its detent over f662–f677. The thumb is
`(974, 424, 546)` from f97 until the transcript is wheeled at f745 (`thumb-1280.txt`, "full clip.sh"). r3 went
424 → 415 → 424.

## S7: the 1280 Ctrl+B / Ctrl+Alt+B flights, against r2

`thumb-1280.txt`, "r2drive.sh" rows; r2's numbers are from `r2/r2-full.mp4` and `r2/r2-off.mp4`, re-run with the same
`thumb.py`.

| flight | r2 Full | r4 Full |
|---|---|---|
| rail fold (Ctrl+B) | f502 382 (in-between), f503 423 | f503 423 |
| rail unfold | f552 384 (in-between), f553 424 | f552 424 |
| right fold (Ctrl+Alt+B) | x rides 974 → 1238 at 424; landed f662 423 | the same; landed f662 423 |
| right unfold | f713 384 (in-between), then 424 riding back | f713 x 1231 at 424, riding back |
| edge drag + spring-back | f807–f831: 350 / 411 / 415 / 357 strobe | no change |

At Reduced and Off the in-between frame is also gone. The frame that re-wraps holds the old thumb, and the next one
draws the new one, so the change shows one frame after the panel moves. It never shows an estimate. For example, the
right panel at Off goes f632 1238 424, then f633 423, where r2 showed 382 at f632.

## S7: NAMED's fold, against r2

`named-rules.txt`: the rule at 290, which is NAMED's head, holds, then glides 15 px to 305. In r2 that happens over
f1293–f1305; in r4 over f1290–f1303 (the drive runs 2 frames earlier).
`sheets/full-named-fold-f1273-1303.png` and `full-named-unfold-f1357-1387.png` are every second frame, the same frames
as r2's sheets moved by 2. TODAY and what follows rise over NAMED's body, which stays drawn under them. No band opens.
Unfolding, TODAY and what follows ride down whole in every frame. I set the fold sheet beside r2's, frame for frame,
and they show the same thing.
