#!/usr/bin/env python3
"""Run bot/homemade.py's Narrated class through lichess-bot's own mock of lichess.org, without a token or a network.

Usage: .tools/lichess-bot/.venv/bin/python tools/hook_smoke.py [--casual]

Without the flag the mock game is rated and between two bots, so sparring must stay off. With --casual the opponent is a
person rated 1472 in a casual game, so the hook must turn sparring on and add its intro line.

It runs from the repository and from an installed package alike (deploy/install.sh), and reads bot/config.yml as
written. lichess-bot's test harness plays a scripted White (Scholar's mate) against the engine. The mock chat call does nothing,
so what this checks is the part that is ours: the class loads by name, finds the engine binary, starts `uci coach`, sends
it one event for each move on either side, and hands each line it says to `li.chat`. Each hand-off is logged as
"Said in N ms". The harness writes the log to a file; this reads it back and prints what it found: the engine's moves,
the lines about the student (they open with "You") and the lines from Symbia (they open with "I", "My" or "Your"),
any line that still has an old tag or is past the chat limit, and any warning. The scripted opponent plays White and
the engine Black, so a game with N engine moves should show up to 2N lines about you (the Game and You lanes) and N
from me; the You lane's lines are dropped when the judge is a move behind.
It ends the process itself, since the scripted opponent stops answering if the engine defends.
"""
import logging
import multiprocessing
import os
import re
import sys
import tempfile
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BOT = ROOT / ".tools" / "lichess-bot"
os.chdir(BOT)
sys.path[:0] = [str(ROOT / "bot"), str(BOT)]  # our homemade.py before lichess-bot's example one

import types  # noqa: E402

import yaml  # noqa: E402

# lichess-bot's harness refuses to import unless pytest is loaded, only so that nobody runs it by accident. We are
# running it on purpose, so give it an empty module of that name.
sys.modules.setdefault("pytest", types.ModuleType("pytest"))
from test_bot import test_bot as harness  # noqa: E402


def make_casual():
    """Replace the mock site's game record with a casual game against a person rated 1472, so that sparring turns on.

    The mock's own game is rated and between two bots, which is the case where she must play her best. Game processes
    started by multiprocessing import this file again, and so they run this too, which is how it reaches the game.
    """
    import json

    from test_bot import lichess as site

    original = site.GameStream.iter_lines

    def iter_lines(self, *args, **kwargs):
        lines = original(self, *args, **kwargs)
        first = json.loads(next(lines))
        first["rated"] = False
        first["white"] = {"id": "bo", "name": "bo", "rating": 1472}  # No title, so a person.
        yield json.dumps(first).encode("utf-8")
        yield from lines

    site.GameStream.iter_lines = iter_lines


if "--casual" in sys.argv:
    make_casual()


def report(log):
    """What the game said, from the log file the harness wrote."""
    from homemade import INTRO, INTRO_EVERY, SPAR_INTRO  # The lines said once per game; they carry no tag.

    text = Path(log).read_text() if Path(log).exists() else ""
    said = re.findall(r"Said in \d+ ms: (.*)", text)
    moves = len(re.findall(r"Source: Engine", text))
    intro = [s for s in said if s in INTRO or s in INTRO_EVERY or s == SPAR_INTRO]
    lines = [s for s in said if s not in intro]
    about_you = [s for s in lines if s.startswith("You ")]  # The Game lane and the You lane both open this way.
    from_me = [s for s in lines if not s.startswith("You ")]  # The Me lane opens with "I", "My" or "Your".
    old_tags = [s for s in lines if s.startswith(("Game: ", "You: ", "Me: "))]
    too_long = [s for s in said if len(s) > 140]
    warnings = [line for line in text.splitlines() if "homemade" in line and " WARNING " in line]
    coach = [
        line.split(" INFO ", 1)[-1]
        for line in text.splitlines()
        if "homemade" in line and ("INFO Coach " in line or "INFO Sparring on" in line)
    ]
    for s in said:
        print("said:", s)
    for line in warnings + coach:
        print("log:", line)
    print(
        f"engine moves {moves}; lines about you {len(about_you)}, lines from me {len(from_me)}; "
        f"intro {len(intro)}; old tags {len(old_tags)}; past 140 characters {len(too_long)}; warnings {len(warnings)}"
    )


