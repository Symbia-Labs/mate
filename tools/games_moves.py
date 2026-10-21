#!/usr/bin/env python3
"""Write tests/fixtures/games_sample.moves from tests/fixtures/games_sample.pgn: one line per game, the moves in UCI.

Usage: .tools/lichess-bot/.venv/bin/python tools/games_moves.py

tools/fingerprint.py reads this file, so it needs nothing but the standard library. This needs python-chess, which the
lichess-bot environment already has. Run it again only if the PGN changes.
"""
from pathlib import Path

import chess.pgn

ROOT = Path(__file__).resolve().parent.parent
PGN = ROOT / "tests" / "fixtures" / "games_sample.pgn"
OUT = ROOT / "tests" / "fixtures" / "games_sample.moves"


def main():
    lines = []
    with open(PGN) as f:
        while (game := chess.pgn.read_game(f)) is not None:
            if game.headers.get("FEN") or game.headers.get("Variant", "Standard") != "Standard":
                raise SystemExit("only standard games from the starting position can be listed")
            lines.append(" ".join(move.uci() for move in game.mainline_moves()))
    OUT.write_text("\n".join(lines) + "\n")
    plies = sum(len(line.split()) for line in lines)
    print(f"{len(lines)} games, {plies} plies -> {OUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
