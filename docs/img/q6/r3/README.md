# Q6 r3: evidence

**Binary:** head `aaf73ad`, release (`dbuild synaps-desktop-q6 build --release --locked -p synaps-desktop`), sha256
`9381daf8…aedc6b184d`, 38,523,624 bytes, installed as `bella:~/.local/bin/synaps-desktop-hybrid.q6`. Every still and
the clip here are from that binary.

**Config safety:** every app run's settings folder was a scratch folder under `~/.cache/q6r3` (`capture-linux.sh`'s
absolute `$out/config`). The real `~/.config/synaps-desktop/settings.json` kept its mtime (00:54:01) through every run
(`live.log`). The sandbox config (`desktopproto`) got two `auth.` lines and `client_input = exclusive` for the live run
and was restored: sha256 `c1b7ac41…` before and after. The sandbox daemon was stopped after.

Scripts: `design.sh`/`designs.sh` (offline stills), `clip.sh` (the 60 fps clip), `live.sh` (the sandbox run),
`tip.sh` (the note's tooltip), `all.sh` (the first three in order). `*-drive.log`: each key's time.

## Shady's re-review (N1–N8)

| item | still | what it shows |
|---|---|---|
| N1 | `live-midturn-palette.png`, `crop-midturn-palette-1280.png` | Live, mid-turn: Ctrl+K `workers`. The seven engine rows are dim, and their meta reads `not while a turn runs` |
| N1 | `live-midturn-refused.png` | Enter on it: nothing sent; the note `/workers: not while a turn runs` (the composer's note too) |
| N3 | `live-model-bare.png`, `live-workers-cancel-scout.png` | A bare `/model` and `/workers cancel scout`: the usage line, the text kept, no turn |
| N2 | `live-slash-escaped.png`, `crop-slash-escaped-context-9-lines.png` | `//etc/hosts: what is it?` went as the message `/etc/hosts: what is it?…` and the agent answered it |
| N4 | `live-wide-workers.png`, `crop-synaps-label-1920.png` | 1920, labels hanging: each answer's well carries `S Y N A P S` (dim). The agent's `A G E N T` stays on its own row |
| N4 | (test) | No still here shows an answer as the top message: the live scroll stills (not kept) stopped on the compaction summary, under `Y O U`. The pinned label with an answer on top is covered only by `a_command_answer_pins_its_own_label`: it pins its own label at the pin, never the agent's above it |
| N6 | `live-context.png` | `/context`'s 9 lines show whole (`9 lines`), with no fold |
| N7 | `live-compact-asked.png` | `/compact: asked · see the transcript`, whole at 1280 |

## Design re-gate (403b659)

| # | evidence | measured |
|---|---|---|
| 1 | `r3-{nord,paper}-{1280,1920}-goto-open.png` (the demo's chip strip showing) | The box frame runs 96–532 at 1280 and the chip starts at 557: 24 px of canvas, no chip pixels under the box. At 1920 it runs 96–610 and the next thing below (the composer) starts at 628, a 17 px gap. The same in nord and paper |
| 2 | `r3-nord-1280-cmds-led.png`, `crop-nord-1280-nothing-matches.png` | A miss shows `Nothing matches` in `dim_k`. The list is one row (32 px) |
| 3 | `crop-notes-1280.png` | At 1280, every note is whole: model-bare, workers-cancel-scout, midturn-refused, compact-sent, compact-asked, compact-after and observe-refused |
| 3 | `live-compact-after.png` | After `conversation compacted`, the `/compact` note is gone |
| 3 | `live-observe-refused.png` | Observing: `/compact: another client has the input` |
| 3 | `tip-note.png`, `tip-hover-700.png`, `crop-note-tooltip-1280.png` | The pointer on the note shows the house tooltip with the whole text |
| 3 | (optional R2-O2) | An answered or sent note is in `dim_u`. Refused and not-run notes stay `warn` (compare `live-workers` with `live-midturn-refused`) |
| 4 | `roles_check.txt` (Q6 section), `r3-paper-1280-cmds-f.png`, `crop-paper-1280-cmds-f-you_k.png` | `you_k` is at least 5.15 on `card` and 4.50 on `card_hi` in all 140 theme × accent cases; raw `you` was 1.01 at its lowest. `thumb_k` is at least 1.96 on `card_hi` |
| 4 | `live-observe-palette.png`, `crop-observe-palette-plain-match-2x.png` | Observing, `compact` lights `/compact` in `prose_k`, not the accent |
| 5 | `palette-scroll-full.mp4`, `scroll-open-steps.png`, `clip-full-drive.log` | See below |

### The clip (`palette-scroll-full.mp4`)

- Recorded at Full, 1280×720 nord, 60 fps, lossless: H.264 High 4:4:4 Predictive, `-qp 0`, gbrp. The frame md5s of
  the `.mkv` capture and the `.mp4` match (`99c6366d9851`). 840 frames.
- The keys, at frame (time × 60):
  - Ctrl+P at f30, the process's first open;
  - ↓ ×12 at f92–f335;
  - PageDown at f387;
  - Esc at f448;
  - Ctrl+P at f509;
  - Esc at f601.
- **The box and the thumb appear on the same frame.** On both opens the box and the thumb (`thumb_k`, `#666a73`)
  first show on the same frame: f59 and f538. The thumb is whole from that frame, with no fade-in. (The capture's
  first painted frame comes about 29 frames after the key, in both opens. That is the headless compositor's latency;
  the box has it too.)
- **The thumb follows the list.** The thumb's rows, from the frame data:
  - f59–f296: 188–216 (no scroll yet; the selection is still in view);
  - then f297, f319, f341 and f363: 189, 192, 194 and 197. Each is one ↓ that scrolled, so the thumb moves on the
    frame the list scrolls;
  - f416: PageDown, 226–253.
- **One fix came out of the clip.** In the first take (`63a6796`), a reopen kept the list scrolled down by one row:
  `scroll_to_item(0)` scrolled to the selection quad, which is the list's first child. The SESSIONS head was hidden
  and the thumb sat at 190.
  - `aaf73ad` resets the list's offset to 0 on open, on a mode flip and on typing.
  - The take kept here shows the reopen at the top, with the thumb back at 188–216 (f538).
  - Pinned by `palette_reopens_at_the_top`.
