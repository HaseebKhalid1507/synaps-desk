# Events in the desk: evidence

Live on the **desktopproto sandbox** on bella (daemon 0.11.1, haiku-4-5), `live.sh` phases `setup`, `order`,
`record`, `reload`, `teardown`. The sandbox config got the two model-credential lines for the run and was restored
after: sha `c1b7ac41…320edd` before and after, 0 auth lines left (`logs/live.log`). Every app run's
`XDG_CONFIG_HOME` was an absolute scratch folder under `~/.cache/events-live` (`scripts/capture-linux.sh`); bella's
real `~/.config/synaps-desktop/settings.json` kept its mtime (`07:04:07.671`, before the first run) through every run.
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
`SteeringDelivered` (or `Steered`) carrying the same id adds nothing; the history append that holds the
message turns the note into that message's row, at the message's place (same row id, its live time and the
frame-only fields kept). For an idle wake that is right above the turn's reply.

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
data; a class this client doesn't know is withheld (closed, as the gate fails). So a reload never shows less than
the live row did, and the desk never displays a class's content that its own history would not. Tests:
`events::tests::a_withheld_class_shows_its_marker_live_as_in_the_history`.

## Stills and the clip

- `events-live-1920-nord.mp4`: the whole `record` run, 150 s at 30 fps, lossless (libx264rgb qp 0, gbrp; frame
  md5 `ce9a02db17aa`, the same as the capture's mkv). Drive log with event times: `logs/live-1920-nord-drive.log`.
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
