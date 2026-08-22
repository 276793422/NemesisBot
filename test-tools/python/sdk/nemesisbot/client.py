"""NemesisBot subprocess client (H6 / U17, minimal version).

Protocol (verified against nemesis-web sources — no explicit "done" frame
exists; the Vue frontend treats an assistant `chat/receive` frame as turn
completion, with a watchdog resync as backstop):
  - send:    {"type":"message","module":"chat","cmd":"send",
              "data":{"content":"..."}}
  - receive: {"type":"message","module":"chat","cmd":"receive",
              "data":{"role":"assistant","content":"..."}}
  - error:   {"type":"system","module":"error","cmd":"notify",
              "data":{"content":"..."}}

Endpoint (fixed 2026-08-22 review round 5): the chat protocol lives on the
WEB SERVER's /ws route (nemesis-web server.rs + websocket_handler.rs), not
on the standalone WebSocketChannel (port 49001, which speaks a different
{type, content} protocol). The web /ws port and auth token are read from
the workspace config; the real bound port (when config asks for 0/dynamic
or the bind differs) comes from <workspace>/state/gateway.json which the
gateway rewrites after bind.

Known limitations (minimal version, per goal):
  - single conversation (no session_id switching)
  - no media
  - send() blocks until the assistant receive frame or the timeout
"""

from __future__ import annotations

import json
import os
import shutil
import signal
import subprocess
import time
from pathlib import Path
from typing import Optional

import websockets


class NemesisBotError(RuntimeError):
    """Raised on lifecycle or protocol errors."""


DEFAULT_WEB_PORT = 49000
DEFAULT_TIMEOUT = 300.0
HEALTH_PORT = 18790


def _read_ws_endpoint(workspace: Path, fallback_port: int) -> tuple[int, str]:
    """Resolve the web-server /ws endpoint for a `--local` workspace.

    Reads <workspace>/.nemesisbot/config.json for channels.web.port and
    auth_token, then prefers the gateway's post-bind report in
    <workspace>/.nemesisbot/workspace/state/gateway.json (real bound port)
    when present and reachable-looking.
    """
    port = fallback_port
    token = ""
    cfg_path = workspace / ".nemesisbot" / "config.json"
    try:
        with open(cfg_path, encoding="utf-8") as f:
            cfg = json.load(f)
        web = (cfg.get("channels") or {}).get("web") or {}
        port = int(web.get("port") or fallback_port)
        token = str(web.get("auth_token") or "")
    except (OSError, ValueError):
        pass  # not onboarded yet / unparsable — fall back to defaults
    state_path = workspace / ".nemesisbot" / "workspace" / "state" / "gateway.json"
    try:
        with open(state_path, encoding="utf-8") as f:
            state = json.load(f)
        real = int(state.get("web_port") or 0)
        if real > 0:
            port = real
    except (OSError, ValueError):
        pass
    return port, token


def _find_binary(explicit: Optional[str] = None) -> str:
    """Locate nemesisbot(.exe): explicit path first, then PATH."""
    if explicit:
        p = Path(explicit)
        if not p.exists():
            raise NemesisBotError(f"binary not found: {explicit}")
        return str(p)
    name = "nemesisbot.exe" if os.name == "nt" else "nemesisbot"
    found = shutil.which(name)
    if not found:
        raise NemesisBotError(
            f"{name} not found on PATH — pass binary=... to NemesisBot()"
        )
    return found


