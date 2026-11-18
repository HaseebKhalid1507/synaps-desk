#!/usr/bin/env python3
"""Hand one synthetic event to a session's event socket (as agent-comms does; `synaps send` has no
--expects-response). usage: inject.py <run dir> <session id> <json event fields>"""
import json, os, socket, sys, uuid
from datetime import datetime, timezone

run, sid, fields = sys.argv[1], sys.argv[2], json.loads(sys.argv[3])
reg = json.load(open(os.path.join(run, sid + ".json")))
ev = {"id": str(uuid.uuid4()), "timestamp": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
      "source": {"source_type": "agent-comms", "name": "agent-comms", "callback": None},
      "channel": None, "sender": None,
      "content": {"text": "", "content_type": "message", "severity": "Medium", "data": None},
      "expects_response": False, "reply_to": None}
for k, v in fields.items():
    if k in ("text", "severity", "content_type"):
        ev["content"][k] = v
    else:
        ev[k] = v
with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
    s.settimeout(2.0); s.connect(reg["socket_path"]); s.sendall(json.dumps(ev).encode()); s.shutdown(socket.SHUT_WR)
print(ev["id"])
