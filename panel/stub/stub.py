"""Test stand-in for hydra-browser: /status on 8080 and a minimal MCP server on 8931.

noVNC (6080) is real: Xvfb + x11vnc + websockify, started by start.sh.
"""
import json, os, threading, time, urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

EGRESS = None

def lookup():
    global EGRESS
    try:
        with urllib.request.urlopen("https://ipinfo.io/json", timeout=5) as r:
            d = json.load(r)
        EGRESS = {k: d.get(k) for k in ("ip", "city", "region", "country", "org", "timezone")}
    except Exception as e:
        print("egress lookup failed:", e, flush=True)

class Status(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path != "/status":
            self.send_error(404); return
        body = json.dumps({"ready": EGRESS is not None, "exit": os.environ.get("HYDRA_EXIT"), "egress": EGRESS}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

TOOLS = [{"name": "browser_navigate", "description": "Navigate to a URL", "inputSchema": {"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]}}]

class Mcp(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))))
        if "id" not in req:
            self.send_response(202); self.send_header("content-length", "0"); self.end_headers(); return
        m = req["method"]
        if m == "initialize":
            result = {"protocolVersion": req["params"]["protocolVersion"], "capabilities": {"tools": {}}, "serverInfo": {"name": "stub", "version": "0"}}
        elif m == "tools/list":
            result = {"tools": TOOLS}
        elif m == "tools/call":
            result = {"content": [{"type": "text", "text": f"stub: {json.dumps(req['params'])}"}]}
        else:
            result = {}
        msg = json.dumps({"jsonrpc": "2.0", "id": req["id"], "result": result})
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("mcp-session-id", "stub-session")
        self.send_header("transfer-encoding", "chunked")
        self.end_headers()
        for chunk in (": warming up\n\n", f"event: message\ndata: {msg}\n\n"):
            data = chunk.encode()
            self.wfile.write(b"%x\r\n%s\r\n" % (len(data), data)); self.wfile.flush()
            time.sleep(0.3)
        self.wfile.write(b"0\r\n\r\n")

threading.Thread(target=lookup, daemon=True).start()
threading.Thread(target=ThreadingHTTPServer(("0.0.0.0", 8931), Mcp).serve_forever, daemon=True).start()
ThreadingHTTPServer(("0.0.0.0", 8080), Status).serve_forever()