class NemesisBot:
    """Own a `nemesisbot --local gateway` subprocess and chat with it.

    Context-manager recommended::

        with NemesisBot(workspace="./bot") as bot:
            print(bot.send("hello"))

    The subprocess is spawned WITHOUT a new window (creationflags on
    Windows per the project's background-process discipline) and its whole
    process tree is terminated on stop().
    """

    def __init__(
        self,
        workspace: str,
        binary: Optional[str] = None,
        ws_port: Optional[int] = None,
        startup_timeout: float = 60.0,
    ):
        self.workspace = Path(workspace).resolve()
        self.workspace.mkdir(parents=True, exist_ok=True)
        self._binary_arg = binary
        self.binary: Optional[str] = None  # resolved at start()
        self._ws_port_override = ws_port
        self.ws_port = ws_port or DEFAULT_WEB_PORT  # re-resolved at start()
        self.auth_token: str = ""
        self.startup_timeout = startup_timeout
        self._proc: Optional[subprocess.Popen] = None
        self._ws = None

    # -- lifecycle ---------------------------------------------------------

    def start(self) -> None:
        """Spawn the gateway and wait for its health endpoint."""
        if self._proc is not None:
            raise NemesisBotError("already started")
        self.binary = _find_binary(self._binary_arg)
        creationflags = 0
        if os.name == "nt":
            # CREATE_NO_WINDOW: never open a console window (project rule).
            creationflags = subprocess.CREATE_NO_WINDOW  # type: ignore[attr-defined]
        log_path = self.workspace / "sdk_gateway.log"
        log_f = open(log_path, "ab")
        try:
            self._proc = subprocess.Popen(
                [self.binary, "--local", "gateway"],
                cwd=str(self.workspace),
                stdout=log_f,
                stderr=log_f,
                creationflags=creationflags,
            )
        finally:
            log_f.close()
        self._wait_healthy()
        # Resolve the web /ws endpoint AFTER the gateway is up: config.json
        # carries port+token, gateway.json carries the real bound port.
        if self._ws_port_override is None:
            self.ws_port, self.auth_token = _read_ws_endpoint(
                self.workspace, DEFAULT_WEB_PORT
            )
        else:
            _, self.auth_token = _read_ws_endpoint(self.workspace, self.ws_port)

    def _wait_healthy(self) -> None:
        import urllib.request

        deadline = time.monotonic() + self.startup_timeout
        url = f"http://127.0.0.1:{HEALTH_PORT}/health"
        last_err: Optional[Exception] = None
        while time.monotonic() < deadline:
            if self._proc and self._proc.poll() is not None:
                raise NemesisBotError(
                    f"gateway exited early (code {self._proc.returncode}); "
                    f"see {self.workspace / 'sdk_gateway.log'}"
                )
            try:
                with urllib.request.urlopen(url, timeout=2) as resp:
                    if resp.status == 200:
                        return
            except Exception as e:  # noqa: BLE001 — probe loop
                last_err = e
            time.sleep(0.5)
        raise NemesisBotError(
            f"gateway not healthy within {self.startup_timeout}s: {last_err}"
        )

    def stop(self) -> None:
        """Terminate the subprocess tree."""
        if self._ws is not None:
            try:
                import asyncio

                asyncio.get_event_loop().run_until_complete(self._ws.close())
            except Exception:  # noqa: BLE001 — best-effort close
                pass
            self._ws = None
        if self._proc is None:
            return
        if os.name == "nt":
            # Kill the tree (gateway spawns children).
            subprocess.run(
                ["taskkill", "/PID", str(self._proc.pid), "/T", "/F"],
                capture_output=True,
                creationflags=subprocess.CREATE_NO_WINDOW,
            )
        else:
            self._proc.send_signal(signal.SIGTERM)
            try:
                self._proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self._proc.kill()
        self._proc = None

    def __enter__(self) -> "NemesisBot":
        self.start()
        return self

    def __exit__(self, *exc) -> None:
        self.stop()

    # -- chat ----------------------------------------------------------------

    def send(self, prompt: str, timeout: float = DEFAULT_TIMEOUT) -> str:
        """Send one turn; block for the assistant's reply frame.

        Completion detection mirrors the Vue frontend: an assistant
        `chat/receive` frame completes the turn (the protocol has no
        explicit done frame). `system/error` frames raise.
        """
        import asyncio

        return asyncio.run(self._send_async(prompt, timeout))

    async def _send_async(self, prompt: str, timeout: float) -> str:
        uri = f"ws://127.0.0.1:{self.ws_port}/ws"
        if self.auth_token:
            uri = f"{uri}?token={self.auth_token}"
        async with websockets.connect(uri, open_timeout=10) as ws:
            send_frame = {
                "type": "message",
                "module": "chat",
                "cmd": "send",
                "data": {"content": prompt},
            }
            await ws.send(json.dumps(send_frame))
            while True:
                try:
                    raw = await asyncio.wait_for(ws.recv(), timeout=timeout)
                except asyncio.TimeoutError:
                    raise NemesisBotError(
                        f"no assistant reply within {timeout}s"
                    ) from None
                frame = json.loads(raw)
                if (
                    frame.get("type") == "message"
                    and frame.get("module") == "chat"
                    and frame.get("cmd") == "receive"
                ):
                    data = frame.get("data") or {}
                    if data.get("role") == "assistant":
                        return data.get("content") or ""
                if (
                    frame.get("type") == "system"
                    and frame.get("module") == "error"
                ):
                    data = frame.get("data") or {}
                    raise NemesisBotError(
                        data.get("content") or "server error frame"
                    )
