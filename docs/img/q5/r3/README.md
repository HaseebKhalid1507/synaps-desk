# Q5 r3: evidence

Captured on bella with headless gamescope at 144 Hz (`scripts/capture-linux.sh`, with an absolute `XDG_CONFIG_HOME` under `~/.cache/q5r3`). Recorded with x11grab at 60 fps and re-encoded losslessly: libx264rgb, qp 0, gbrp, High 4:4:4 Predictive. Every clip's per-frame md5 equals the raw capture's (`clips.txt`).

The binary is the release build of `ff642fe`, the last code commit, sha256 `593553ad…046e15`. Later commits change docs only.

bella's load average was about 10 the whole time (other builds were running), so the 60 fps capture's frame spacing is uneven: one captured frame can hold 1 to 4 of the app's 144 Hz frames. Motion per *captured* frame is therefore lumpy. The tests check per-frame motion on the test clock.

| file | what | script |
|---|---|---|
| `r3-full.mp4`, `r3-reduced.mp4`, `r3-off.mp4` | 1280×720 nord, `--offline`, 1,320 frames. Three favourites (docs-sync, relay-refresh, which NEEDS YOU, and ci-cache). FAVOURITES folded and unfolded, then its head double-clicked (turned round mid-fold). The rail's edge dragged 21 px out and let go. The transcript wheeled up (the stub appears), then Ctrl+J. | `clip.sh` (drive times in `r3-clip-*-drive.txt`) |
| `r3-full-1920x1080.mp4` | 1920×1080 Full: Ctrl+B ×2, Ctrl+Alt+B ×2 | `clip.sh full 1920x1080` |
| `r3-chips.mp4` | the chips scene (`SYNAPS_BENCH_CHIPS=3`): steps change and clocks tick, then one worker ends done and one failed (the reorder) | `chipclip.sh` |

**The chip reorder (R2-H1, gate B).** `chips-trace.txt` (from `chips.py`, each chip's 1 px frame) and `sheets/chips-reorder-f383-404.png`.
- When reviewer fails and trace-cart completes, trace-cart and scout swap cells.
- trace-cart goes from (652, 525) to (336, 557) and scout the other way, over f380–f397. Both axes move in every frame until they land on their cells exactly at f397. No frame jumps to the end.
- Read from the sheet, trace-cart's left edge goes 652, 635, 547, 525, 450, …, and scout's goes 336, 357, 443, 465, 540, …. Both move one way.
- Per captured frame the steps are 22–88 px around the peak, because of the capture's spacing: an 88 px frame is followed by a 22 px one. Over f380–f383 the average is 62 px per 16.7 ms. The spring's own peak is about 55.
- `a_chip_slides_to_its_new_cell_the_whole_way` asserts every 144 Hz frame: no step reaches pitch/4 (79 px).
- A moving chip is drawn on the canvas, so in every frame where the two cross, one covers the other. No text is drawn through text.

**Ctrl+J at Reduced and Off (gate A).** `sheets/reduced-ctrlj-f622-626.png` and `off-ctrlj-f622-626.png`:
- f622 and f623 show the stub. At f624, the first frame after the key, the card (AUTHORIZE) is in view and the stub has gone, at both levels.
- The thumb goes from 322–423 to 416–539 in that same frame (`thumb-1280.txt`).
- At Full, `sheets/full-ctrlj-f620-668.png` shows the glide, as in r2.

**FAVOURITES keeping the one that needs you (R2-M2).** `sheets/full-favs-fold-f112-135.png`, every frame:
- docs-sync, relay-refresh and ci-cache sit under the head.
- PINNED and what follows rise over ci-cache first, then relay-refresh rises with them over docs-sync, until relay-refresh is under the head.
- No band opens in any frame.
- `full-favs-unfold-f197-220.png` shows the unfold.
- `full-favs-turned-round-f280-319.png` shows the head double-clicked 0.22 s apart. The fold turns round from where it is drawn, and what follows never jumps (R2-L2).
- At Reduced and Off it is one frame each way.

**The thumb (S3, S5).** `thumb-1280.txt` and `thumb-1920.txt`, from `thumb.py`: the transcript thumb in every frame, as (x, top, bottom).
- 1280, the rail's edge dragged 311 → 332 and let go (f395–f404): the thumb holds 424–546 through the drag.
- Let go within 24 px of the auto width, the edge springs back. While it flies, the bar is drawn from the column it flies at (415, f404–f416). It lands at 424 (f417). That's two clean changes, not r2's alternation.
- Reduced and Off: the thumb never leaves 424 (the springback is one frame).
- 1920 Full: the thumb is 514–906 in every frame of all four flights. Only its x moves, riding the right panel's edge 1614 → 1878 and back. There is no in-between frame at 1920.
- Not filmed: an armed chip. The offline scene has no daemon to send a cancel to, so it can't arm one. `a_chip_is_the_columns_share_whatever_it_says` asserts an armed chip's bounds.
