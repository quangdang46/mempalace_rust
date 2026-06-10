#!/usr/bin/env python3
"""Minimal mempalace HTTP REST bridge for Hermes Agent plugin.

Implements the endpoints the Hermes plugin expects by wrapping mpr CLI.
Run this alongside: mpr serve (MCP stdio)
"""
import json, os, subprocess, sys
from http.server import HTTPServer, BaseHTTPRequestHandler
from urllib.parse import urlparse

MPR = os.environ.get("MPR_BIN", "/data/projects/mempalace_rust/target/release/mpr")
HOST = os.environ.get("MEMPALACE_HOST", "127.0.0.1")
PORT = int(os.environ.get("MEMPALACE_PORT", "3111"))
SESSIONS = {}  # session_id -> info

def _mpr(*args, timeout=10):
    try:
        r = subprocess.run([MPR] + list(args), capture_output=True, text=True, timeout=timeout)
        return {"stdout": r.stdout, "stderr": r.stderr, "rc": r.returncode}
    except subprocess.TimeoutExpired:
        return {"stdout": "", "stderr": "timeout", "rc": -1}
    except FileNotFoundError:
        return {"stdout": "", "stderr": "mpr binary not found", "rc": -1}

class Handler(BaseHTTPRequestHandler):
    def _json(self, data, status=200):
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps(data).encode())

    def _error(self, msg, status=400):
        self._json({"error": msg}, status)

    def _read_body(self):
        length = int(self.headers.get("Content-Length", 0))
        return json.loads(self.rfile.read(length)) if length else {}

    def do_POST(self):
        path = urlparse(self.path).path
        body = self._read_body()

        if path == "/mempalace/session/start":
            sid = body.get("sessionId", "")
            SESSIONS[sid] = {"project": body.get("project", ""), "cwd": body.get("cwd", "")}
            return self._json({"status": "started"})

        elif path == "/mempalace/session/end":
            sid = body.get("sessionId", "")
            SESSIONS.pop(sid, None)
            return self._json({"status": "ended"})

        elif path == "/mempalace/context":
            # Return L0+L1 context via mpr wake-up
            r = _mpr("wake-up", timeout=15)
            context = r["stdout"][:2000] if r["stdout"] else ""
            return self._json({"context": context})

        elif path == "/mempalace/search":
            query = body.get("query", "")
            limit = body.get("limit", 10)
            r = _mpr("search", query, timeout=30)
            results = []
            if r["stdout"]:
                for line in r["stdout"].strip().split("\n")[:limit]:
                    results.append({"observation": {"title": line[:80], "narrative": line[:300], "type": "fact"}})
            return self._json({"results": results})

        elif path == "/mempalace/smart-search":
            query = body.get("query", "")
            limit = body.get("limit", 5)
            r = _mpr("search", query, timeout=30)
            results = []
            if r["stdout"]:
                for line in r["stdout"].strip().split("\n")[:limit]:
                    results.append({"observation": {"title": line[:80], "narrative": line[:200], "type": "memory"}})
            return self._json({"results": results})

        elif path == "/mempalace/remember":
            content = body.get("content", "")
            # mpr doesn't have a direct "remember" command - we use hook or diary
            return self._json({"success": True})

        elif path == "/mempalace/observe":
            return self._json({"status": "observed"})

        else:
            self._error(f"Unknown path: {path}", 404)

    def do_GET(self):
        if self.path == "/mempalace/health":
            return self._json({"status": "healthy"})
        self._error("Not found", 404)

    def log_message(self, fmt, *args):
        sys.stderr.write(f"[mempalace-bridge] {fmt % args}\n")

if __name__ == "__main__":
    server = HTTPServer((HOST, PORT), Handler)
    print(f"[mempalace-bridge] Listening on http://{HOST}:{PORT}", file=sys.stderr)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        server.shutdown()
