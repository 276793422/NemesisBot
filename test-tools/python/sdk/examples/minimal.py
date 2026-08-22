"""Minimal example: 3 lines to a working agent.

Prerequisites:
    - nemesisbot(.exe) on PATH (or pass binary=...)
    - a configured model in <workspace>/config.json (nemesisbot model add ...)

Run:
    python examples/minimal.py "What files are in the workspace?"
"""

import sys

from nemesisbot import NemesisBot

WORKSPACE = "./sdk_example_workspace"


def main() -> None:
    prompt = (
        sys.argv[1]
        if len(sys.argv) > 1
        else "List the files in this directory and say hi."
    )
    with NemesisBot(workspace=WORKSPACE) as bot:
        reply = bot.send(prompt)
        print(reply)


if __name__ == "__main__":
    main()
