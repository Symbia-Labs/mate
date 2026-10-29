#!/usr/bin/env python3
"""Write the coach events of whole games, the way tools/quiet_stats.py sends them, so another runner can replay them.

Usage: .tools/lichess-bot/.venv/bin/python tools/game_events.py [--games matches/symbia_lichess_games.ndjson]
           [--cache matches/b16_tutor_cache.json] [--depth 5] [--out matches/game_events.txt]

One `G <index>` line starts each game (the index quiet_stats.py gives it), then one line per half-move:
`tutor <ply> <uci> <depth> <cp|mate> <value> <fen>` for Symbia's moves and `student <ply> <uci> <fen>` for the other
side's, the FEN being the position before the move. The tutor's depth and score come from the cache quiet_stats.py
filled, so both runners see the same numbers. A position missing from the cache stops the script.
"""
import argparse
import json
import sys
from pathlib import Path

import chess

sys.path.insert(0, str(Path(__file__).resolve().parent))
from quiet_stats import load_games  # noqa: E402


def main():
    root = Path(__file__).resolve().parent.parent
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--games", default=str(root / "matches" / "symbia_lichess_games.ndjson"))
    p.add_argument("--cache", default=str(root / "matches" / "b16_tutor_cache.json"))
    p.add_argument("--depth", type=int, default=5)
    p.add_argument("--out", default=str(root / "matches" / "game_events.txt"))
    a = p.parse_args()
    cache = json.loads(Path(a.cache).read_text())
    games = load_games(a.games, 0)
    with open(a.out, "w") as f:
        for gi, g in enumerate(games):
            f.write(f"G {gi}\n")
            board = chess.Board()
            for ply, san in enumerate(g["san"], start=1):
                move = board.parse_san(san)
                fen = board.fen()
                if board.turn == g["tutor"]:
                    d, kind, value = cache[f"{a.depth} {fen}"]
                    f.write(f"tutor {ply} {move.uci()} {d} {kind} {value} {fen}\n")
                else:
                    f.write(f"student {ply} {move.uci()} {fen}\n")
                board.push(move)
    print(f"{len(games)} games written to {a.out}")


if __name__ == "__main__":
    main()
