# Q2 Authorize: finish-gate evidence (polish.md §6)

**Binaries.** `main` is the installed `synaps-desktop-hybrid.q4` (built from 609cf04, whose tree is
byte-identical to main 4f87411: `git diff 609cf04 4f87411` is empty), sha256 `61e724e6…`. `q2` is this
PR's release build at 4475cdf, sha256 `11478689…`, installed as `bella:~/.local/bin/synaps-desktop-hybrid.q2`.

**How.** Each still comes from the `--offline` demo (relay-refresh, its guard confirm pending) in a headless
gamescope on bella. The flags are `--motion off` and `--size W×H`, with the theme by `--theme`. Captures
use `import -window`. Steps, after a 4 s settle:

| file | step |
|---|---|
| `*-1-card.png` | the card as launched |
| `*-3-allow-all.png` | `space` sent to the window (the card has the keys; armed) |
| `*-5-stub.png` | 15 wheel notches up over the list, so the card's row is off screen |
| `*-2-secret.png` | a second launch with `SYNAPS_DEMO_PROMPT=secret` (a demo knob added here) |

Themes: nord, night-city, default, paper. Sizes: 1920×1080 and 1280×720. Files are named
`<main|q2>-<theme>-<width>-<n>-<state>.png`.

**What the main side shows.** Main's demo has no secret prompt and no `raised_at` / `confirm_scope`, so
`main-*-2-secret.png` is main's confirm card. Main has no Allow all, so its `3-allow-all` matches its `1-card`.
Main's card stays in the dock when the list scrolls, so its `5-stub` shows the docked card, not a stub.

**What the stills show.** The card sits in the log directly under the bash call it gates (`waiting`). It is flat
`card` with the 1 px `warn_mark` frame and the 2 px dashes, 8 on and 4 off, along its top. The header reads
`A U T H O R I Z E` and, on the right, `bash · guard · asked HH:MM`. The well holds the command alone, its `$`
in `you`. Under it is the folder line, `in ~/work/relay-gateway`. The last line has the Allow all box with the
daemon's own text, then the footer, then `D E N Y` and `A L L O W`. Offline nothing can be answered: the
footer reads `offline: nothing to answer` and ALLOW is drawn disabled. The demo client owns the session's
input, so Allow all is offered. Scrolled away, the stub reads `NEEDS YOU · bash: cargo test … · ctrl j`.

**Not in these stills.** The receipt and the collapse need an answer, and the offline demo cannot answer or
raise a prompt. The tests cover them: `the_answered_card_collapses_in_its_row`, `a_receipt_says_who_answered`,
`allow_all_is_the_input_owners`. They are not shown here. The PR body lists what is still open.
