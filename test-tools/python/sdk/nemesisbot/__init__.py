"""NemesisBot Python SDK.

Drive a NemesisBot agent as a subprocess: spawn `nemesisbot --local gateway`
in a workspace you choose, then exchange chat turns over its WebSocket.

Example (one-shot, 3 lines — turn() owns the gateway lifecycle):

    from nemesisbot import NemesisBot

    bot = NemesisBot(workspace="./mybot")
    print(bot.turn("List the files in this directory."))

Example (multi-turn, long-lived gateway):

    with NemesisBot(workspace="./mybot") as bot:
        print(bot.send("hello"))
"""

from .client import NemesisBot, NemesisBotError

__all__ = ["NemesisBot", "NemesisBotError"]
__version__ = "0.1.0"
