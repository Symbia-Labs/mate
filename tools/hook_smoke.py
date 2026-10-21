#!/usr/bin/env python3
"""Run bot/homemade.py's Narrated class through lichess-bot's own mock of lichess.org, without a token or a network.

Usage: .tools/lichess-bot/.venv/bin/python tools/hook_smoke.py

It runs from the repository and from an installed package alike (deploy/install.sh), and reads bot/config.yml as
written. lichess-bot's test harness plays a scripted White (Scholar's mate) against the engine. The mock chat call does nothing,
so what this checks is the part that is ours: the class loads by name, finds the engine binary, starts it with the
Narrate option, and for every move the engine makes, reads the sentence out of the move's comment and hands it to
`li.chat`. Each hand-off is logged as "Said in N ms"; this prints those lines and the count of engine moves.
It ends the process itself, since the scripted opponent stops answering if the engine defends.
"""
import logging
import multiprocessing
import os
import sys
import tempfile
import threading
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


def main():
    # The real config, as written, so that its relative engine path is the one being tested. Only the token and the
    # place the game record goes are replaced.
    with open(ROOT / "bot" / "config.yml") as f:
        config = yaml.safe_load(f)
    with tempfile.TemporaryDirectory() as temp:
        config["token"] = ""
        config["pgn_directory"] = os.path.join(temp, "games")
        done = threading.Event()

        def run():
            try:
                harness.run_bot(config, logging.INFO)
            finally:
                done.set()

        threading.Thread(target=run, daemon=True).start()
        done.wait(60)
        print("finished" if done.is_set() else "stopped after 60 s (the scripted opponent had no move left)")
        sys.stdout.flush()
        # os._exit skips multiprocessing's clean-up, so the harness's helper processes (and through them the engine)
        # would be left running after every smoke test. Stop them first.
        for child in multiprocessing.active_children():
            child.kill()
        os._exit(0)


if __name__ == "__main__":
    main()
