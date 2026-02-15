#!/usr/bin/env python3
"""Orion Toolbox Exec Server

Minimal HTTP server that accepts command execution requests from the
orion-api agent container.  Runs inside the toolbox sidecar and exposes:

  POST /exec   — run a command, return stdout/stderr/exit_code
  GET  /health — liveness probe
  GET  /tools  — list available commands and package managers

Security:
  - Listens only on 0.0.0.0:9090 (Docker internal network, not exposed to host)
  - Requires Bearer token matching TOOLBOX_SECRET env var
  - Enforces per-request timeout (default 120 s)
  - Caps captured output at 64 KB per stream
  - Blocks dangerous command patterns (same list as skill-shell)
"""

import http.server
import json
import os
import shutil
import signal
import subprocess
import sys
import threading
from urllib.parse import urlparse

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

LISTEN_HOST = "0.0.0.0"
LISTEN_PORT = int(os.environ.get("TOOLBOX_PORT", "9090"))
SECRET = os.environ.get("TOOLBOX_SECRET", "")
DEFAULT_TIMEOUT = int(os.environ.get("TOOLBOX_TIMEOUT", "120"))
MAX_OUTPUT_BYTES = 65_536
MAX_REQUEST_BYTES = 1_048_576  # 1 MB

# ---------------------------------------------------------------------------
# Safety — mirrors skill-shell BLOCKED_PATTERNS and ELEVATED_COMMANDS
# ---------------------------------------------------------------------------

BLOCKED_PATTERNS = [
    "rm -rf /",
    "rm -rf /*",
    "rm -rf ~",
    "mkfs.",
    "dd if=/dev/zero",
    "dd if=/dev/random",
    ":(){:|:&};:",
    "chmod -r 777 /",
    "chown -r",
    "shutdown",
    "reboot",
    "poweroff",
    "halt",
    "init 0",
    "init 6",
    "format c:",
    "> /dev/sda",
    "mv /* /dev/null",
]

ELEVATED_COMMANDS = ["sudo", "su ", "pkill", "kill -9", "killall"]

KNOWN_TOOLS = [
    "gcc", "g++", "make", "cmake",
    "python3", "pip3", "pip",
    "node", "npm",
    "git", "wget", "curl", "jq",
    "ssh", "rsync", "zip", "unzip",
    "apt-get", "apt", "dpkg",
]

KNOWN_PACKAGE_MANAGERS = [
    "apt-get", "apt", "pip", "pip3", "npm",
]


def check_safety(command: str) -> str | None:
    """Return an error message if the command is blocked, else None."""
    lower = command.lower()
    for pattern in BLOCKED_PATTERNS:
        if pattern in lower:
            return f"Command blocked: contains dangerous pattern '{pattern}'"
    for pattern in ELEVATED_COMMANDS:
        if lower.startswith(pattern) or f" {pattern}" in lower:
            return f"Elevated command '{pattern.strip()}' is not allowed"
    return None


def truncate(text: str) -> tuple[str, bool]:
    """Truncate to MAX_OUTPUT_BYTES; return (text, was_truncated)."""
    if len(text.encode("utf-8", errors="replace")) <= MAX_OUTPUT_BYTES:
        return text, False
    truncated = text.encode("utf-8", errors="replace")[:MAX_OUTPUT_BYTES].decode(
        "utf-8", errors="replace"
    )
    return truncated + "\n... [output truncated]", True


def discover_tools() -> list[str]:
    """Return list of available tool binaries from KNOWN_TOOLS."""
    return [t for t in KNOWN_TOOLS if shutil.which(t)]


def discover_package_managers() -> list[str]:
    """Return list of available package managers."""
    return [m for m in KNOWN_PACKAGE_MANAGERS if shutil.which(m)]


# ---------------------------------------------------------------------------
# HTTP handler
# ---------------------------------------------------------------------------

