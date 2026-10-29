#!/usr/bin/env python3
"""Measure the engine's speed alone and with the coach judging beside it (B13, prediction 7).

Usage: .tools/lichess-bot/.venv/bin/python tools/beside.py --uci target/release/uci [--searches 5] [--cpus 0,1]

The engine searches the same middlegame position for one second, several times: first alone, then while a second
process, `uci coach`, judges one student move after another without pause, which is the most work the You lane can be
given. The speed is nodes divided by the time the engine reports. With --cpus the engine and the coach are both held
to those cores with taskset (Linux), so `--cpus 0` is the worst case, one core shared, and `--cpus 0,1` is the hosted
machine, which has two.

It needs python-chess, which the lichess-bot virtual environment has.
"""
import argparse
import re
import statistics
import subprocess
import sys
import threading
import time
from pathlib import Path

import chess

ROOT = Path(__file__).resolve().parent.parent
MIDDLEGAME = "r1bq1rk1/ppp2ppp/2np1n2/2b1p3/2B1P3/2NP1N2/PPP2PPP/R1BQ1RK1 w - - 0 7"
INFO = re.compile(r"info .*?nodes (\d+).*?time (\d+)")


def start(command, cpus):
    prefix = ["taskset", "-c", cpus] if cpus else []
    return subprocess.Popen(prefix + command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)


def search(engine, movetime_ms):
    """Nodes per second of one search, from the last info line the engine printed."""
    engine.stdin.write(f"position fen {MIDDLEGAME}\ngo movetime {movetime_ms}\n")
    engine.stdin.flush()
    last = None
    for line in engine.stdout:
        found = INFO.search(line)
        if found:
            last = found
        if line.startswith("bestmove"):
            break
    nodes, millis = int(last.group(1)), max(1, int(last.group(2)))
    return nodes * 1000 / millis


def events(moves_path):
    """(fen before, move) for every move of the sample games, over and over."""
    games = [line.split() for line in Path(moves_path).read_text().splitlines() if line.strip()]
    while True:
        for moves in games:
            board = chess.Board()
            for move in moves:
                yield board.fen(), move
                board.push_uci(move)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--uci", required=True)
    parser.add_argument("--searches", type=int, default=5)
    parser.add_argument("--movetime", type=int, default=1000)
    parser.add_argument("--cpus", help="hold both processes to these cores with taskset, e.g. 0 or 0,1")
    parser.add_argument("--moves", default=str(ROOT / "tests" / "fixtures" / "games_sample.moves"))
    args = parser.parse_args()

    engine = start([args.uci], args.cpus)
    engine.stdin.write("uci\nisready\n")
    engine.stdin.flush()
    for line in engine.stdout:
        if line.startswith("readyok"):
            break
    search(engine, 200)  # warm up
    alone = [search(engine, args.movetime) for _ in range(args.searches)]

    coach = start([args.uci, "coach"], args.cpus)
    done = threading.Event()
    judged = []

    def feed():
        ply = 1
        for fen, move in events(args.moves):
            if done.is_set():
                break
            coach.stdin.write(f"student {ply} {move} {fen}\n")
            coach.stdin.flush()
            for line in coach.stdout:
                if line.startswith(("say ", "drop ")) and " you " in line[:14]:
                    break
            judged.append(time.monotonic())
            ply += 2

    feeder = threading.Thread(target=feed, daemon=True)
    feeder.start()
    time.sleep(1.0)  # let the coach get busy
    beside = [search(engine, args.movetime) for _ in range(args.searches)]
    done.set()
    feeder.join(timeout=10)
    coach.stdin.close()
    coach.wait(timeout=10)
    engine.stdin.write("quit\n")
    engine.stdin.flush()
    engine.wait(timeout=5)

    a, b = statistics.median(alone), statistics.median(beside)
    print(f"alone:  {a / 1e6:.3f} Mnps (median of {len(alone)}; " + ", ".join(f"{x / 1e6:.2f}" for x in alone) + ")")
    print(f"beside: {b / 1e6:.3f} Mnps (median of {len(beside)}; " + ", ".join(f"{x / 1e6:.2f}" for x in beside) + ")")
    print(f"beside is {100 * b / a:.1f}% of alone; the coach judged {len(judged)} moves meanwhile"
          + (f", on cores {args.cpus}" if args.cpus else ""))
    return 0


if __name__ == "__main__":
    sys.exit(main())
