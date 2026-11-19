# F1 fluid motion: live evidence

> **Media:** the screenshots and clips named here are in the private repo [synaps-desk-evidence](https://github.com/HaseebKhalid1507/synaps-desk-evidence/tree/main/docs/img/fluid1), at the same paths. See [EVIDENCE.md](../../EVIDENCE.md).

All clips were recorded live against the **desktopproto sandbox** daemon on bella (`--profile desktopproto`, claude-haiku-4-5).
- Capture: `scripts/capture-linux.sh` (headless gamescope at 144 Hz, `MESA_VK_WSI_DEBUG=sw`, lossless x11grab at 30 fps).
- Input: `xdotool`.
- Encoding: every clip is lossless (`libx264rgb -qp 0`) and cropped to the column.

30 fps samples a 144 Hz app, so the clips show where things are, not how smooth they are. The smoothness numbers are in `docs/bench/fluid1/README.md`.

## Round 2 (review fixes; binary `fec9800`)

One take, driven by `xdotool`:
1. a new session;
2. a prompt for 30 numbered facts;
3. a follow-up send;
4. Ctrl+T;
5. Ctrl+1;
6. Ctrl+2 and Ctrl+1, 90 ms apart.

| file | what it shows |
|---|---|
| `live-r2-send-arrivals.mp4`, `live-r2-arrival-frames.png` | The first send of a new session, then the reply arriving. The thinking row arrives with its `AGENT` label hung in the gutter. Then the prose row reveals on `snap` (t = 8.83 s, its first frame a sliver), with the write head already lit on its **shown** bottom, and riding it down every frame of the reveal (8.87, 8.90, 8.93 s…). **Your first message lands at rest:** a session's first message is not a live splice (see below). |
| `live-r2-stream-write-head.mp4` | 30 lines streamed while following. The write head (gutter, x ≈ 632 in the capture) is lit on **every** frame from the first delta (t = 9.43 s) to the last (16.60 s): 216 consecutive frames by a per-frame scan of the gutter column. It rides the row's shown bottom as each new line glides in, then decays after the stream. (Round 1: dark on every streaming frame.) |
| `live-r2-write-head-frames.png` | Eight stills from that stream, 1 s apart: the head beside the newest line every time. |
| `live-r2-follow-up-send.mp4`, `live-r2-follow-up-frames.png` | The follow-up send. Your block reveals over f03–f05 (~100 ms) with `YOU` beside it, whole, in its first revealed frame (f03). Then the reply: its `AGENT` label and the head appear with the row's first revealed frame (f16), not at unwrap. |
| `live-r2-tab-ctrl1-layer-and-rapid-back.mp4`, `live-r2-ctrl1-frames.png` | Ctrl+T, then Ctrl+1 back to the first session's tab, then Ctrl+2 and Ctrl+1 90 ms apart. Details below. |
| `live-wheel-spin.mp4` | (Round 1, unchanged path.) Real X11 wheel input (`xdotool click 4/5`: 3 lines a notch). A spin up is one glide, single notches step on `wheel`, and a spin down lands at the end and re-follows. |

**Ctrl+1, measured frame by frame** (best-fit horizontal shift against the settled frame, residual in grey levels):
- **The first frame that shows tab 1's rows is already the layer:** shifted −12 px, at 0.87 of the settled ink.
- Then −8, −6, −4, −2, −1, −1 and 0 px, every frame a pure move (residual ≤ 0.13).
- No earlier frame shows the column at rest (round 1 showed it at rest for 4 frames, then knocked it 12 px).
- **The switch away and straight back** (Ctrl+2 then Ctrl+1, 90 ms apart, t ≈ 38.5 s) is drawn with **no layer**. The frame after the return matches the settled column at shift 0 (residual 0.00), and nothing slides.

**Not recorded:**
- A touchpad fling and its coast. bella has a touchpad (`CUST0001 04F3:31FD`), but nobody can use it remotely, and headless gamescope can't send X11's fractional scroll valuators. The coast is covered by `scroll::tests` (X11-shaped packets at 125 Hz against 144 Hz frames).
- A hi-res wheel. bella's mouse (Logitech G102 on `hid-generic`) has no Resolution Multiplier in its HID descriptor, so it sends 120 units (3.0 lines) a detent. It's covered by `a_hi_res_wheel_never_coasts`.

**A session's first message** isn't a live splice (`app.rs` `sync_list`: `sp.start > 0`), so it lands at rest. An empty transcript's first splice can't be told from that session's history load without a marker the client doesn't have. Round 1's note ("your message lands (its block reveals)") was wrong for this case. A follow-up message does reveal (above). The flight from the composer (F2) will cover the first message too.
