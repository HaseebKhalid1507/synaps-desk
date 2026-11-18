#!/usr/bin/env python3
"""Observe one daemon session's frames (an Observe attach, Digest history) and log their order:
one line per frame, its arrival time, seq and kind, and for events their id and severity.
Deltas are counted by kind (text, thinking, ...), not printed. usage: frames.py <daemon socket> <session id> <seconds> <log>"""
import json, socket, sys, time

sock_path, sid, secs, out = sys.argv[1], sys.argv[2], float(sys.argv[3]), sys.argv[4]
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(sock_path)
f = s.makefile("rwb")
def send(v):
    f.write((json.dumps(v) + "\n").encode()); f.flush()
send({"type": "hello", "protocol_version": 3, "cwd": "/tmp", "client_version": "evframes/0",
      "client": {"kind": "attach", "terminal": None, "instance": "evframes", "history": "digest", "tail_items": 0,
                 "capabilities": {"tool_queue": True, "owner_reason_input": True}, "name": "evframes"}})
send({"type": "attach", "attach": "existing", "session_id": sid, "mode": "observe"})
t0 = time.time(); s.settimeout(0.5); deltas = {}
log = open(out, "a")
def line(t, msg):
    log.write(f"{t - t0:8.3f} {msg}\n"); log.flush()
def flush(now):
    if deltas:
        line(now, "  (" + ", ".join(f"{n} {k}" for k, n in deltas.items()) + " deltas)"); deltas.clear()
buf = b""
while time.time() - t0 < secs:
    try:
        chunk = s.recv(1 << 20)
    except socket.timeout:
        continue
    if not chunk:
        break
    buf += chunk
    while b"\n" in buf:
        raw, buf = buf.split(b"\n", 1)
        try:
            fr = json.loads(raw)
        except Exception:
            continue
        now = time.time()
        if fr.get("type") != "event":
            line(now, f"frame {fr.get('type')}")
            continue
        ev = fr.get("event", {}); kind = ev.get("ev"); seq = fr.get("seq")
        if kind == "stream":
            st = ev.get("event", {}); sub = st.get("llm") or st.get("agent") or st.get("session")
            if sub in ("text", "thinking", "tool_use_delta", "tool_result_delta"):
                deltas[sub] = deltas.get(sub, 0) + 1; continue
            flush(now)
            extra = ""
            if sub == "steering_delivered":
                m = st.get("message", "")
                extra = " event-payload id=" + m.split('id="', 1)[1].split('"', 1)[0] if m.startswith("<event ") else " user-steer"
            if sub == "message_history":
                extra = f" messages={len(st.get('messages', []))}"
            if sub in ("tool_use", "tool_use_start", "tool_result"):
                extra = f" {st.get('tool_name', '')} {st.get('tool_id', '')}"
            line(now, f"seq {seq} stream/{st.get('kind')}/{sub}{extra}")
            continue
        flush(now)
        extra = ""
        if kind == "external":
            e = ev.get("event", {}); c = e.get("content", {})
            extra = f" id={e.get('id')} source={e.get('source', {}).get('source_type')} severity={c.get('severity')} expects_response={e.get('expects_response')}"
        elif kind == "turn_started":
            extra = f" trigger={ev.get('trigger')} user_text={'set' if ev.get('user_text') else 'none'}"
        elif kind == "steered":
            t = ev.get("text", "")
            extra = " event-payload" if t.startswith("<event ") else " user-steer"
        elif kind == "conversation":
            extra = f" messages_len={ev.get('digest', {}).get('messages_len')}"
        line(now, f"seq {seq} {kind}{extra}")
