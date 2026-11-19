# Q7 r2 evidence (Shady #19 M1/M2, finish gate #19 must-fix 1)

> **Media:** the screenshots and clips named here are in the private repo [synaps-desk-evidence](https://github.com/HaseebKhalid1507/synaps-desk-evidence/tree/main/docs/img/q7/r2), at the same paths. See [EVIDENCE.md](../../../EVIDENCE.md).

Recorded on bella from the `--offline` demo (headless gamescope at 144 Hz, x11grab, `MESA_VK_WSI_DEBUG=sw`), nord, a fresh
`settings.json` under an **absolute** `XDG_CONFIG_HOME` in `~/.cache/q7r2` for every run (the right pane open, as main's
default, so 1280 is inline: F8's band). bella's real `~/.config/synaps-desktop/settings.json` was checked before and
after each script: unchanged (mtime 00:54:01, sha256 `9c379f92…`).

Binaries: **r2** `7bdac0c`, sha256 `875c8e0b…` (installed as `bella:~/.local/bin/synaps-desktop-hybrid.q7`); **r1**
`3e2093d`, sha256 `5f88fb61…`.

## A running row parked under the band, Full, 1280×720 (`evidence.sh`, `park-check.py`)
- `park-full-1280-r1.mp4` and `park-full-1280-r2.mp4`: the same drive on both, one wheel notch up every 2.4 s from the
  tail to the top, so every scroll position is held for over 2 s. Lossless: `libx264rgb -qp 0`, `gbrp`, High 4:4:4
  Predictive, 30 fps, 2,040 frames; each file's per-frame md5 equals its capture's (r1 `e929cbeba5a0`, r2 `eba785c00615`).
- `park-check.py` counts frames where the band's rectangle (x 336–959, y 86–112) changes while the list is parked
  (under 400 px changed below it):
  - **r1: 19 frames,** f1327–1395 (2.3 s parked): a running row's spinner beats inside the band, 4–8 px a beat.
  - **r2: 0 frames** in the whole clip.
- `park-f1330-r1-over-r2.png` (2× zoom, r1 above, r2 below): frames 1330, 1360 and 1390 differ between r1 and r2 in
  exactly 24 px, all inside the band (x 339–344, y 95–102, the spinner's dots on the `A` of `AGENT`), and in none

## The authorize card under the band (`card-under-band-1280x440.png`)
- 1280×440: the card is taller than the view, so at the tail its top is under the band.
- The card's trace runs at the column's left (x 336) from y 101, under the rule. Beside the band (x 320–335 and
  960–975, y 74–100) every pixel is canvas: no stroke pokes out.

## A day line pushing a pinned label (`day2.sh`, `day-line-pushes-1920x536.png`)
- The idle session (`--session 20261113-161706-a1b9`: its last turn the next morning, under `Sat Nov 14`), labels
  hanging. The day line has pushed the pinned `AGENT 16:25` up and partly out of the top; the label sits right above
  it.
- Inline, the band is pushed the same way (unit test `a_day_line_ends_the_message_above`). In the Full glide at
  1280×440 the band crossed the push range between two 60 fps frames, so there is no inline still.
