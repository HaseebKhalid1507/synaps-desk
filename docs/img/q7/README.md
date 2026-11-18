# Q7 Sticky: evidence

Recorded on bella from the `--offline` demo (headless gamescope at 144 Hz, x11grab, `MESA_VK_WSI_DEBUG=sw`), driven
by `evidence.sh` (here). Binaries are release builds made through `dbuild` from the same tree:
- **Q7:** `3e2093d`, sha256 `5f88fb61…` (installed as `bella:~/.local/bin/synaps-desktop-hybrid.q7`);
- **main:** `0a78bb7`, sha256 `2bf0106a…`.

## Side by side at rest (`side-<theme>-<size>.png`: main left, Q7 right)

These are frame 45 of a 2 s capture, untouched, with the demo at its end (following). The pixel diff between main and Q7,
as connected regions (12 px join):

| | 1920×1080 (labels hang) | 1280×720 (labels inline) |
|---|---|---|
| nord | 397 px: the pinned `AGENT` (565,106 46×9) and its `04:12` (576,130 36×8) | 16,420 px: the band (336,86 624×27) |
| night-city | 394 px: the same two | 16,485 px: the band, plus a 7×8 box and two 2×2 dots |
| default | 396 px: the same two | 16,432 px: the band, plus two 2×2 dots |
| paper | 405 px: the same two, plus two 2×2 dots | 16,433 px: the band, plus two 2×2 dots |

- **1920:** the agent's turn is taller than the view, so its label and time sit in the gutter at the view's top + 12 px.
  Nothing else differs, apart from the 2×2 dots.
- **1280:** the pane is narrower than 872 px, so the labels are inline. The `AGENT … 04:12` line sits at the column's top on
  the canvas band, with the hair rule under it (F8). It covers the top 27 px of the column.
- **The 2×2 dots and the 7×8 box:** the braille spinners, caught in whichever frame each binary was on (as in Q3's
  pairs).

## Scrolled (`scrolled-nord-<size>.png`)

Six wheel notches up at Reduced, so both binaries are at the same scroll position (frame 105 of 4 s).
- 1920: 397 px, the same label and time.
- 1280 and 1024: 5,433 px and 5,481 px, the band (336,86 624×27 and 632×27).

## Clips (lossless: `libx264rgb -qp 0`, `gbrp`, High 4:4:4 Predictive; each file's per-frame md5 equals its capture's)

| file | level | size | fps | frames | frame md5 (mkv = mp4) |
|---|---|---|---|---|---|
| `sticky-full-1920.mp4` | Full | 1920×1080 | 30 | 480 | `eb53c747aefc` |
| `sticky-reduced-1920.mp4` | Reduced | 1920×1080 | 30 | 480 | `476056ea9663` |
| `sticky-off-1920.mp4` | Off | 1920×1080 | 30 | 480 | `17af218372a6` |
| `sticky-full-1280.mp4` | Full | 1280×720 | 30 | 480 | `ed35721767d6` |
| `sticky-full-1024.mp4` | Full | 1024×720 | 30 | 480 | `c44249f64658` |

The script, the same in each clip: 24 wheel notches up, one every 0.35 s, then 12 down.

**Caveat: the theme.** The first run of `evidence.sh` passed a relative `XDG_CONFIG_HOME`. The app ignores a relative
path, so the clips read bella's own `~/.config/synaps-desktop/settings.json` (night-city, right panel collapsed). That
run also rewrote that file once at 00:54:01; see the PR. The side-by-side shots above are from the fixed script (an
absolute path, each run's own settings).
- With the right panel collapsed, 1280 hangs the labels. 1024 is the inline case (F8's band).

**What the clips show:**
- `full-1920-f180-224.png` (every 4th frame, the gutter and the column's left):
  - `AGENT` and `04:12` stay at the pin while rows scroll under them;
  - where two messages meet (third row), the label above is partly out at the top, its time's bottom touching the next
    `AGENT`.
- `full-1024-every-40th.png`: the inline band at the column's top (`AGENT`, then `YOU`), the time at the right and the
  rule under it.
- **Reduced = Off, pixel for pixel, in the gutter.** Over the gutter strip, the Reduced and Off clips go through the
  same 23 distinct states in the same order. With the column included, they differ in 1 of 24 states: a spinner frame.
- Full glides the wheel (F1). The label moves with the list on every frame, with nothing of its own.
