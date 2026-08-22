# NemesisBot Python SDK (minimal)

Drive a NemesisBot agent as a subprocess from Python: spawn
`nemesisbot --local gateway` in a workspace, chat over its WebSocket.

## Install

```bash
pip install .          # from python/sdk/
# dev:
pip install -e ".[dev]"
```

Requires Python ≥3.10 and the `websockets` package (auto-installed).
A `nemesisbot(.exe)` binary must be on PATH or passed via `binary=`.

## Usage

```python
from nemesisbot import NemesisBot

with NemesisBot(workspace="./mybot") as bot:
    reply = bot.send("List the files in this directory.")
    print(reply)
```

First run in a fresh workspace needs a model configured
(`nemesisbot model add --model <vendor/model> --key <KEY> --default`),
same as any gateway.

## Protocol notes

Verified against `nemesis-web` sources (`websocket_handler.rs`,
`server.rs`, Vue `ChatPanel.vue`):

- send: `{"type":"message","module":"chat","cmd":"send","data":{"content":"…"}}`
- reply: `{"type":"message","module":"chat","cmd":"receive","data":{"role":"assistant","content":"…"}}`
- error: `{"type":"system","module":"error","cmd":"notify","…}`

**There is no explicit "done" frame.** The web frontend treats the
assistant `receive` frame as turn completion (with a watchdog resync as
backstop); the SDK does the same — `send()` returns on the first
assistant frame, or raises after `timeout` (default 300 s).

## Known limitations (minimal version)

- single conversation per process (no session switching)
- no media (text only)
- one turn at a time; no streaming callbacks

## Tests

```bash
pip install pytest
pytest tests/           # unit tests always run; e2e skips without a binary
NEMESISBOT_BIN=... pytest tests/ -k end_to_end   # real run
```
