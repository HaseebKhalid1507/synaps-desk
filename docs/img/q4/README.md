# Q4 chrome motion: evidence

> **Media:** the screenshots and clips named here are in the private repo [synaps-desk-evidence](https://github.com/HaseebKhalid1507/synaps-desk-evidence/tree/main/docs/img/q4), at the same paths. See [EVIDENCE.md](../../EVIDENCE.md).

Recorded on bella with `scripts/capture-linux.sh` (a headless gamescope at 144 Hz, 1920×1080, x11grab
lossless at 30 fps, `MESA_VK_WSI_DEBUG=sw`), driven by xdotool. The binaries:

- **main** = `c54ebd8` release (`dbuild synaps-desktop-q4 build --release` at that commit);
- **Q4** = this branch's release build, installed as `bella:~/.local/bin/synaps-desktop-hybrid.q4`.

The `*-drive.log` files give each step's wall time (the clip starts about 1 s before `t0`: frame ≈ 30 × (t − t0 + 1.0); corrected in r2, it said 3.5 s).

## Side by side with main (`side-<theme>-<size>.png`: main left, Q4 right)

The `--offline` demo, frame 45 of a 2 s capture, nothing touched. Themes nord, night-city, default and
paper; 1920×1080 and 1280×720. Pixel diff, main vs Q4:

| | 1920×1080 | 1280×720 |
|---|---|---|
| nord | 16 px (the running glyph's scanline: the beat's phase) | 0 |
| night-city | 0 | 0 |
| default | 0 | 16 px (the same scanline) |
| paper | 0 | 0 |

At rest nothing changed: Q4 is motion only.

## Motion at Full, and the same drive at Reduced and Off

`full-chrome.mp4`, `reduced-chrome.mp4`, `off-chrome.mp4` (24 s each, the same xdotool drive). Frame
sheets, frame numbers in the corner (30 fps):

| sheet | what |
|---|---|
| `*-rail-hover.png` | the pointer down the rail: hover on in 60 ms, off over 160 (Off: on and off in one frame) |
| `*-rail-press.png` | a press held on a rail row (one step past hover, 1 px down), let go off it (springs back, no click) |
| `*-switch-rail.png` | a session switch from the rail: the selection quad springs (y, `snap`) |
| `*-switch-meter.png` | its right pane: the new numbers afterglow, the context meter's segments light in order |
| `*-tab-back.png` | back by the rail: the strip's indicator (fill + bar, one quad pair) springs x and width |
| `*-tab-open.png` | a middle click opens a tab: its width springs 0 → w (clipped), the indicator follows |
| `*-tab-x.png` | its × closes it: the width springs back to 0 |
| `*-menu.png` | a rail row's menu: rise 6 px + strength over `quick`, Esc: 90 ms, 2 px back, no hitbox |
| `*-tracer-strip.png`, `full-tracer-line.png` | a worker opened in a tab from its row: the 2 × 24 px `you_bar` line travels to the tab's top edge (`base`), then the tab's bar draws |
| `*-scrollbar.png` | wheel notches, then still: the thumb comes up in 90 ms and fades out over 270 ms |

Reduced and Off end in the same frame as Full's last: the same tab, selection, menu and numbers,
reached at once.

`full-settings.mp4`: Settings opened, the text size stepped up and down by keys (the segment's bar
springs to the new segment; the saved value afterglows after the save).

## The link, live on the sandbox daemon (`live-link-drop-and-back.mp4`)

`synaps daemon start --profile desktopproto` (the sandbox profile, no auth lines: no model calls), the
app attached with `--profile desktopproto`; the drive stops the daemon and starts it again.
`live-link-topbar-frames.png` is the top bar at frames 25–260:

- frame 31: the drop (`reconnecting ·`, its disc at rest). **Corrected in r2:** the tear *is* in this
  clip, at frames 30–32 (the finish gate decoded them); this note said it fell between captures. r2's
  live clip shows it again full-window (`r2/link-tear.png`), and r2 no longer tears at the first
  frame of a drop: a non-reload drop tears only after a 1 s grace (H1).
- frame 35: the client gives up (`connect …` in `err_ch`, the disc `err_mark_ch`); frames 35–45: both
  drain to the grey of their own luminance over 360 ms and hold while the daemon is down.
- frame 218: the daemon is back. `live-link-scan-frames.png` (frames 214–225): the Scan's 1 px line and
  its trail run down the window over 600 ms over the drawn UI; the palette restores.
