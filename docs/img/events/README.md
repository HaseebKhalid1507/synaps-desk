# Events in the desk: evidence

Live on the **desktopproto sandbox** on the build machine (daemon 0.11.1, haiku-4-5), `live.sh` phases `setup`, `order`,
`record`, `reload`, `teardown`. The sandbox config got the two model-credential lines for the run and was restored
after: sha `c1b7ac41…320edd` before and after, 0 auth lines left (`logs/live.log`). Every app run's
`XDG_CONFIG_HOME` was an absolute scratch folder under `~/.cache/events-live` (`scripts/capture-linux.sh`); the
machine's real `~/.config/synaps-desktop/settings.json` kept its mtime (`07:04:07.671`, before the first run) through every run.
Every event text, name and channel is synthetic. The scratch folder is gone.

Binaries: this branch's release builds (`dbuild synaps-desktop-events build --release --locked`): `record` ran
the first commit's (`09d3038`); `reload` the next one's, which adds the sticky-label fix (an event row ends the
agent's message above it, so its label is not pinned over the event), sha256 `8ed98b79…fcb7bea0`.

## The frame order (`logs/frames.log`, `frames.py`: an Observe attach logging every frame)

**Idle: an event wakes a turn** (`synaps send --severity high --channel alerts`):

```
seq 53 external id=755ce4e5-… source=cli severity=High expects_response=False
seq 54 conversation messages_len=3          ← the event's message injected (history append)
seq 55 turn_started trigger=event_auto user_text=none
seq 57 conversation …   seq 58 stream/llm/response_start … (deltas) … done … idle
```

No `steered` and no `steering_delivered`: the `External` frame, then the history append that holds its
`<event …>` message, then a `TurnStarted` with `trigger: event_auto` and no `user_text`.

**Mid-turn: an event while the agent runs a tool** (`bash sleep 12`; `synaps send --severity medium`):

```
seq 122 stream/llm/tool_use bash …
seq 124 external id=e90b4e49-… source=cli severity=Medium   ← at drain time, while the tool runs
seq 125 conversation messages_len=5                         ← unchanged: the event is steered, not injected
seq 127 stream/llm/tool_result …
seq 128 stream/agent/steering_delivered event-payload id=e90b4e49-…   ← right after the tool's result
seq 130 conversation messages_len=8                         ← tool_use, tool_result and the event's message
seq 131 stream/llm/response_start …
```

No `steered` frame for an event (that frame is only the user's own `Steer`). The engine delivers the event into
the turn after the running tool's result (`drain_steering`), as a `role=user` message of its form, and the next
history append carries it there.

So the desk's row: the `External` frame starts it at once (a note, at the end of the transcript); a
`SteeringDelivered` carrying the same id adds nothing; the history append that holds the message turns the note
into that message's row, at the message's place (same row id, its live time and the frame-only fields kept). For
an idle wake that is right above the turn's reply. **Round 2** (Shady #22 H1): that held only for the windows
above; an event that comes while the reply streams is now held last until delivered (below).

## Disclosure (spec §9.7)

What the daemon sends: `External { event }` is the engine's `Event` as drained, **unredacted**. `on_queue_wake`
and `note_in_main_conversation` (`session/actor.rs`) emit `SessionEventWire::External(event.clone())`, and
`wire.rs` mirrors it field for field; nothing on that path consults `content.disclosure`. The only gate is
`format_event_for_agent` (`events/format.rs`), the model-visibility boundary: `gate_for_model` with no consent and
no redactor, so `local_only`, `model_visible_after_redaction`, `model_visible_after_consent` and
`persist_never_transmit` become `[content withheld: <class> (<reason>)]` with no data; `model_visible`,
`never_persist` and none pass. The TUI (`stream_handler.rs on_external`) shows `content.text` raw from the frame,
whatever the class.

What the desk does: it shows no more than the TUI, and less for a withheld class. `ExternalEvent::shown` applies the
same gate as the agent's form: a withheld class's live row shows the same marker as its history row, and keeps no
data; a class this client doesn't know is withheld (closed, as the gate fails). Tests:
`events::tests::a_withheld_class_shows_its_marker_live_as_in_the_history`.

**Round 2, `local_only` (the owner's call on Shady #22 M3):** that class is defined as "display locally only, never
enters model context", so its live row shows its text (and data) with a quiet `not sent to the agent` mark. Its
history row can only show what the agent's form kept, the marker `[content withheld: local_only (local_only)]`. So
for this class a reload shows less than the live row did: the one mismatch between them, by design. Within the
session the row keeps the live text after its message lands (the same row). Every other withheld class
(`…_after_redaction`, `…_after_consent`, `persist_never_transmit`, unknown) shows the marker live too. Tests:
`events::tests::a_withheld_class_…` (client), `events::tests::a_local_only_event_shows_live_marked_and_the_marker_after_a_reload`
(desk); live: `r2/stills/stream-local.png` against `r2/stills/reload-1920x1080-nord.png` (`side-stream-reload.png`).

**`never_persist` (Shady #22 N4):** the engine shows it and keeps it nowhere (`DisplayOnly`: not in the history,
not in a buffer, not steered). Its live row is all there is: it stays a note, and a reload has none of it. A
session not attached never learns of it, so it marks no rail row. Test:
`a_never_persisted_event_keeps_its_live_row`.

## Stills and the clip

The repo has small lossy previews only (H.264 crf 23, the transcript column cropped at x 530). The lossless clips
and the captures they came from are kept off the repo, in the maintainer's `synaps-desktop/events-evidence/` folder,
with `MANIFEST.md` (their sha256 and frame md5s, and the preview commands): this repo is public, and a merged blob
stays in its history.

- `events-live-preview.mp4` (921 KB; 560 px wide, 8 fps): the whole `record` run, 150 s. Its lossless original
  (1920×1080, 30 fps, libx264rgb qp 0, gbrp) has frame md5 `ce9a02db17aa`, the same as the capture's mkv. Drive
  log with event times: `logs/live-1920-nord-drive.log`.
- `stills/live-*.png` (1920×1080, nord, live): `low` (send, quiet), `medium-channel` (`#deploys`), `ask` (a comms
  ask with `expects_response`: `COMMS kestrel #7 ask ASK`, the `↩ comms_send(…)` line gone), `high` (`#alerts`,
  warn), `critical` (`uptime-watch`, err, folded: `… 2 more lines · show all`), `midturn-running` /
  `midturn-event` (the row arrives under the running `bash`, 7 s in) / `midturn-after` (after the tool's result,
  the reply right under it), `side-mark` (an event to `ev-side`, not on screen: UNREAD on its rail row, the UNREAD
  chip at 1; `crop-side-mark.png`), `scrolled` / `scrolled-after` (scrolled up, an event lands: the transcript
  column's pixels are unchanged; only the scrollbar's thumb and the right panel's token counts changed).
- `stills/reload-<size>-<theme>-up*.png`: the desk relaunched on the same session (the history form), 1920 and
  1280, nord and paper, scrolled up to the events. `side-live-reload.png`: the live rows (left, `live-ask`) and the
  reloaded ones (right): the same rows. `side-midturn.png`: mid-turn, live then after the turn.
- The tab mark: `ev-side` was not open as a tab in the run; a tab reads the same `Model::fact` as the rail
  (`tabstrip.rs`), covered by `model::tests::an_event_on_a_session_not_on_screen_marks_it_unread`.

## Round 2: an event while the reply streams (Shady #22 H1)

The same sandbox and scripts (`live.sh setup`, `stream`, `reload`, `teardown`), this branch's release build at
`9e7ca7d`, sha256 `a52bf24c…7b0d45`. Config sha `c1b7ac41…320edd` before and after, 0 auth lines left
(`r2/logs/live.log`). `settings.json`'s mtime was `08:16:13.638` before the first app run of this round (it had
changed since round 1's `07:04:07` by something else on the machine: no run of mine was going) and the same after.

**The frame order** (`r2/logs/frames-stream.log`): a 40-line reply, two `synaps send`s while its text streams:

```
 9.846 seq 58 stream/llm/response_start
11.616   (20 thinking, 50 text deltas)
11.616 seq 129 external id=7a6c16d0-… severity=High      ← mid-text
13.820   (89 text deltas)
13.820 seq 220 external id=7400ed73-… severity=Medium    ← mid-text
18.942   (203 text deltas)
18.947 seq 426 stream/agent/steering_delivered event-payload id=7a6c16d0-…   ← the round's text done, no tool
18.947 seq 427 stream/agent/steering_delivered event-payload id=7400ed73-…
18.947 seq 429 conversation messages_len=6               ← the reply, then the two events' messages
19.503 seq 430 stream/llm/response_start                 ← a second round: the agent answers them
```

So the history has the whole reply, then the events, then the answer; round 1's desk had put each event row
where its frame came, between two halves of the reply. Now the row is held last: the reply streams on above it
(`r2/stills/stream-held-1.png`, `-2`, `-4`: one AGENT response, the events under it), the
`steering_delivered`s leave it there, and the second round goes under it (`stream-after.png`). Relaunched on the
session (the history form), the rows are the same, in the same order (`side-stream-reload.png`: live left,
reload right; the only difference is the `local_only` row, above).

`r2/events-stream-held-preview.mp4` (779 KB; 800×1080, 30 fps): 13 s from just after the prompt to the second
round's answer, the transcript column. Its lossless original (off the repo, above) has frame md5 `eeb77fa4f792`,
the same crop of the capture's mkv (66 s at 1920×1080). Drive log: `r2/logs/stream-1920-nord-drive.log`.

What the history form can't say: a text whose last line is `Data: {…}` or `Data: […]` (valid JSON) can't be told
from the event's data, by the agent either. Round 2 kept it as text on the live row only, so a reload lost that
line (Shady #22 M1′); round 3 reads it alike everywhere and shows it (below).

## Round 3: the data line, shown (Shady #22 M1′)

`ExternalEvent::history()` and `shown()` now read the agent's form with `from_payload`, the history fold's own
parser: a text whose last line is a JSON `Data:` line reads as its text plus that data, live, folded and
re-attached alike (one identity: no second row on a re-attach or a session switch). The desk draws an event's data
as the agent reads it: a quiet mono `Data: {…}` line under the text, folded past 160 characters with the text. The
form's 1000-character cut of a long data is no longer JSON, so it is a line of the text, as the agent reads it
(it was dropped before).

Live on the same sandbox (`live.sh setup`, `data`, `reload one`), this branch's release build at `623c33e`, sha256
`0867a71e…742bb2`: a comms fyi (no plugin trailer) ending in the peer's own `Data: {…}` line, and a watcher event
with real data (long: folded). `r3/stills/data-live-reload.png`: the two rows live (left) and after a relaunch on
the session (right), the same text and the same data line; the fyi's row crops are pixel-identical. Drive log:
`r3/logs/data-1920-nord-drive.log`.

The run (`r3/logs/live.log`): another desktopproto daemon, not this run's, already held the sandbox's lock, so the
setup's daemon did not start and the two sessions were made on it. Nothing else of it was touched: the two
sessions were closed after (`synaps daemon close`), and that daemon was left running (`live.sh` now refuses to set
up over a daemon it didn't start, and stops only its own). Its model calls failed for want of credentials (error
notices under the events, not in any history: the relaunch has none, and the still is cropped to the rows). Config
sha `9803b9f3…916b35` before and after (it had changed since round 2, not by this run), 0 auth lines left.
`settings.json`'s mtime moved at `09:13:15` during the run: written by a desk on the real config (its tabs are
another daemon's sessions only); this run's tabs went to its scratch config, as every run's.
