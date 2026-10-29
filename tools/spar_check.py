#!/usr/bin/env python3
"""Checks that the sparring chooser acts only where it should, from two fingerprints of the same positions.

Usage: python3 tools/spar_check.py --uci target/release/uci OFF.tsv ON.tsv [--rating 1300] [--seed 1] [--depth 5]

OFF.tsv and ON.tsv come from tools/fingerprint.py with Sparring off and on (Opponent and SparSeed as given here). It
reports, and exits 1 if any of these is false:
  1. every changed move was in a position past half-move 10, at least +100 centipawns for her and not a forced mate;
  2. the score printed for a changed move is not below 0 and is within the limit of the full-strength score;
  3. in the first changed positions, a clock under 20 seconds gives the full-strength move and a long clock gives the
     chosen one (the chooser is off when she is short of time).
Needs only the Python standard library.
"""
import argparse
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from fingerprint import MOVES, Engine  # noqa: E402

LIMITS = [(2000, 0), (1800, 50), (1600, 80), (1400, 120), (1200, 160), (1000, 200), (1, 250)]


def limit_for(rating):
    return next((limit for floor, limit in LIMITS if rating >= floor), 100)


def cp(text):
    m = re.fullmatch(r"cp (-?\d+)", text)
    return int(m.group(1)) if m else None


def best_with_clock(engine, moves, depth, millis):
    engine.send("position startpos" + (f" moves {' '.join(moves)}" if moves else ""))
    engine.send(f"go depth {depth} wtime {millis} btime {millis}")
    lines = engine.until("bestmove")
    return lines[-1].split()[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--uci", required=True)
    parser.add_argument("off")
    parser.add_argument("on")
    parser.add_argument("--rating", type=int, default=1300)
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--depth", type=int, default=5)
    parser.add_argument("--moves", default=str(MOVES))
    args = parser.parse_args()

    off = [l.split("\t") for l in Path(args.off).read_text().splitlines()]
    on = [l.split("\t") for l in Path(args.on).read_text().splitlines()]
    games = [line.split() for line in Path(args.moves).read_text().splitlines() if line.strip()]
    limit = limit_for(args.rating)
    changed = [i for i in range(len(off)) if off[i][2] != on[i][2]]
    problems = []
    for i in changed:
        ply = int(off[i][1])
        before, after = cp(off[i][4]), cp(on[i][4])
        if ply < 10:
            problems.append(f"row {i}: changed at half-move {ply}")
        if before is None or before <= 100:
            problems.append(f"row {i}: changed with a full-strength score of {off[i][4]}")
        elif after is None or after < 0 or before - after > limit:
            problems.append(f"row {i}: score {off[i][4]} became {on[i][4]}, limit {limit}")
    print(f"{len(changed)} of {len(off)} moves changed; limit {limit}; problems in the board checks: {len(problems)}")

    engine = Engine(args.uci, [("Narrate", "true"), ("Sparring", "true"), ("Opponent", str(args.rating)),
                               ("SparSeed", str(args.seed))])
    short = long = 0
    for i in changed[:5]:
        g, ply = int(off[i][0]), int(off[i][1])
        moves = games[g][:ply]
        quick = best_with_clock(engine, moves, args.depth, 15000)
        slow = best_with_clock(engine, moves, args.depth, 120000)
        short += quick == off[i][2]
        long += slow == on[i][2]
        if quick != off[i][2]:
            problems.append(f"row {i}: with 15 s on the clock she played {quick}, full strength is {off[i][2]}")
        if slow != on[i][2]:
            problems.append(f"row {i}: with 120 s on the clock she played {slow}, the fingerprint says {on[i][2]}")
    engine.quit()
    n = min(5, len(changed))
    print(f"clock check on {n} positions: full-strength move with 15 s {short}/{n}; chosen move with 120 s {long}/{n}")
    for p in problems[:20]:
        print("PROBLEM", p)
    sys.exit(1 if problems else 0)


if __name__ == "__main__":
    main()
