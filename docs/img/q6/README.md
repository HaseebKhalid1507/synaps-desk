# Q6 palette: evidence

Recorded on bella with `scripts/capture-linux.sh` (a headless gamescope at 144 Hz, x11grab lossless,
`MESA_VK_WSI_DEBUG=sw`), the `--offline` demo, settings `{"text_size": 16, "theme": <theme>}`. The binaries:

- **main** = `0a78bb7` release, built the same way (`dbuild synaps-desktop-q6 build --release --locked -p synaps-desktop`
  at that commit), sha256 `ccea70a2…48bbe`;
- **Q6** = this branch's release build, sha256 `20f3017f…551080b2`, installed as
  `bella:~/.local/bin/synaps-desktop-hybrid.q6` (`bin/q6-ev`).

The scripts are here: `shot.sh` (one frame: frame 45 of a 2 s capture), `ev.sh` (the side by sides), `clip.sh` (the
keyboard drive), `sheets.sh` (the frame sheets), `bench.sh`, `idlepal.sh` (`docs/bench/q6/`); `all.sh` runs them in that order.

## Side by side with main (`side-goto-<theme>-<size>.png`: main left, Q6 right)

Q6 opened with `--palette goto` (the same as Ctrl+P at launch), nothing else touched; nord, night-city, default and
paper at 1920×1080 and 1280×720. `side-cmds-nord-1920x1080.png` and `side-cmds-paper-1280x720.png`: `--palette
commands` (Ctrl+K).

**At rest (no palette) Q6 is main:** the same frame of both, pixel difference (ImageMagick `compare -metric AE`):

| | 1920×1080 | 1280×720 |
|---|---|---|
| nord | 0 | 8 px |
| night-city | 15 px | 8 px |
| default | 9 px | 0 |
| paper | 6 px | 8 px |

Every non-zero pixel is a running glyph's braille dot (the beat's phase at frame 45): at 1280 the rail row's glyph
(24,395) and the composer foot's (351,568); at 1920 the foot's (1279,720). An earlier run of the same script had other
counts at other sizes (nord 1280: 15, default 1920: 7), the same glyphs.

What the shots show (C1–C14, polish.md §1):
- the box is `overlay::frame` (card, 1 px `frame`, square, no shadow, no scrim): C8;
- 640 wide (1920) and the centre pane less 48 (1280: 624), its top 72 px under the window's, centred on the centre pane;
- the mode control is two segments in Display caps (`G O   T O` · `C O M M A N D S`, `d.s`), the selected one on
  `card_hi` with the 2 px `you_bar` on top; `Tab` (the key that flips it) at the right in `m.meta`;
- the query field: Plex Mono 13 `ink`, the `Fold` chevron in `you` leading it; the placeholder names both modes;
- group heads in `d.s` caps with the JP sub-labels: `S E S S I O N S セッション`, `W O R K E R S ワーカー`, `H I S T O R Y 履歴`,
  `C O M M A N D S コマンド` (the JP face re-subset for ワーカー and コマンド: C3), and `V I E W S` (Settings, Usage) without one;
- rows 32 tall: the state glyph (sessions and workers, spinning on the beat: C7, C10), the title `prose_k` in `s.ui` 14,
  the meta `m.meta` `dim_k` (`~/folder · age · $cost`), the key hint in `m.meta` at the right (an open tab's
  `Ctrl+N`, an action's key, `owner` on an engine command);
- the selected row: `card_hi` and a 2 px `you_bar` on its left edge (C5), one quad.

## Motion: the same keyboard drive at Full, Reduced and Off (`palette-<level>.mp4`)

1280×720 nord, 60 fps, 16 s, lossless: `libx264rgb -qp 0`, High 4:4:4 Predictive, `gbrp` (the header says `qp=0`).
The packed mp4's frames are the capture's, byte for byte (framemd5 of every frame: full `c0d9c18c8001`, reduced
`756e4ff63810`, off `2acee848a100`, the same for `.mkv` and `.mp4`).

The drive (`clip-<level>-drive.log`, seconds from `t0`; Ctrl+K at `t0` shows at f88, so frame ≈ 88 + 60 t):
Ctrl+K; ↓ ↓ ↓ ↑; types `fold`, then deletes it; Tab (GO TO); ↓ ↓; types `ledger`; Esc; Ctrl+P; Esc.

| sheet | what |
|---|---|
| `full-open-f85-90.png` | Ctrl+K: f87 is the session, f88 the palette whole (no enter: keyboard-opened is instant) |
| `full-quad-f161-174.png` | ↓: the selection quad (fill + bar, one quad) springs y on `snap`: changed frames f163–171 (150 ms) |
| `full-height-f333-361.png` | typing `fold` (a letter every 120 ms): the list's height springs on `snap` to what each letter leaves; changed frames f335–359 without a gap |
| `reduced-quad-f160-165.png`, `off-quad-f160-165.png` | the same ↓ at Reduced and Off: the quad is on the next row in one frame (f162) |
| `reduced-height-f333-338.png`, `off-height-f333-338.png` | the same typing at Reduced and Off: each letter is one changed frame (f335, f342, f349, f356) |

Changed frames per clip (framemd5 of consecutive frames): at Full, the open is one frame (f88), each ↓ 9 frames, the
typing its springs; the steady changes every 7–8 frames outside them are the demo's running glyphs on the 8 Hz beat.
At Reduced and Off (no spinners: the glyphs are still), only the keys change frames: 46 changed frames in Off's 960,
54 in Reduced's (Reduced keeps colour fades ≤ `quick`).
