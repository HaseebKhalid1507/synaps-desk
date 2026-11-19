# Q5 panels: evidence

> **Media:** the screenshots and clips named here are in the private repo [synaps-desk-evidence](https://github.com/HaseebKhalid1507/synaps-desk-evidence/tree/main/docs/img/q5), at the same paths. See [EVIDENCE.md](../../EVIDENCE.md).

Recorded on bella with `scripts/capture-linux.sh` (headless gamescope at 144 Hz, x11grab lossless,
`MESA_VK_WSI_DEBUG=sw`), the `--offline` demo, settings `{"text_size": 16, "theme": <theme>}`. Every run's config is
a scratch folder under `~/.cache/q5` (`capture-linux.sh`'s absolute `$out/config`). bella's real
`~/.config/synaps-desktop/settings.json` kept its mtime (`1794981241`, 00:54:01) and sha256 (`9c379f92…`) from before
the first run to after the last.

Binaries (release, `dbuild … build --release --locked`):
- **main** = `9b0d263` (the `synaps-desktop` tree), sha256 `91e0c957…0c727`;
- **Q5** = this branch at `adcce6d`, sha256 `33ea4d6e…738b7bd`, installed as
  `bella:~/.local/bin/synaps-desktop-hybrid.q5`. The commits after it change tests and docs only: the PR head's
  release build is byte-identical (the same sha256).

The repo is public and a merged blob stays in its history, so the lossless clips are kept off the repo, in the
maintainer's `synaps-desktop/q5-evidence/` folder, at the same paths (`docs/img/q5/<path>.mp4` is
`q5-evidence/img/q5/<path>.mp4`), with `MANIFEST.md`: each clip's sha256 and per-frame md5, and the preview commands.
The `clips.txt` manifests are kept both here and there. The raw x11grab captures were deleted after each clip's frame
md5 was checked against them. The PNGs here are reduced to a 256-colour palette (no dither) to keep them small; the
full-colour originals are in `q5-evidence/` at the same paths. Every frame number in these READMEs is a frame of the
lossless clip.

r1's three clips have no preview here: r4's (`r4/`) supersede them.

Every capture, clip and bench run here is of those two binaries, one after another (`all.sh`: nothing of ours
overlapped a bench run).

Scripts: `shot.sh` (frame 45 of a 2 s capture), `fly.sh` (Ctrl+B in a 60 fps capture; the frame 9 frames = 150 ms
after the rail's fold button first changed), `ev.sh` (the side by sides; its output is `ev.txt`), `clip.sh` (the
drive; `clip-<level>-drive.txt` has each input's time), `sheets.sh` (the frame sheets, from the clips), `all.sh`.

## Side by side (`side-fold-<theme>-<size>.png`: main at rest left, Q5 150 ms into Ctrl+B right)

nord, night-city, default and paper at 1920×1080 and 1280×720. On the right the rail is mid-fold: its content at its
full 312 px width, sliding out under the window's left edge (its costs and ages are what is left in view, nothing
squeezed or rewrapped), the column at its old wrap re-centred in the wider pane, the right panel untouched.

**At rest Q5 draws main's frame** (`compare -metric AE`, the same frame of each):

| | 1920×1080 | 1280×720 |
|---|---|---|
| nord | 8 px | 8 px |
| night-city | 0 | 8 px |
| default | 0 | 0 |
| paper | 0 | 17 px |

Every differing pixel was located (a per-pixel diff of the two frames): 8 at the rail row's running glyph (24, 395)
and 8 at the composer foot's (351, 568 at 1280; 647, 928 at 1920): the beat's phase in the braille spinners at frame
45, the glyphs Q6's README found. paper 1280's other 52 are the authorize card's clock: `asked 04:47` in main's
frame, `04:48` in Q5's (the minute turned between the two runs).

## Clips (lossless, off the repo): `panels-full.mp4`, `panels-reduced.mp4`, `panels-off.mp4`

1280×720 nord, 60 fps, 27 s each, the same drive at each level (`clip.sh`). Each is `libx264rgb -qp 0`, `gbrp`, High
4:4:4 Predictive, 1620 frames, and its per-frame md5 equals its capture's (`clips.txt`).

The drive (times in `clip-<level>-drive.txt`; frame ≈ 60 × (t + 1.5)): Ctrl+B (rail folds), Ctrl+B (unfolds),
Ctrl+Alt+B twice (the right panel), the rail's edge dragged 21 px out and let go (the 312 detent), a click on the
composer and a draft typed then cleared, six wheel notches up (the card scrolls away: the stub), Ctrl+J, the rail
wheeled to HISTORY, NAMED folded and unfolded.

- **The composer's growth is not in the clips.** `--offline` has no client, so the composer is disabled and the typed
  draft never landed. Its evidence is `composer_grows_upward_on_flick` (the UI test) and `composer_grow` /
  `composer_shrink` in `docs/bench/q5`.

Frame sheets (each frame labelled with its frame number in the clip):
- `full-rail-fold-f118-154.png`, `full-rail-unfold-f198-234.png`: every 3rd frame; the rail slides out and back in,
  clipped at full width, the strip landing.
- `full-right-fold-f278-314.png`: the right panel, mirrored (its left edge rides the box's).
- `full-edge-detent-f450-482.png`: every 2nd frame; the drag (1:1, the edge at 318…332) and, let go 21 px past 312,
  the spring back to it on `snap`: the rail's edge (its border, x at y 650) at 332, 329, 325, 321, 319, 318, 315,
  314, 313, 312, 311 over f463–f477. At Reduced and Off the edge goes from 332 to 311 in one frame.
- `full-stub-dock-f744-804.png`, `full-stub-leaves-f884-944.png`: the stub arriving over the chips as the card
  scrolls away, and leaving after Ctrl+J; the composer stays put, the dock's top moves.
- `full-named-fold-f1122-1158.png`, `full-named-unfold-f1206-1242.png`: NAMED folding (what follows slides up into
  place) and unfolding (its rows revealed down to what follows, riding down; the rows whose place is below the
  rail's end ride on out of it, the rail never short).
- `reduced-rail-fold-f117-125.png`, `off-rail-fold-f117-125.png`, `reduced-named-fold-f1122-1130.png`: every frame
  around the same inputs at Reduced and Off: the panel and the section change in one frame.

## Tests (in `ui_tests.rs`)

- `panels_fold_on_sheet_clipped_not_squeezed`: rail and right panel, fold and unfold at Full: it starts where it was,
  never leaves [from, to], moves one way, its flight box is the spring's width every frame, 420–600 ms on motion's
  clock; then no frame. Reduced and Off: in one frame, no frame asked.
- `column_rewraps_at_most_once_per_panel_motion` (1100×800, a width that crosses a wrap width both ways): per flight
  the column's wrap changes at most once, at the start when the pane narrows, in the frame it lands (or the next)
  when it widens; the column is centred in the pane as drawn every frame; scrolled back, the top row's top never
  moves (flight and rewrap included); following, the list follows and stays at its end.
- `a_section_unfolding_never_leaves_the_rail_short` (1440×700, a bucket at the rail's top unfolding): every frame,
  the rows after it whose place is below the view are drawn where they ride, contiguous, down to the view's end.
- `sticky_and_palette_follow_a_panel_in_flight` (1440×900): every frame of a rail fold and unfold, Q7's pinned label
  stays pinned, on its band on the column as drawn while inline; with Q6's palette open, every frame of a fold and
  unfold it is centred on the centre pane as drawn, under the tab strip.
- `drag_is_one_to_one`, `detent_settles` (20 px out → back to the auto width on `snap`, 150–260 ms, auto again; a
  release just over the fold threshold folds; a double-click springs to the auto width on `sheet`),
  `window_resize_never_animates`.
- `dock_height_springs_on_sheet`, `composer_grows_upward_on_flick`, `rail_section_folds_on_sheet`,
  `a_chip_arrives_from_below_at_lift`.
- Unchanged and passing: F1's four, `scroll_tests_unchanged`, `idle_requests_no_frames`, `off_is_static` and every
  other test (583 in `synaps-desktop`).
