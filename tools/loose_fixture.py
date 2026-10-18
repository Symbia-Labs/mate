#!/usr/bin/env python3
"""Writes a fixture for the loose_matches_the_detector gate from one game in a PGN file.

  python3 tools/loose_fixture.py <file.pgn> <GameId> > tests/fixtures/loose_<GameId>.json

Same definition as the loose block: a knight, bishop, rook or queen that is attacked and either undefended
(charged 0.5 of its value) or attacked by a cheaper piece (charged 0.5 of the difference). Attack maps, not
legal moves, so a pinned attacker still counts. Needs python-chess.
"""
import json
import sys

import chess
import chess.pgn

VALUE = {chess.PAWN: 100, chess.KNIGHT: 320, chess.BISHOP: 330, chess.ROOK: 500, chess.QUEEN: 900, chess.KING: 10000}


def loose(board, side):
    out = []
    for p in (chess.KNIGHT, chess.BISHOP, chess.ROOK, chess.QUEEN):
        for sq in board.pieces(p, side):
            attackers = board.attackers(not side, sq)
            if not attackers:
                continue
            v = VALUE[p]
            if not board.attackers(side, sq):
                out.append([chess.square_name(sq), v * 0.5])
                continue
            cheapest = min(VALUE[board.piece_type_at(a)] for a in attackers)
            if cheapest < v:
                out.append([chess.square_name(sq), (v - cheapest) * 0.5])
    return sorted(out)


path, want = sys.argv[1], sys.argv[2]
with open(path, encoding="utf-8") as f:
    while (g := chess.pgn.read_game(f)) is not None:
        if g.headers.get("GameId") != want:
            continue
        b = g.board()
        plies = [{"fen": b.fen(), "white": loose(b, chess.WHITE), "black": loose(b, chess.BLACK)}]
        for mv in g.mainline_moves():
            b.push(mv)
            plies.append({"fen": b.fen(), "white": loose(b, chess.WHITE), "black": loose(b, chess.BLACK)})
        json.dump({"game": want, "plies": plies}, sys.stdout)
        sys.exit(0)
sys.exit(f"no game {want} in {path}")
