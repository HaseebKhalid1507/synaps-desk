# F1 fluid motion: live evidence

Recorded live against the **desktopproto sandbox** daemon on bella (`--profile desktopproto`,
claude-haiku-4-5), with `scripts/capture-linux.sh` (headless gamescope 144 Hz, `MESA_VK_WSI_DEBUG=sw`,
lossless x11grab at 30 fps), driven by `xdotool` (a new session, a typed prompt, the wheel, Ctrl+T and
Ctrl+1). Every clip is lossless (`libx264rgb -qp 0`), cropped to the column. 30 fps samples a 144 Hz app,
so the clips show where things are, not how smooth they are: the smoothness numbers are in
`docs/bench/fluid1/README.md`.

| file | what it shows |
|---|---|
| `live-send-arrivals.mp4` | Your message lands (its block reveals, no lift), then the agent's thinking row arrives: its height reveals 0 → its own on `snap`, clipped, its content rising 8 px and lifting from 70 %. |
| `live-arrival-reveal-frames.png` | 12 consecutive frames (33 ms apart) of that arrival: the thinking row opening under your block. |
| `live-stream-tail-follow.mp4` | 30 lines streamed while following: words come in at 70 % and brighten, each wrapped line's height glides in, and the end of content stays at the bottom of the view on every frame. |
| `live-wheel-spin.mp4` | Real X11 wheel input (`xdotool click 4/5`: 3 lines a notch): a spin up is one glide, single notches step on `wheel` (91 ms to 95 %), a spin down lands at the end and re-follows. |
| `live-tab-open-and-back-layer.mp4` | Ctrl+T opens a new session's tab and Ctrl+1 goes back: each column arrives as one layer (12 px from the tab's side on `glide`, lifting from 70 %). |

Not recorded: a touchpad fling and its coast. Headless gamescope has no touchpad, and XTEST can't send
X11's fractional scroll valuators. The coast is covered by `scroll::tests` (X11-shaped packets at 125 Hz
against 144 Hz frames).
