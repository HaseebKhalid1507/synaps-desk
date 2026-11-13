# Idle CPU against the desktopproto sandbox (bella, headless gamescope 1600x1000)

One window at a time, attached to the same sandbox session, nobody touching it.
"idle" = 30 s with the session idle; "running" = 25 s while the session's `bash`
tool runs `sleep 50` (a turn is live, no prompt waiting). CPU = utime+stime of the
app process over wall time (scripts' method, `/proc/<pid>/stat`).

| run | binary | idle | running tool |
|---|---|---|---|
| 1 | main deabaf1 | 0.167 % | 0.560 % |
| 1 | hybrid, 2 Hz blinking dot (dropped) | 0.200 % | 1.200 % |
| 2 | hybrid, static dot (this PR) | 0.200 % | 0.880 % |
| 3 | main deabaf1 | 0.200 % | 0.640 % |
| 4 | hybrid, static dot (this PR) | 0.233 % | 0.800 % |

Raw:
```
CPU main-idle: cpu_pct=0.167 cpu_s=0.05 wall_s=30.0 rss_kb=169632
CPU main-running: cpu_pct=0.560 cpu_s=0.14 wall_s=25.0 rss_kb=170656
CPU hybrid-idle: cpu_pct=0.200 cpu_s=0.06 wall_s=30.0 rss_kb=174872
CPU hybrid-running: cpu_pct=1.200 cpu_s=0.30 wall_s=25.0 rss_kb=175640
CPU hybrid-idle: cpu_pct=0.200 cpu_s=0.06 wall_s=30.0 rss_kb=175084
CPU hybrid-running: cpu_pct=0.880 cpu_s=0.22 wall_s=25.0 rss_kb=176252
CPU main-idle: cpu_pct=0.200 cpu_s=0.06 wall_s=30.0 rss_kb=171768
CPU main-running: cpu_pct=0.640 cpu_s=0.16 wall_s=25.0 rss_kb=172116
CPU hybrid-idle: cpu_pct=0.233 cpu_s=0.07 wall_s=30.0 rss_kb=178212
CPU hybrid-running: cpu_pct=0.800 cpu_s=0.20 wall_s=25.0 rss_kb=178832
```
(The hybrid-running rows are in order: blink, static, static.)
