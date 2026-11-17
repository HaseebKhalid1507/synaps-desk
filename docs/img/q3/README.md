# Q3 Life: evidence

Recorded on bella with `scripts/capture-linux.sh` (a headless gamescope at 144 Hz, x11grab at 30 fps,
`MESA_VK_WSI_DEBUG=sw`), driven by `evidence.sh` (here). The binaries:

- **main** = `4f87411`. The binary is Q4's release build (`bella:~/.local/bin/synaps-desktop-hybrid.q4`,
  sha256 `61e724e6…`). `git diff 609cf04 4f87411` is empty, and the md5 of every `.rs` file in bella's
  build mirror of that tree matches `git archive 4f87411`. No source file there is newer than the binary.
- **Q3** = this branch's release build, installed as `bella:~/.local/bin/synaps-desktop-hybrid.q3`.

## Side by side at rest (`side-<theme>-<size>.png`: main left, Q3 right)

The `--offline` demo, frame 45 of a 2 s capture, untouched. Pixel diff, main vs Q3, as connected regions:

| | 1920×1080 | 1280×720 |
|---|---|---|
| nord | 152 px: 3 glyph boxes (two rail glyphs, one gutter glyph) | 108 px: 2 glyph boxes |
| night-city | 148 px: the same 3 boxes | 112 px: the same 2 |
| default | 148 px: the same 3 boxes | 112 px: the same 2 |
| paper | 1,287 px: the same 3 boxes, plus one inline-code chip (847,426, 67×17) | 112 px: the same 2 |

`rest-zoom.png` shows each changed box at ×8 (the chip at ×4):

- the rail's running session: the 8×8 square becomes ⠹, lit and unlit dots;
- the rail's workers session: the two squares become the two heads, {1,6};
- a running call's gutter glyph: the square with its scan row becomes the snake;
- paper's inline code: the near-black chip with light text becomes a light chip (`well` = `tool_out`) with dark
  text (`code_in`, 4.87:1 at its lowest across the 140 theme × accent cases).

Nothing else on screen moved. The demo's shown session is waiting on a prompt, so its composer doesn't sweep.

## Motion: `life-{full,reduced,off}.mp4` (34 s each, the same script)

`SYNAPS_BENCH_LIFE=4 SYNAPS_BENCH_LIFE_LEVEL=<level> … --bench-motion`, the scripted scene in `bench.rs`. A turn
starts on the bench's session (frame ≈ 58), and then:

1. a `bash` call runs;
2. scout spawns, and its tool count and step change;
3. four more workers spawn;
4. the bash call ends ok, and a `read` fails;
5. scout completes;
6. a prompt comes;
7. a rail row is favourited;
8. the prompt resolves and the turn ends.

**Packing: lossless.** Each one is `libx264rgb -qp 0 -preset veryslow -pix_fmt gbrp` from the x11grab
`.mkv`. The per-frame md5 (`-f framemd5`, gbrp) of each `.mp4` equals its capture's:

- full `8bb486a24f7f`;
- reduced `f031920b4ba0`;
- off `364710a3ef41`.

What the Full clip shows, decoded frame by frame (frame numbers at 30 fps):

| frames | what |
|---|---|
| 100–127 | **P1**: the gutter's spinner steps every 4 frames (8 Hz), its dots on device pixels (2×2). The rail's running and workers glyphs step with it, phase-locked. **P2**: the composer's top edge (y 951) carries the comet, 12 px a frame (30 Hz), four bands, left to right. |
| 142–190 | **P2**: the running `bash` row's bottom edge sweeps (y 872) once the call has run 1 s, in the row's lit hue, with the composer's still sweeping. |
| 191–198 (`full-roll-f191-198.png`) | **P5**: scout's `1 tool` → `2 tools` (the count changes length, so the whole count rolls; the clock beside it doesn't), and its live line's step rolls up. |
| 264–330 | **P2**: five live workers' lines sweep (y 552, 616, 680, 744, 808), all on one timer. |
| 306–319 (`full-write-on-f306-319.png`) | **P4**: `bash` ends ok: the spinner goes, the box stays blank for one 40 ms step, then the check writes on over `base`. |
| 463–478, 568–575 | **P5**: the top bar's NEEDS YOU count rolls 0 → 1 and back (only the digit cell rolls), and the rail's NEEDS YOU / RUNNING chips roll. |
| 522–533 (`full-star-f522-533.png`) | **P6**: the favourited row moves into FAVOURITES (the rail's own reveal). Its filled star shows the four `you_bar` sparks leaving it diagonally (frames 523–526), then it rests. The 4 px hop is there, but it is hard to see in frames that are also sliding. |

`life-<level>-sheet.png`: frames 150, 193, 311, 470, 527 and 600 at each level.

- **Reduced, frames 140–200:** only the chip clocks change, once a second (frames 162 and 192), plus the scripted event at 194. No spinner steps and no sweep. Every glyph is drawn in its still frame (⠹ for a running call).
- **Off, the same frames:** only the scripted event at 194. No clock ticks.

**Not in the clips:** P3, the settle pass. It plays only where this window drives the session (`drives()`
needs a client), and the bench has none. `ui_tests::handoff_in_travels_and_goes` covers it:

- the sweep runs while busy;
- at READY, a pass with Handoff's light starts from the same corner as the bar's light;
- it is gone after `sig`;
- Reduced and Off have no sweep and no pass.
