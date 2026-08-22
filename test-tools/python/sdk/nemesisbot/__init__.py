"""NemesisBot Python SDK.

Drive a NemesisBot agent as a subprocess: spawn `nemesisbot --local gateway`
in a workspace you choose, then exchange chat turns over its WebSocket.

Example:
    from nemesisbot import NemesisBot

    with NemesisBot(workspace="./mybot") as bot:
        reply = bot.send("List the files in this directory.")
        print(reply)
"""

from .client import NemesisBot, NemesisBotError

__all__ = ["NemesisBot", "NemesisBotError"]
__version__ = "0.1.0"
