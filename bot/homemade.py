"""The engine class lichess-bot loads for this bot (config: engine.protocol "homemade", engine.name "Narrated").

tools/bot.sh copies this file beside lichess-bot.py, which looks for homemade.py there. lichess-bot runs each game in
its own process, so the hook has to be a class it loads by name; a patch applied at start-up would not reach the games.
"""
import logging
import os
import threading
import time

from lib.engine_wrapper import MinimalEngine, UCIEngine

logger = logging.getLogger(__name__)


class ExampleEngine(MinimalEngine):
    """Kept because lichess-bot loads its own test engines, and they import this name from here.

    Without it every game fails to start: get_homemade_engine imports those test engines before it looks up ours.
    """

SAY = "say: "  # What the engine puts at the start of its `info string` sentence.
BAR = " | "  # Between the sentence about the opponent's move and the one about the engine's own.
ROOM = "player"  # The chat the opponent sees. Spectators have their own room.
INTRO = [  # Said once per game, after the greeting in config.yml and before the first move's sentences. 140 characters a line at most.
    "My evaluation is a graph of small, readable blocks. After each move I say what I see: captures, checks, mates and loose pieces.",
    "I comment on your moves too. I also say who is ahead and how many moves I looked ahead. More at https://symbia-labs.com",
]


def post(li, game_id, lines):
    """Send each line to the chat in order, and log how long each call took. A failure stops the rest, quietly."""
    for line in lines:
        started = time.monotonic()
        try:
            li.chat(game_id, ROOM, line)
        except Exception as error:  # A chat failure must never cost a game.
            logger.warning(f"Could not post the sentence: {error}")
            return
        logger.info(f"Said in {round((time.monotonic() - started) * 1000)} ms: {line}")


class Narrated(UCIEngine):
    """The chess engine over UCI, plus its sentences about each move posted to the game chat."""

    poster = None
    introduced = False  # Each game builds its own engine, so this is once per game.

    def __init__(self, commands, *args, **kwargs):
        # lichess-bot builds the path from engine.dir and engine.name, and the name here is this class.
        binary = os.path.abspath(os.path.join(os.path.dirname(commands[0]), "uci"))
        super().__init__([binary, *commands[1:]], *args, **kwargs)

    def play_move(self, board, game, li, *args, **kwargs):
        before = len(self.move_commentary)
        super().play_move(board, game, li, *args, **kwargs)  # sends the move
        if len(self.move_commentary) <= before:
            return  # no move was made by the engine, so nothing to say
        text = str(self.move_commentary[-1].get("string", ""))
        if not text.startswith(SAY):
            return
        lines = text[len(SAY):].split(BAR)
        if not self.introduced:
            self.introduced = True
            lines = INTRO + lines
        # From another thread, so the next move never waits on a chat call.
        self.poster = threading.Thread(target=post, args=(li, game.id, lines), daemon=True)
        self.poster.start()

    def quit(self):
        if self.poster is not None:
            self.poster.join(timeout=5)  # let the last sentences out before the engine goes
        super().quit()
