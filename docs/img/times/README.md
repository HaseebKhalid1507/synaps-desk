# Message times: evidence

Captured headless on bella (gamescope, nord). The `demo-*` shots use the `--offline` demo, whose histories carry fixed
local times, so they look the same on any machine. The `live-*` shots come from the desktopproto sandbox daemon
(0.11.1, haiku). Bella's zone is America/New_York, so the times shown are EST and the wire times are UTC.

| Shot | What it shows |
|---|---|
| `demo-wide-times-in-the-margin.png` | A wide pane: each YOU and AGENT label has its local `HH:MM` under it in the margin (`m.meta` mono, `dim`). Re-captured in round 2 on F1's gutter (the label and its time hang outside the row's reveal clip). |
| `demo-narrow-times-at-the-line-end.png` | A narrow pane (760 px, no hang): the time sits at the end of the label's line. |
| `demo-time-tooltip.png` | Hovering a time shows the date and the seconds: `Fri Nov 13 2026, 04:12:07`. |
| `demo-resumed-next-morning-day-line.png` | *Idle CPU*, taken up again the next morning: a day line (`Sat Nov 14`, between two `hair` rules) over that turn. |
| `demo-legacy-no-times.png` | *Release notes for 0.12*, a history with no `_synaps_at`: no times, never "now". |
| `live-streaming-event-ts.png` | A live turn while it streams: AGENT is stamped with the `ts` of the event that started its row. |
| `live-done-history-time.png` | Round 1: the same turn once its history landed, the history's time had replaced the live one (same minute here). Round 2 keeps the live time instead (below). |
| `live-stamp-to-history-pair.png` | The two above, cropped and stacked: `03:39` before and after. |
| `live-resumed-next-day.png` | A sandbox session whose journal was dated the evening before (`_message_times` moved to 2026-11-16 22:15Z), resumed by name and given a new turn: `Tue Nov 17` over the new turn, and the old turn keeps its times. |
| `live-worker-view-journal-time.png` | A worker's view (scout, `sa_1`): AGENT's time comes from the worker's journal (`at_ms`, `now_ms`). |

## Round 2 (review of #12, M1: a row keeps the first time it had)

The daemon dates a message when its round ends (`stamp_new` at the round checkpoint), so an AGENT row that started
streaming at 04:48:24 has `_synaps_at` 04:49:41 once a 75 s tool call is done. Round 2 keeps the live time, as the TUI does.

| Shot | What it shows |
|---|---|
| `live-r2-agent-time-stays.png` | One turn, `sleep 75`, hovered: AGENT `04:48` (`Tue Nov 17 2026, 04:48:24`) while the tool runs (top) and after the turn, its history dated 04:49:41 (bottom). No jump. |
| `live-r2-live-then-fresh-attach.png` | Another such turn: `04:43` while running and after (rows 1-2); the same session opened in a fresh window shows the history's `04:44` (row 3): a row first seen in history has the history's time, as a TUI resume. |
| `live-r2-main-vs-pr-same-turn.png` | Two windows, same prompt at the same moment, four snaps across the turn (0:08, 0:28, 0:58, 1:03). Top: `main` (`8d6ecc5`), no times. Bottom: this PR, `04:46` on YOU and AGENT throughout. |
| `live-r4-reattach-after-tui-clear.png` | Round 4, on the desktopproto sandbox (0.11.1). The desktop asks "Reply with exactly one word: pong" in tab A, opens tab B (another session, so A is detached), then the TUI attaches A, runs `/clear` and asks the same question, and the desktop goes back to tab A (a re-attach from the tab cache). Top row, `71cb62e`, which compares the attach's `meta.journal_id`: the re-asked YOU row shows `05:48:35`, the old conversation's time; it was asked at 05:49:55. Bottom row, this round, which reads the journal from the conversation header: YOU shows `05:52:45`, its own time. On the wire, the attach after the clear had `meta.journal_id` = the first journal and `header.id` = the new one; `cleared` named the new journal. |
| `live-r4-reattach-after-tui-clear-full.png` | The bottom-right frame of the above, full window. |
