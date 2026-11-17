# Message times: evidence

Captured headless on bella (gamescope, nord). The `demo-*` shots use the `--offline` demo, whose histories carry fixed
local times, so they look the same on any machine. The `live-*` shots come from the desktopproto sandbox daemon
(0.11.1, haiku). Bella's zone is America/New_York, so the times shown are EST and the wire times are UTC.

| Shot | What it shows |
|---|---|
| `demo-wide-times-in-the-margin.png` | A wide pane: each YOU and AGENT label has its local `HH:MM` under it in the margin (`m.meta` mono, `dim`). |
| `demo-narrow-times-at-the-line-end.png` | A narrow pane (760 px, no hang): the time sits at the end of the label's line. |
| `demo-time-tooltip.png` | Hovering a time shows the date and the seconds: `Fri Nov 13 2026, 04:12:07`. |
| `demo-resumed-next-morning-day-line.png` | *Idle CPU*, taken up again the next morning: a day line (`Sat Nov 14`, between two `hair` rules) over that turn. |
| `demo-legacy-no-times.png` | *Release notes for 0.12*, a history with no `_synaps_at`: no times, never "now". |
| `live-streaming-event-ts.png` | A live turn while it streams: AGENT is stamped with the `ts` of the event that started its row. |
| `live-done-history-time.png` | The same turn once its history landed: the message time (`_synaps_at`) has replaced it, and the text is the same. |
| `live-stamp-to-history-pair.png` | The two above, cropped and stacked: `03:39` before and after. |
| `live-resumed-next-day.png` | A sandbox session whose journal was dated the evening before (`_message_times` moved to 2026-11-16 22:15Z), resumed by name and given a new turn: `Tue Nov 17` over the new turn, and the old turn keeps its times. |
| `live-worker-view-journal-time.png` | A worker's view (scout, `sa_1`): AGENT's time comes from the worker's journal (`at_ms`, `now_ms`). |
