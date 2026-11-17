# F1 fluid motion: A/B on the real i3 panel (bella)

`scripts/bench-linux.sh` on bella's logged-in i3/X11 session (DP-3, 143.86 Hz, the window floated by the
script), runs alternating main → F1 → main → F1, then `--bench-motion` for each. `--refresh-hz 143.86`.
Load average during the runs: 7.4–7.9 (other builds on bella; both binaries saw the same).

- **main** = `~/.local/bin/synaps-desktop-hybrid.motion`, built from motion PR 4's head `e9f0a61`,
  whose tree is identical to main `8e69eda` (`git diff e9f0a61 8e69eda` is empty). Release profile.
- **F1** = `fluid/f1` release build (`dbuild synaps-desktop-f1 build --release`).

| | main r1 | main r2 | F1 r1 | F1 r2 |
|---|---|---|---|---|
| streaming (200 deltas/s, 5 s): fps | 142.4 | 142.9 | 141.5 | 142.9 |
| streaming: dropped frames | 11 | 7 | 13 | 6 |
| streaming: draw p50 / p99 ms | 2.22 / 5.83 | 2.31 / 4.70 | 2.23 / 5.25 | 2.60 / 4.76 |
| scroll (bench `scroll_by`, 2,000 messages): draw p50 / p99 ms | 1.82 / 3.32 | 1.60 / 3.25 | 1.82 / 3.49 | 1.66 / 3.11 |
| RSS idle / after stream / end (MiB) | 260 / 308 / 319 | 202 / 254 / 265 | 261 / 308 / 320 | 202 / 253 / 266 |

**Wheel spin** (F1 bench `spin_*`: a 3-line notch every 25 ms for 2.5 s over the 2,000-message transcript,
dispatched through the window's own input path). `spin_reduced` is GPUI's own wheel, the path main uses.

| | F1 r1 Full | F1 r2 Full | F1 r1 Reduced (= main's wheel) | F1 r2 Reduced |
|---|---|---|---|---|
| fps | 143.9 | 144.0 | 39.5 (a frame per notch) | 39.5 |
| dropped | 0 | 0 | n/a (frames only on input) | n/a |
| draw p50 / p95 / p99 / max ms | 1.84 / 2.57 / 3.19 / 4.19 | 1.64 / 2.26 / 2.93 / 3.44 | 2.19 / 3.53 / 3.79 / 4.29 | 2.06 / 3.15 / 3.40 / 3.73 |
| present interval p50 / p99 / max ms | 6.93 / 8.42 / 8.83 | 6.90 / 8.18 / 8.44 | 27.4 / 29.5 / 30.5 | 27.5 / 29.1 / 29.1 |
| travel | 6000 px (exact) | 6000 px | 6000 px | 6000 px |

**Idle and the beat** (`--bench-motion`):

| | main | F1 |
|---|---|---|
| idle 10 s: presents / app frames | 0 / 0 | 0 / 0 |
| idle CPU ms/s | 2.11 | 2.18 |
| beat (a tool runs, 6 s): presents / app frames / CPU ms/s | 48 / 0 / 11.8 | 48 / 0 / 14.1 |
| 25 glyphs scanning: presents / CPU ms/s | 48 / 12.8 | 48 / 12.8 |
| Motion Off: presents | 0 | 0 |

**Binary size:** main 37,438,104 B, F1 37,474,712 B (+36.6 KB, +0.1 %).

## Round 2 (review fixes, `fec9800`)

Same script and binaries' roles (main = the same `synaps-desktop-hybrid.motion`), load average 2.4–3.1.
One round on DP-3 (143.86 Hz), then two rounds with the window on **workspace 2, the laptop's own
240 Hz panel (eDP-1-1)**, `BENCH_WORKSPACE=2 --refresh-hz 240`. Raw: `r2-*.json`.

| | main 144 | F1 r2 144 | main eDP (2 runs) | F1 r2 eDP (2 runs) |
|---|---|---|---|---|
| streaming fps | 143.1 | 143.3 | 143.1, 143.3 | 142.7, 142.7 |
| streaming draw p50 / p99 ms | 1.50 / 2.40 | 1.67 / 2.59 | 1.63 / 2.58, 1.85 / 2.88 | 1.71 / 2.82, 1.74 / 3.20 |
| scroll draw p50 / p99 ms | 1.18 / 2.27 | 1.16 / 2.24 | 1.17 / 2.27, 1.41 / 3.44 | 1.14 / 2.25, 1.13 / 2.15 |
| wheel spin (Full) fps, dropped | — | 143.9, 0 | — | 144.1, 143.6 |
| wheel spin draw p50 / p99 ms | (Reduced: 1.31 / 2.46) | 1.13 / 2.16 | — | 1.32 / 2.41, 1.16 / 2.12 |
| RSS idle / end (MiB) | 200 / 259 | 202 / 263 | 202 / 261, 202 / 262 | 201 / 262, 202 / 263 |

**240 Hz: not reachable on bella's i3/X11 session.** The window really was on the 240 Hz eDP panel
(workspace 2), but every run presents at 6.95 ms (143.86 Hz), main and F1 alike. Under X11 with no
compositor, vsync follows the primary output (DP-3, 143.86 Hz), and bella's eDP is the iGPU's
reverse-PRIME sink (`eDP-1-1`). So the "dropped" counts against 240 in the raw JSON mean nothing.
A real 240 Hz run needs the laptop panel as primary or alone (or Wayland). I didn't change
Haseeb's xrandr layout. F1's draw p99 everywhere (≤ 3.2 ms) fits a 240 Hz frame (4.17 ms).

Not measured: a real touchpad. bella has one (`CUST0001 04F3:31FD`), but nobody can use it remotely,
and headless gamescope can't send X11's fractional scroll valuators. The coast is covered by
`scroll::tests` with X11-shaped packets. bella's mouse (Logitech G102, `hid-generic`, no Resolution
Multiplier in its HID descriptor, valuator increment 120) is a plain wheel: 3.0 lines a detent.
