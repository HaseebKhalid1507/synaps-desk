# Q3 Life, round 3: evidence on the merged tree

Round 3 merges main `9121dba` (Q2 Authorize, #18) and answers Shady's re-review and the design re-gate of
`4c63a0c`. Recorded on bella with `scripts/capture-linux.sh` (headless gamescope at 144 Hz, x11grab,
`MESA_VK_WSI_DEBUG=sw`), driven by `evidence3.sh` (here).

- **Q3:** this branch at `699f1b2`, release build sha256 `b4fb51d4…`. (The head, `f72f5ad`, adds only a bench memory sample; its build, sha256 `f9d2baff…`, is the one installed as `bella:~/.local/bin/synaps-desktop-hybrid.q3`.)
- **main:** `9121dba`. Its tree is identical to `c81ab14` (`git diff c81ab14 9121dba` is empty). The binary is Q2's
  release build of that tree (`synaps-desktop-hybrid.q2`, sha256 `e8ee38d9…`). The md5 of every source file in bella's
  Q2 build mirror matches `git archive 9121dba`, and no source file there is newer than the binary.

## Clips (lossless: `libx264rgb -qp 0`, `gbrp`, High 4:4:4 Predictive; each file's per-frame md5 equals its raw capture's)

| file | level | size | fps | frames |
|---|---|---|---|---|
| `life-full-1920.mp4` | Full | 1920×1080 | 60 | 1757 (29.3 s) |
| `life-reduced-1920.mp4` | Reduced | 1920×1080 | 60 | 1759 (29.3 s) |
| `life-off-1920.mp4` | Off | 1920×1080 | 30 | 878 |
| `life-full-1280.mp4` | Full | 1280×720 | 30 | 877 |

The scene is `SYNAPS_BENCH_LIFE=4` (`bench.rs`), the same as round 2's, with these changes:
- the session is this window's to drive (`owner_here`, the way a user's own session is);
- the workers' clocks are their real ages;
- the prompt is answered, every worker finishes and the rail's other sessions settle;
- the turn ends from RUNNING, with the composer's sweep on screen. That is where P3 plays.

## P3, the settle pass (the re-gate's PASS conditions)

**Full** (`life-full-1920.mp4`, 60 fps; sheets `p3-full-f1576-1608.png` and `p3-full-corner-f1577-1580-x4.png` at 4×):

| condition | frames | result |
|---|---|---|
| The comet is gone on the frame the pass starts | f1577: the comet's tail is at x 1268–1303 on the top edge (y 951). f1578: the turn ends. Nothing is left at x 1268–1303. | **yes** |
| The pass leaves the bar's corner together with the Handoff light (one L) | f1578: the pass's head is the first 12 px of the top edge (x 632–643), and Handoff's light is the bar's first 24 px (y 951–974), both from the corner (632, 951) | **yes** |
| It crosses once, in about `sig` | The head runs f1578 → f1592 (x 1303, the edge's end) and the tail leaves by f1607, about 0.48 s. Handoff's light lands by f1603. `sig` is 600 ms, on `out` (the last part of the travel draws nothing new). | **yes, once** |
| It lands into `edge` with no flash and no leftover segment | The head's colour stays at one level from f1578 to f1592 (max channel delta 482). Then the four bands leave in order (318, 193, 96). f1608 clears the last pixel. | **yes** |
| Then 0 presents | No pixel changes from f1609 to the clip's end (f1756, 2.5 s). The window's presents are gate 3's, which is measured by the bench. | **yes** (pixels) |

**Reduced** (`life-reduced-1920.mp4`, 60 fps): the turn ends at f1580.
- The bar flashes: x 633 at y 1000 is (146,173,201) at f1580, then 136 → 132 → 130, and is back at rest (129,161,193) at f1584.
- The top edge (y 951) doesn't change in any frame f1574–1599, so there is no pass.
- Nothing changes after f1587.

**Why there was no pass before:** the r3 clip at `b1e3c17` showed Handoff's light but no pass. Two causes:
- **The pass was hidden.** It drew in the app's frame, on the composer's top border, and GPUI paints a div's border after its children, so the border covered the pass. The comet only showed because the beat layer paints over the frame.
- **The test missed it.** `handoff_in_travels_and_goes` looks up the pass by its debug bounds, so it passed anyway.
- **Fix (`a4d553f`):** the pass, and the in-frame sweep without the layer, now paint `deferred`.
- **Head position (`9c52420`):** the pass's head now starts one band in, so the corner is lit both ways on the turn's last frame.

## The re-gate's should-fix items

- **The plural no longer swaps early** (`tools-roll-f387-400-x3.png`).
  - Scout's count reads `1 tool` through f393 (the digit leaving at f392–393), then `2 tools` from f394, once the digits have crossed. The four workers' rolls at f858 also read `1 tool`.
  - From the change (f389), the row has the new text's layout: `1 tool` sits 7 px left, where `2 tools` rests. So the words keep their place, and nothing moves when the plural lands.
  - The words after the changed digits are the old text's until the roll is halfway (`rolled-words-*`; `transcript_counts_roll` asserts it).
- **Clocks never go backwards.** The four crew clocks read `4s` at f857 and `5s` at f858 (round 2 jumped from 5s back to 3s there). The scene now reports each worker's real age.
- **Rolls step at 30 Hz** (R2-M1, below).
  - In the 60 fps clip, each roll's digits move every other frame (f389/390, 392/393, …).
  - With the composer's sweep on screen, the rolls land on the sweep's ticks.

## Side by side at rest (`side-<theme>-<size>.png`: main `9121dba` left, Q3 right)

These are the `--offline` demo, frame 45 of a 2 s capture, untouched. Both sides now show Q2's authorize card.

The pixel diff between main and Q3 (`699f1b2`), as connected regions:

| | 1920×1080 | 1280×720 |
|---|---|---|
| nord | 148 px: 3 glyph boxes (two rail glyphs, one gutter glyph) | 112 px: 2 glyph boxes |
| night-city | 148 px: 3 glyph boxes | 112 px: 2 glyph boxes |
| default | 148 px: 3 glyph boxes | 112 px: 2 glyph boxes |
| paper | 1291 px: 3 glyph boxes and the inline-code chip (73×23) | 1251 px: 2 glyph boxes and the chip |

- **The glyph boxes:** the braille spinners, in whichever frame each binary caught them.
- **Paper's chip:** Q3's light chip.
- **Everything else** (Q2's card, the rail, the transcript, the composer) is pixel-identical to main.
