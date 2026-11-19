# Q5 r2: evidence

> **Media:** the screenshots and clips named here are in the private repo [synaps-desk-evidence](https://github.com/HaseebKhalid1507/synaps-desk-evidence/tree/main/docs/img/q5/r2), at the same paths. See [EVIDENCE.md](../../../EVIDENCE.md).

bella, headless gamescope at 144 Hz (`scripts/capture-linux.sh`, an absolute `XDG_CONFIG_HOME` under `~/.cache/q5r2`),
recorded with x11grab and re-encoded losslessly: libx264rgb, qp 0, gbrp, High 4:4:4 Predictive. Every clip's per-frame
md5 equals the raw capture's (`clips.txt`). The binary is the PR head's release build, sha256
`abba38dee70931216a71e7c3464b15ef07c543ce246df93823c663058bcd3b24`.

The repo is public and a merged blob stays in its history, so the lossless clips are kept off the repo, in the
maintainer's `synaps-desktop/q5-evidence/` folder, at the same paths (`docs/img/q5/<path>.mp4` is
`q5-evidence/img/q5/<path>.mp4`), with `MANIFEST.md`: each clip's sha256 and per-frame md5, and the preview commands.
The `clips.txt` manifests are kept both here and there. The raw x11grab captures were deleted after each clip's frame
md5 was checked against them. The PNGs here are reduced to a 256-colour palette (no dither) to keep them small; the
full-colour originals are in `q5-evidence/` at the same paths. Every frame number in these READMEs is a frame of the
lossless clip.

Previews in the repo (`*-preview.mp4`): H.264 `-crf 23 -preset veryslow -tune animation`, yuv420p, made from the
lossless clips at their full 1280×720 and 60 fps, every frame kept, so a frame number here is the same frame in the
preview. Lossy: they show the motion, not exact pixels; the traces and the pixel claims below are from the lossless
clips. Here: `r2-bench-panels-preview.mp4` (the composer's growth and
shrink). r2's other clips have no preview: r4's `r4-r2drive-*` re-run their drive on the final build, and r3's
`r3-chips` the chips.

| file | what | script |
|---|---|---|
| `r2-full.mp4`, `r2-reduced.mp4`, `r2-off.mp4` (off the repo) | 1280×720 nord, `--offline`, 60 fps, 1,920 frames: Ctrl+D, FAVOURITES folded and unfolded, Ctrl+D; Ctrl+B ×2, Ctrl+Alt+B ×2; the rail's edge dragged 21 px out and let go; the transcript wheeled up (the stub), Ctrl+J; the rail wheeled down to HISTORY, NAMED folded and unfolded | `clip.sh` (drive times: `r2-clip-*-drive.txt`) |
| `r2-bench-panels.mp4` (off the repo; preview `r2-bench-panels-preview.mp4`) | 1280×720 nord, Full, 60 fps: the `--bench-panels` scene (folds and unfolds over a 400-turn transcript, then the draft set to three lines and cleared) | `composer.sh` |
| `r2-chips.mp4` (off the repo), `r2-chips.json` | 1280×720 nord, Full, 60 fps: `SYNAPS_BENCH_CHIPS=3`, steps changing every 0.4 s (2 to 55 characters), clocks ticking, the pointer on a chip for 0.5 s, then one worker done and one failed; the JSON has every chip's bounds after each change | `chipclip.sh` |
| `stills/chips-{nord,paper}-{1920x1080,1280x720}-{1,3,6}.png` | the chips with 1, 3 and 6 workers, 3.5 s into the scene | `chips.sh` |
| `sheets/*.png` | frame sheets cut from the clips (frame numbers in the names) | below |

## What the clips show

**The transcript scrollbar through a panel flight (must-fix 1, Shady H1).** `thumb-trace.txt` traces the thumb in
column 976 over f450–f720 (the rail's fold and unfold, `thumb.py`); `thumb-trace-right.txt` follows it as it rides the
right panel's edge over f600–f760 (`thumb2.py`). At rest it is 424–546.
- Rail fold, Full: the thumb holds 424–546 through the flight. It changes in the landing frame (f502: 382, the rows
  re-wrapped at 1280 and not yet measured) and the next (f503: 423, measured). Unfold: f552 384, f553 424.
- Right panel, Full: 424–546 in every frame of both flights while its x rides from 974 to 1238 and back. Landed: 423
  (f662); the unfold's first frame 384 (f713), then 424.
- Reduced and Off: the same settled thumbs (folded 423, open 424). Off's rail fold goes straight to 423; Reduced's has
  the one 382 frame (f471), as Full's. (Corrected in r3, from the re-gate: the right panel at Off has the one
  in-between frame both ways too, f632 382 → f633 423 and f713 384 → f714 424.)
- r1 strobed by 34 px every 1–3 frames through each flight. What is left is the landing re-wrap: one frame on estimated
  heights before the next frame draws the measured ones (`AppView` asks for that frame when the column changes).

**FAVOURITES (E2).** `sheets/full-favs-fold-f203-236.png`, `full-favs-unfold-f281-314.png`: the chevron turns on
`quick` over f203–f212 and back over f281–f290. The demo's only favourite is the session that NEEDS YOU, which a folded
FAVOURITES keeps showing, so no body folds here; a section's body folding is NAMED's below (and the tests).

**NAMED, mid-rail, the rail scrolled (must-fixes 2 and 3).** `sheets/full-named-fold-f1275-1305.png` and
`full-named-unfold-f1359-1389.png` (every second frame). Folding: the rows below rise over NAMED's body, which stays
drawn under them; no band opens, and the rows above NAMED hold while it does (f1277–f1292). (Corrected in r3, from
the re-gate: the rail is scrolled to its end, so its offset then gives way to the shorter rail as the fold's held
length goes, and the rows above glide down 15 px over f1293–f1305: a second beat, smooth.) Unfolding: from the first frame TODAY and what
follows ride down whole to the rail's foot; no frame drops the lower half.

**The composer (E1).** `sheets/bench-composer-grows-f402-417.png`: the draft's last line stays at one y while the box
grows upward over f403–f414 (`flick`), the transcript above it moving up with it; `bench-composer-shrinks-f510-525.png`
back.

**The stub and Ctrl+J (S1, S2).** `sheets/full-stub-opens-f899-922.png`: the NEEDS YOU stub comes up with its top edge
drawn above its text in every frame it shows. `full-ctrlj-f1038-1061.png`: after Ctrl+J the card goes to its place one
way: its top border (column 700) reads 515, 502, 479, 442, … 377, 374, 372, 370, 369, 368 over f1042–f1063, only
up, then rests.

**The chips (Haseeb's item).** `r2-chips.json`: over 21 changes (steps, clocks, one done and one failed) every chip
stays 308 × 24 px at one of three places. The only change is the reorder when the two end (an ended worker goes after
the running ones, by need, as before), and that moves chips by whole cells. (Corrected in r3: at rest. Mid-slide the x spring stopped
half a cell short and jumped the rest, f381–f384; see `../r3/`.) `sheets/chips-steps-f240-520.png` shows
frames 240–520 of the clip.