class ToolboxHandler(http.server.BaseHTTPRequestHandler):
    """Handle /health, /tools, and /exec endpoints."""

    # Suppress default stderr logging per request
    def log_message(self, fmt, *args):
        sys.stderr.write("[toolbox] %s\n" % (fmt % args))

    # --- auth -----------------------------------------------------------------

    def _check_auth(self) -> bool:
        if not SECRET:
            return True  # no secret configured — allow (dev mode)
        auth = self.headers.get("Authorization", "")
        if auth == f"Bearer {SECRET}":
            return True
        self._json_response(401, {"error": "Unauthorized"})
        return False

    # --- helpers --------------------------------------------------------------

    def _json_response(self, status: int, body: dict):
        payload = json.dumps(body).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def _read_json_body(self) -> dict | None:
        length = int(self.headers.get("Content-Length", "0"))
        if length > MAX_REQUEST_BYTES:
            self._json_response(413, {"error": "Request body too large"})
            return None
        if length == 0:
            self._json_response(400, {"error": "Empty request body"})
            return None
        try:
            raw = self.rfile.read(length)
            return json.loads(raw)
        except (json.JSONDecodeError, UnicodeDecodeError) as exc:
            self._json_response(400, {"error": f"Invalid JSON: {exc}"})
            return None

    # --- GET ------------------------------------------------------------------

    def do_GET(self):
        path = urlparse(self.path).path

        if path == "/health":
            self._json_response(200, {"status": "ok"})
            return

        if path == "/tools":
            if not self._check_auth():
                return
            self._json_response(200, {
                "available_tools": discover_tools(),
                "package_managers": discover_package_managers(),
            })
            return

        self._json_response(404, {"error": "Not found"})

    # --- POST -----------------------------------------------------------------

    def do_POST(self):
        path = urlparse(self.path).path

        if path == "/exec":
            if not self._check_auth():
                return
            self._handle_exec()
            return

        self._json_response(404, {"error": "Not found"})

    def _handle_exec(self):
        body = self._read_json_body()
        if body is None:
            return

        command = body.get("command", "").strip()
        if not command:
            self._json_response(400, {"error": "Missing 'command' field"})
            return

        timeout = body.get("timeout", DEFAULT_TIMEOUT)
        if not isinstance(timeout, (int, float)) or timeout <= 0:
            timeout = DEFAULT_TIMEOUT
        timeout = min(timeout, 300)  # hard cap at 5 min

        working_dir = body.get("working_directory", "/workspace/data")

        # Safety check
        err = check_safety(command)
        if err:
            self._json_response(200, {
                "success": False,
                "exit_code": -1,
                "stdout": "",
                "stderr": err,
                "stdout_truncated": False,
                "stderr_truncated": False,
                "blocked": True,
            })
            return

        # Execute
        try:
            proc = subprocess.run(
                ["sh", "-c", command],
                capture_output=True,
                timeout=timeout,
                cwd=working_dir if os.path.isdir(working_dir) else "/workspace",
            )
            stdout_str = proc.stdout.decode("utf-8", errors="replace")
            stderr_str = proc.stderr.decode("utf-8", errors="replace")
            stdout_str, stdout_trunc = truncate(stdout_str)
            stderr_str, stderr_trunc = truncate(stderr_str)

            self._json_response(200, {
                "success": proc.returncode == 0,
                "exit_code": proc.returncode,
                "stdout": stdout_str,
                "stderr": stderr_str,
                "stdout_truncated": stdout_trunc,
                "stderr_truncated": stderr_trunc,
                "blocked": False,
            })
        except subprocess.TimeoutExpired:
            self._json_response(200, {
                "success": False,
                "exit_code": -1,
                "stdout": "",
                "stderr": f"Command timed out after {timeout} seconds",
                "stdout_truncated": False,
                "stderr_truncated": False,
                "blocked": False,
            })
        except OSError as exc:
            self._json_response(200, {
                "success": False,
                "exit_code": -1,
                "stdout": "",
                "stderr": f"Failed to execute command: {exc}",
                "stdout_truncated": False,
                "stderr_truncated": False,
                "blocked": False,
            })


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    server = http.server.HTTPServer((LISTEN_HOST, LISTEN_PORT), ToolboxHandler)
    print(f"[toolbox] Listening on {LISTEN_HOST}:{LISTEN_PORT}", flush=True)

    # Graceful shutdown on SIGTERM (Docker stop)
    def _shutdown(signum, frame):
        print("[toolbox] Shutting down...", flush=True)
        threading.Thread(target=server.shutdown).start()

    signal.signal(signal.SIGTERM, _shutdown)
    signal.signal(signal.SIGINT, _shutdown)

    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