def lifecycle():
    """The coach as the hook runs it: events in, lines out through a stand-in chat, and a clean end when it is closed."""
    from homemade import Coach

    class Chat:
        def __init__(self):
            self.lines = []

        def chat(self, game_id, room, line):
            self.lines.append(line)

    start = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1"
    after = "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1"
    chat = Chat()
    coach = Coach(str(ROOT / "target" / "release" / "uci"), chat, "smoke", ["(intro)"])
    coach.send(f"student 1 e2e4 {start}")
    coach.send(f"tutor 2 e7e5 5 cp 20 {after}")
    coach.close()
    print(f"lifecycle: {len(chat.lines)} lines {chat.lines}; the process ended: {coach.process.poll() is not None}")
    # B16: the process is quiet, so e4 (a best move, nothing loose) is a silence, which writes `skip` and posts nothing.
    assert not any(line.startswith("skip") for line in chat.lines), "a skip line reached the chat"
    assert chat.lines == ["(intro)", "I played pawn to e5 to take space in the center."], chat.lines


def bridge():
    """Who gets sparring and what the intro says, on lichess-bot's own Game and Player classes. Stops at a miss."""
    import datetime
    import zlib

    from homemade import INTRO, INTRO_EVERY, SPAR_INTRO, spar_options, voice
    from lib import model

    def game(rated=False, opponent=None, gid="abcd1234"):
        person = {"name": "Brian", "rating": 1472}
        info = {
            "id": gid,
            "rated": rated,
            "variant": {"name": "Standard"},
            "state": {},
            "createdAt": 0,
            "white": person if opponent is None else opponent,
            "black": {"name": "Symbia", "title": "BOT", "rating": 2100},
        }
        return model.Game(info, "Symbia", "https://lichess.org/", datetime.timedelta(seconds=20))

    seed = str(zlib.crc32(b"abcd1234"))
    cases = [
        ("casual, a person rated 1472", game(), {"Sparring": True, "Opponent": 1472, "SparSeed": seed}),
        ("rated, a person", game(rated=True), None),
        ("casual, a bot", game(opponent={"name": "other", "title": "BOT", "rating": 1500}), None),
        ("casual, Lichess's computer", game(opponent={"aiLevel": 3}), None),
        ("casual, no rating", game(opponent={"name": "guest"}), None),
        ("casual, rating 0", game(opponent={"name": "guest", "rating": 0}), None),
        ("casual, rating past the scale", game(opponent={"name": "x", "rating": 4500}),
         {"Sparring": True, "Opponent": 4000, "SparSeed": seed}),
        ("casual, a person with a title", game(opponent={"name": "x", "title": "NM", "rating": 2300}),
         {"Sparring": True, "Opponent": 2300, "SparSeed": seed}),
        ("not a game at all", types.SimpleNamespace(), None),
    ]
    for name, g, want in cases:
        got = spar_options(g)
        assert got == want, f"{name}: wanted {want}, got {got}"
    assert spar_options(game(gid="zzzz9999"))["SparSeed"] != seed, "the seed should follow the game"
    everything = INTRO + INTRO_EVERY + [SPAR_INTRO]
    long = [s for s in everything if len(s) > 140]
    assert not long, f"intro lines past 140 characters: {long}"
    # B16: the voice is quiet unless SYMBIA_VOICE says every, and anything else means quiet.
    saved = os.environ.pop("SYMBIA_VOICE", None)
    try:
        assert voice() == "quiet"
        for value, want in (("every", "every"), (" Every ", "every"), ("quiet", "quiet"), ("loud", "quiet"), ("", "quiet")):
            os.environ["SYMBIA_VOICE"] = value
            assert voice() == want, f"SYMBIA_VOICE={value!r} gave {voice()!r}, wanted {want!r}"
    finally:
        os.environ.pop("SYMBIA_VOICE", None)
        if saved is not None:
            os.environ["SYMBIA_VOICE"] = saved
    print(f"bridge: {len(cases) + 8} checks held; the longest intro line is {max(map(len, everything))} characters")


def main():
    # The real config, as written, so that its relative engine path is the one being tested. Only the token and the
    # place the game record goes are replaced.
    with open(ROOT / "bot" / "config.yml") as f:
        config = yaml.safe_load(f)
    with tempfile.TemporaryDirectory() as temp:
        config["token"] = ""
        config["pgn_directory"] = os.path.join(temp, "games")
        log = os.path.join(temp, "smoke.log")
        harness.testing_log_file_name = log  # The harness passes this name to its logging process.
        lifecycle()
        bridge()
        done = threading.Event()

        def run():
            try:
                harness.run_bot(config, logging.INFO)
            finally:
                done.set()

        threading.Thread(target=run, daemon=True).start()
        done.wait(60)
        print("finished" if done.is_set() else "stopped after 60 s (the scripted opponent had no move left)")
        time.sleep(1)  # The logging process writes the last lines after the game ends.
        report(log)
        sys.stdout.flush()
        # os._exit skips multiprocessing's clean-up, so the harness's helper processes (and through them the engine)
        # would be left running after every smoke test. Stop them first.
        for child in multiprocessing.active_children():
            child.kill()
        os._exit(0)


if __name__ == "__main__":
    main()
