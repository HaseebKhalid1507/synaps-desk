# Q4 r2: evidence for the review fixes (Shady H1–H4, M1–M8; finish gate 1–5)

Recorded on bella like r1: `scripts/capture-linux.sh` (headless gamescope at 144 Hz, 1920×1080,
x11grab lossless at 30 fps, `MESA_VK_WSI_DEBUG=sw`), driven by xdotool, this branch's release build.
Frame numbers are the clip's own (30 fps); `<clip>-drive.log` holds each step's wall time, and a
clip starts about 1 s before its `t0`, so frame ≈ 30 × (t − t0 + 1.0).

## The link, live on the desktopproto sandbox (H1, H2, finish gate 2 and 3)

`live-reload-storm-loss-back.mp4` (30 s; `live-link-daemon.log` is the daemon's side). The app is
attached with `--profile desktopproto`. No auth lines were added, so there were no model calls. The
daemon was stopped after the run.

| t (s after t0) | frame | step | what the window does |
|---|---|---|---|
| 1.0 | 60 | `synaps daemon reload` | no tear, no drain, no scan. **Corrected in r3:** not "words only". The rail also blanked to `Not connected` and HISTORY went away until the hello, and NEEDS YOU moved about 60 px (the app's behaviour at the time). r3 keeps the rail and the words' width through a reload (`../r3/README.md`) |
| 5.0–6.3 | 179–220 | a storm: 3 reloads in 2 s | the same as the single reload (corrected in r3: the rail blanked too). Nothing replays. That storm was short up/down cycles. r3 runs 4 reloads on the client's real schedule |
| 11.5 | 375 | `synaps daemon stop` (a real loss) | `reconnecting ·`; colours unchanged for the 1 s grace |
| | 404–406 | | the tear: three full-width `err_mark_ch` lines over 2 frames (`link-tear.png`, frames 403–407) |
| | 407–420 | | the drain: every role but the grounds and the words greys toward its own luminance, then holds (`link-live-vs-drained.png`: frame 390 live, 450 drained. **Corrected in r3:** the design re-gate found the drain stalled here. It ran 4 frames, then held for about 3 s and finished in one frame at f500, and the composer's bar never drained, so frame 450 isn't the settled drain. That is fixed, and `../r3/link-live-vs-drained.png` replaces this image) |
| 17.5 | 556 | `synaps daemon start` | the client's reconnect backoff runs |
| | 674–690 | | back: the Scan sweeps down in **live** colour (finish gate 2), and the palette restores from where the drain is (`link-scan-back.png`, frames 672–716 every 4th) |

`link-reload-and-storm.png`: the top 300 px at frames 50, 62 (reload), 175, 190 (storm) and 300. Only
the connection cell's words differ.

How much greys (finish gate 3), measured on frames 390 (live) and 450 (drained). Each pixel's chroma
is taken over a 3-pixel horizontal mean, which cancels LCD subpixel fringes. Coloured pixels
(chroma > 60): 306 live, 2 drained (99.3 % greyed). Chroma > 45: 610 live, 28 drained (95.4 %).
Mean chroma of the pixels coloured when live: 60.5 → 30.6. That 0.506 ratio is the drain's 70 % in
linear light. The wordmark, section heads, rail marks and chart series are all drained now: the
drain covers every role but the grounds and the ink/prose (`theme.rs UNDRAINED`), and
`drain_keeps_every_floor` asserts it for every role.

## Tab × (finish gate 1): `full-tab-close-rail-scrollbar.mp4`, `full-tab-close.png`

Two tabs are opened from the rail, the middle one is shown, then its × is clicked (frames 146–165,
the strip only). The indicator's fill and bar go straight to where the next tab will end up and hold
there while that tab slides under them. The old swing (121 px right and back) is gone.
`reduced-tab-close.png`, `off-tab-close.png`: the same close, one frame.

## The rail's scrollbar fade (M7, finish gate 5): `full-rail-scrollbar.png`

From the same clip: the pointer comes onto the rail, the wheel scrolls it, then the pointer leaves.
The strip is x 296–311, frames 210–303 every 3rd. Thumb colour at x 303, per frame:

| frames | thumb |
|---|---|
| 185 | none (ground `#1c2028`) |
| 216 → 218 | `#474b55` → `#4d525c` → `#4e535d`: in over about 90 ms |
| 218–295 | `#4e535d` (held while the pointer is over it and for 1 s after) |
| 296 → 301 | `#4e525c` → `#4d525c` → `#4b4f59` → `#464a53` → `#393d45` → ground: out over about 170 ms |

The transcript's bar shows whenever its rows overflow, so it never fades. The rail's shows only while
it's in use, which is what this records.

## Rail reorder (H3): `full-rail-reorder.mp4`, `full-rail-reorder.png`

A row is favourited from its menu (frame 54 on). It slides up into FAVOURITES, and every row and head
it passes slides by its old-to-new distance. Flip targets now come from the old layout, by key. Heads
are matched by identity rather than by their count, so OPEN's 12 → 11 doesn't make it a new head. A
sliding item is drawn on its own `chrome` ground, so crossing rows never mix their text. The item that
travels clearly farthest (the favourited row, against the others) is drawn over the rows it passes,
clipped to the rail (`RailFx::leads`), so it can be seen all the way up. Nothing jumps.
`reduced-rail-reorder.png`, `off-rail-reorder.png`: the new order in one frame.

## Hold defers order only (M2): `full-rail-hold-defers.mp4`, `full-rail-hold-defers.png`

docs-sync is shown and a rail row is held down. Ctrl+D favourites docs-sync, which is a reorder. In
sheet cells 3–12 the ★ appears on docs-sync at once (content is live) but the order holds. On
release (cell 12, frame 114) the rows slide (cells 13–17).

## Theme change (M1): `full-theme-changes.mp4`, `full-theme-dark-dark-then-paper.png`

nord → night-city → dracula → paper → nord from the top bar's picker. Dark → dark cross-fades over
`slow` (frames 54–65): grounds, text and marks blend together. The kit's colours and the text views'
defaults switch on the fade's last frame. Dark ↔ light (a change of polarity) is instant (frames
134 → 135). A text-size change never fades (`a_theme_change_cross_fades`). `reduced-theme.png`,
`off-theme.png`: every change in one frame.

## Segmented control (M6, finish gate 5): `full-segment-spring.mp4`, `full-segment-to-*.png`

The control is Settings › Sessions rail: shown | folded. Its 2 px bar springs across to the picked
cell and is drawn over the cells, so the new cell's `raised` fill no longer hides it. The control
moves left between the two clicks because folding the rail widens the pane, which is layout, not
motion. `reduced-segment.png`, `off-segment.png`: the bar in its cell at once.

## Press (H4): `full-press-nord.mp4`, `full-press-paper.mp4`, `press-<theme>.png`

A rail row at rest, hovered, pressed, then let go off it (columns 0–3). The fill, sampled at
(290, 592):

| | rest | hover | press |
|---|---|---|---|
| nord | `#101216` | `#1e2024` | `#121316` |
| paper | `#e8ebf0` | `#d9dce2` | `#e8eaee` |

The row also drops 1 px. In both themes the step toward the ink is blocked by the floors of the
texts on the fill, so the press steps away from the ink instead. It is never the hover fill. Across
20 themes × 7 accents × 4 press roles (`roles_check.py`, the committed `pressed()` mirrored line by
line), press == hover in 0 of 560 cases: 368 step away from the ink, and every press stays ≥ 1.05:1
from its hover and keeps every text ≥ 4.5 and every mark ≥ 3. `press_is_visible_in_every_theme` (theme.rs) checks the
same in Rust.
