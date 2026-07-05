#!/usr/bin/env python3
"""Model-eval battery runner — sends a batch of prompts to a running NemesisBot
gateway over WebSocket (the real agent path: tools + time injection + persona)
and records each assistant reply.

Part of the model-eval Skill. See ../SKILL.md for the full process.

Usage:
    python run_battery.py --url "ws://127.0.0.1:49000/ws?token=TOKEN" \
        --start 0 --end 4 --out battery_mini.txt

The gateway must already be running (this script does not start/stop it).
Run in batches of 4 with a fresh session between batches to avoid
context-accumulation compression notices contaminating the replies.
"""
import argparse, asyncio, json, os, sys, websockets

HERE = os.path.dirname(os.path.abspath(__file__))
PROMPTS = json.load(open(os.path.join(HERE, "prompts.json"), encoding="utf-8"))

# Status notices the agent emits as assistant messages but that are NOT real
# responses — skip them and keep reading for the actual reply.
SKIP_SUBSTRINGS = ("Memory threshold", "Optimizing conversation history", "Emergency compression")


async def run(url, start, end, outfile):
    out = open(outfile, "a", encoding="utf-8")
    async with websockets.connect(url, max_size=2**24) as ws:
        for tag, prompt in PROMPTS[start:end]:
            out.write("\n===== %s: %s =====\n" % (tag, prompt))
            out.flush()
            await ws.send(json.dumps({
                "type": "message", "module": "chat", "cmd": "send",
                "data": {"content": prompt},
            }))
            got = False
            while True:
                try:
                    raw = await asyncio.wait_for(ws.recv(), timeout=120)
                except asyncio.TimeoutError:
                    out.write("<< TIMEOUT >>\n"); break
                except websockets.ConnectionClosed:
                    out.write("<< CLOSED >>\n"); break
                try:
                    msg = json.loads(raw)
                    if (msg.get("type") == "message" and msg.get("cmd") == "receive"
                            and msg.get("data", {}).get("role") == "assistant"):
                        content = msg["data"].get("content", "")
                        if any(s in content for s in SKIP_SUBSTRINGS):
                            continue  # compression/status notice — not a real reply
                        out.write("RESPONSE: " + content + "\n")
                        got = True; break
                except Exception:
                    pass
            if not got:
                out.write("(no assistant response)\n")
            out.flush()
    out.close()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", required=True, help='ws://host:port/ws?token=TOKEN')
    ap.add_argument("--start", type=int, default=0)
    ap.add_argument("--end", type=int, default=len(PROMPTS))
    ap.add_argument("--out", default="battery_out.txt")
    a = ap.parse_args()
    asyncio.run(run(a.url, a.start, a.end, a.out))


if __name__ == "__main__":
    main()
