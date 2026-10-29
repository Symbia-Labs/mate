"""The engine class lichess-bot loads for this bot (config: engine.protocol "homemade", engine.name "Narrated").

tools/bot.sh copies this file beside lichess-bot.py, which looks for homemade.py there. lichess-bot runs each game in
its own process, so the hook has to be a class it loads by name; a patch applied at start-up would not reach the games.
"""
import contextlib
import logging
import os
import subprocess
import threading
import time
import zlib

from lib.engine_wrapper import MinimalEngine, UCIEngine

logger = logging.getLogger(__name__)


class ExampleEngine(MinimalEngine):
    """Kept because lichess-bot loads its own test engines, and they import this name from here.

    Without it every game fails to start: get_homemade_engine imports those test engines before it looks up ours.
    """

ROOM = "player"  # The chat the opponent sees. Spectators have their own room.
INTRO = [  # Said once per game, after the greeting in config.yml and before the first move's sentences. 140 characters a line at most.
    "I only speak up when something happens, and I say why. My evaluation is a graph of small, readable blocks.",
    "How good your move was comes from my own search, a few moves deep. More at https://symbia-labs.com",
]
INTRO_EVERY = [  # The B15 voice, for SYMBIA_VOICE=every: a sentence for every move.
    "My evaluation is a graph of small, readable blocks. After each move I say what yours did, how good it was, and what I played.",
    "How good your move was comes from my own search, a few moves deep. More at https://symbia-labs.com",
]
SPAR_INTRO = "This game is casual, so I play near your level and aim to end level or one pawn up. Rated games get my best."
# Said only when sparring is on, between the two lines above. The same 140 characters a line at most.


def voice():
    """How much she says: "quiet" (the default, B16) or "every" (a line on every move, B15). Set by SYMBIA_VOICE."""
    word = os.environ.get("SYMBIA_VOICE", "").strip().lower()
    return word if word in ("every", "quiet") else "quiet"


def spar_options(game):
    """The engine options for sparring in this game, or None when she should play her best.

    Sparring is for casual games against a person with a rating. A rated game, a bot, or a rating that cannot be read
    gets her best, because there she has no reason to hold back and nothing to measure a level by.
    """
    try:
        if game.mode == "rated" or game.opponent.is_bot:
            return None
        rating = game.opponent.rating
        if isinstance(rating, bool) or not isinstance(rating, int) or rating <= 0:
            return None
        seed = zlib.crc32(str(game.id).encode())
    except AttributeError:
        return None
    return {"Sparring": True, "Opponent": min(rating, 4000), "SparSeed": str(seed)}


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


class Coach:
    """The commentary process, `uci coach`, and the one thread that posts what it says.

    The hook writes one line per event and returns; it never waits on the process. The process runs the Game, You and
    Me lanes (src/coach.rs) and prints a line for each sentence it will say, tagged and short enough for the chat.
    This thread is the only one that talks to the chat, so two lines never overlap and each lane stays in order.
    """

    def __init__(self, binary, li, game_id, intro, how=None):
        self.process = subprocess.Popen([binary, "coach", f"voice={how or voice()}"], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, text=True, bufsize=1)
        self.reader = threading.Thread(target=self.speak, args=(li, game_id, intro), daemon=True)
        self.reader.start()

    def send(self, line):
        """One event for the lanes. If the process is gone, the game goes on without commentary."""
        try:
            self.process.stdin.write(line + "\n")
            self.process.stdin.flush()
        except (OSError, ValueError) as error:
            logger.warning(f"The coach did not take an event: {error}")

    def speak(self, li, game_id, intro):
        post(li, game_id, intro)
        for line in self.process.stdout:
            kind, _, rest = line.rstrip("\n").partition(" ")
            fields = rest.split(" ", 2)  # say <ply> <lane> <text>
            if kind == "say" and len(fields) == 3:
                post(li, game_id, [fields[2]])
            elif kind == "skip":
                logger.debug(f"Coach {line.rstrip()}")  # a lane with nothing to say; nothing is posted
            else:
                logger.info(f"Coach {line.rstrip()}")  # drops and notes, for the daily review

    def close(self, wait=5):
        """Let the lines still waiting out, then stop the process."""
        self.send("quit")
        with contextlib.suppress(OSError, ValueError):
            self.process.stdin.close()
        self.reader.join(timeout=wait)
        try:
            self.process.wait(timeout=1)
        except subprocess.TimeoutExpired:
            self.process.kill()


