"""Tests for the NemesisBot Python SDK.

The end-to-end test is marked `requires_binary` and SKIPS unless a real
nemesisbot binary is locatable (PATH or NEMESISBOT_BIN env) — CI does not
install Python, so this suite never runs under cargo.
"""

import json
import os
import shutil
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from nemesisbot.client import (  # noqa: E402
    DEFAULT_WEB_PORT,
    NemesisBot,
    NemesisBotError,
    _find_binary,
    _read_ws_endpoint,
)


def _binary_available() -> bool:
    env = os.environ.get("NEMESISBOT_BIN")
    if env and Path(env).exists():
        return True
    name = "nemesisbot.exe" if os.name == "nt" else "nemesisbot"
    return shutil.which(name) is not None


def test_find_binary_explicit_missing_raises():
    with pytest.raises(NemesisBotError):
        _find_binary("/nonexistent/path/nemesisbot")


def test_workspace_created(tmp_path):
    ws = tmp_path / "sub" / "ws"
    bot = NemesisBot(workspace=str(ws))
    assert (ws).exists()


def test_read_ws_endpoint_from_config(tmp_path):
    """Port+token come from <ws>/.nemesisbot/config.json; gateway.json
    post-bind port wins when present; gateway.port drives the health probe
    (per-workspace, not the 18790 machine default)."""
    ws = tmp_path
    cfg_dir = ws / ".nemesisbot"
    cfg_dir.mkdir()
    (cfg_dir / "config.json").write_text(
        json.dumps(
            {
                "channels": {"web": {"port": 51234, "auth_token": "tok123"}},
                "gateway": {"port": 51999},
            }
        ),
        encoding="utf-8",
    )
    port, token, health = _read_ws_endpoint(ws, DEFAULT_WEB_PORT)
    assert port == 51234
    assert token == "tok123"
    assert health == 51999
    # post-bind state overrides the configured web port (health stays put)
    state_dir = cfg_dir / "workspace" / "state"
    state_dir.mkdir(parents=True)
    (state_dir / "gateway.json").write_text(
        json.dumps({"pid": 1, "web_port": 51240}), encoding="utf-8"
    )
    port, token, health = _read_ws_endpoint(ws, DEFAULT_WEB_PORT)
    assert port == 51240
    assert token == "tok123"
    assert health == 51999


def test_read_ws_endpoint_defaults_when_missing(tmp_path):
    port, token, health = _read_ws_endpoint(tmp_path, DEFAULT_WEB_PORT)
    assert port == DEFAULT_WEB_PORT
    assert token == ""
    assert health == 18790


def test_turn_missing_binary_raises_before_start(tmp_path):
    """turn() validates the binary up front — no gateway spawn attempt."""
    bot = NemesisBot(workspace=str(tmp_path), binary="/nonexistent/nemesisbot")
    with pytest.raises(NemesisBotError):
        bot.turn("hello")
    assert bot._proc is None  # nothing started, nothing to clean up


@pytest.mark.skipif(not _binary_available(), reason="requires nemesisbot binary")
def test_end_to_end_send(tmp_path):
    """Real run: spawn gateway in a temp workspace, one turn, stop.

    Requires a configured model — run `nemesisbot model add ... --default`
    against NEMESISBOT_BIN's home first, or point workspace at an existing
    configured .nemesisbot dir.
    """
    import time

    bot = NemesisBot(workspace=str(tmp_path))
    bot.start()
    try:
        reply = bot.send("Reply with exactly: OK", timeout=120)
        assert isinstance(reply, str)
        assert len(reply) > 0
    finally:
        bot.stop()
    # Process tree gone (Windows taskkill /T).
    time.sleep(0.5)
    assert bot._proc is None


@pytest.mark.skipif(not _binary_available(), reason="requires nemesisbot binary")
def test_end_to_end_turn_owns_lifecycle(tmp_path):
    """Real run through the 3-line entry point: turn() starts the gateway
    when not running and stops it afterwards (same model requirement as
    test_end_to_end_send)."""
    import time

    bot = NemesisBot(workspace=str(tmp_path))
    reply = bot.turn("Reply with exactly: OK", timeout=120)
    assert isinstance(reply, str)
    assert len(reply) > 0
    # turn() owned the gateway → it must have stopped it again.
    assert bot._proc is None