class Narrated(UCIEngine):
    """The chess engine over UCI. Each move is an event for the coach, and the coach's lines go to the game chat."""

    coach = None  # Started at the first move, so a game that never starts costs nothing.
    ply = 0  # The highest half-move the coach has been told of.

    def __init__(self, commands, *args, **kwargs):
        # lichess-bot builds the path from engine.dir and engine.name, and the name here is this class.
        self.binary = os.path.abspath(os.path.join(os.path.dirname(commands[0]), "uci"))
        super().__init__([self.binary, *commands[1:]], *args, **kwargs)

    def play_move(self, board, game, li, *args, **kwargs):
        position = board.fen()
        try:
            if self.coach is None:
                self.begin(game, li)
            self.hear_student(board)
        except Exception as error:  # Commentary must never cost a game.
            logger.warning(f"The coach did not get the student's move: {error}")
        commented = len(self.move_commentary)
        super().play_move(board, game, li, *args, **kwargs)  # searches, then sends the move
        try:
            if self.coach is not None and len(self.move_commentary) > commented:
                self.hear_tutor(position, board)
        except Exception as error:
            logger.warning(f"The coach did not get the engine's move: {error}")

    def begin(self, game, li):
        """Once per game: set the sparring options when they apply, then start the coach with the intro to match."""
        how = voice()
        intro = list(INTRO_EVERY if how == "every" else INTRO)
        options = spar_options(game)
        if options is not None:
            try:
                # Straight to python-chess: EngineWrapper.configure would close the engine if an option were refused.
                self.engine.configure(options)
                intro.insert(len(intro) - 1, SPAR_INTRO)
                logger.info(f"Sparring on: opponent {options['Opponent']}, seed {options['SparSeed']}")
            except Exception as error:  # Without the options she plays her best, which is safe.
                logger.warning(f"Could not set the sparring options, playing my best: {error}")
        self.coach = Coach(self.binary, li, game.id, intro, how)

    def hear_student(self, board):
        """The move that was just played against the engine, with the position it was played from."""
        played = len(board.move_stack)
        if played == 0:
            return  # The engine has White and the first move.
        before = board.copy()
        move = before.pop()
        self.coach.send(f"student {played} {move.uci()} {before.fen()}")
        self.ply = played

    def hear_tutor(self, position, board):
        """The engine's own move, from the search's last report: its depth, its score and the first move of its line."""
        info = self.move_commentary[-1]
        line = info.get("pv") or []
        score = info.get("score")
        if not line or score is None:
            return  # A move without a report from the search has nothing to say.
        relative = score.relative
        mate = relative.mate()
        said = f"mate {mate}" if mate is not None else f"cp {relative.score()}"
        played = len(board.move_stack) + 1
        self.coach.send(f"tutor {played} {line[0].uci()} {info.get('depth', 0)} {said} {position}")
        self.ply = played

    def discard_last_move_commentary(self):
        """A takeback undoes the student's last move and the engine's answer to it."""
        super().discard_last_move_commentary()
        if self.coach is not None and self.ply >= 1:
            self.coach.send(f"takeback {max(1, self.ply - 1)}")
            self.ply = max(0, self.ply - 2)

    def quit(self):
        if self.coach is not None:
            self.coach.close()  # lets the last sentences out before the engine goes
        super().quit()
