#!/usr/bin/env python3
"""Check the engine's sentence about each played move against python-chess, which shares no code with the engine.

Usage: .tools/lichess-bot/.venv/bin/python tools/narrate_check.py [PGN] [--show N]
(default tests/fixtures/games_sample.pgn; --show prints the first N sentences so they can be read)

For every move of every game it asks the real `uci` binary (Narrate on) for the position after that move, takes the
first half of the `info string say:` line (the sentence about the move just played), and works out from python-chess
what is true of that move: the capture and who took what, the piece and square moved to, castling, promotion, check,
and which pieces of each side are loose before and after. "Loose" is the loose block's definition, written out again
here (as in tools/loose_fixture.py): a knight, bishop, rook or queen that is attacked and either undefended or
attacked by a cheaper piece.

It reports two things. Claims: every claim in a sentence must be true (precision). Recall: when the mover has a piece
that became loose with the move, the sentence must name the most valuable one. Exits 1 if any claim is false.
"""
import re
import subprocess
import sys
from pathlib import Path

import chess
import chess.pgn

ROOT = Path(__file__).resolve().parent.parent
ENGINE = ROOT / "target" / "release" / "uci"
VALUE = {chess.PAWN: 100, chess.KNIGHT: 320, chess.BISHOP: 330, chess.ROOK: 500, chess.QUEEN: 900, chess.KING: 10000}
NAME = {chess.PAWN: "pawn", chess.KNIGHT: "knight", chess.BISHOP: "bishop", chess.ROOK: "rook", chess.QUEEN: "queen", chess.KING: "king"}
SQ = r"[a-h][1-8]"


def loose(board, side):
    out = set()
    for piece in (chess.KNIGHT, chess.BISHOP, chess.ROOK, chess.QUEEN):
        for sq in board.pieces(piece, side):
            attackers = board.attackers(not side, sq)
            if not attackers:
                continue
            if not board.attackers(side, sq):
                out.add(sq)
            elif min(VALUE[board.piece_type_at(a)] for a in attackers) < VALUE[piece]:
                out.add(sq)
    return out


def named(board, squares):
    """The pieces a list clause should name: the two most valuable, then a count of the rest."""
    found = sorted(((VALUE[board.piece_type_at(s)], s) for s in squares), key=lambda t: (-t[0], t[1]))
    return [(NAME[board.piece_type_at(s)], chess.square_name(s)) for _, s in found[:2]], max(0, len(found) - 2)


def parse_list(text):
    """"queen on d6, rook on f5 and 2 more" -> ([("queen", "d6"), ("rook", "f5")], 2)."""
    items, more = [], 0
    for part in re.split(r", | and ", text):
        m = re.fullmatch(rf"(\w+) on ({SQ})", part)
        if m:
            items.append((m.group(1), m.group(2)))
            continue
        m = re.fullmatch(r"(\d+) more", part)
        if m:
            more = int(m.group(1))
            continue
        raise ValueError(f"cannot read list item {part!r}")
    return items, more


def claims_of(sentence):
    """Every claim in a sentence, as tuples. Raises ValueError for a sentence outside the grammar."""
    out = []
    rest = sentence
    m = re.match(rf"You took my (loose )?(\w+) on ({SQ}) with your (\w+?)(?: and promoted to a (\w+))?\.\s*", rest)
    if m:
        out.append(("capture", m.group(2), m.group(3), m.group(4), bool(m.group(1)), m.group(5)))
    elif (m := re.match(r"You took my pawn en passant\.\s*", rest)):
        out.append(("en passant",))
    elif (m := re.match(r"You castled (kingside|queenside)\.\s*", rest)):
        out.append(("castle", m.group(1)))
    elif (m := re.match(rf"You promoted to a (\w+) on ({SQ})\.\s*", rest)):
        out.append(("promotion", m.group(1), m.group(2)))
    elif (m := re.match(rf"You moved your (\w+) to ({SQ})\.\s*", rest)):
        out.append(("move", m.group(1), m.group(2)))
    else:
        raise ValueError("no main clause")
    rest = rest[m.end():]
    while rest:
        if (m := re.match(r"It gives check\.\s*", rest)):
            out.append(("check",))
        elif (m := re.match(r"That leaves your (.+?) loose\.\s*", rest)):
            out.append(("leaves", *parse_list(m.group(1))))
        elif (m := re.match(r"It attacks my (.+?), which (?:is|are) loose\.\s*", rest)):
            out.append(("attacks", *parse_list(m.group(1))))
        elif (m := re.match(r"Your (.+?) (?:is|are) still loose\.\s*", rest)):
            out.append(("still", *parse_list(m.group(1))))
        else:
            raise ValueError(f"cannot read clause at {rest!r}")
        rest = rest[m.end():]
    return out


def judge(b0, move, claims):
    """The claims that are false, as strings."""
    you = b0.turn
    b1 = b0.copy()
    b1.push(move)
    wrong = []
    mapped = {move.to_square if s == move.from_square else s for s in loose(b0, you)}
    new_yours = loose(b1, you) - mapped
    old_yours = loose(b1, you) & mapped
    mine_before = loose(b0, not you)
    new_mine = loose(b1, not you) - mine_before
    is_capture = b0.is_capture(move)
    for c in claims:
        kind = c[0]
        if kind == "capture":
            _, taken, sq, taker, was_loose, promo = c
            ok = (is_capture and not b0.is_en_passant(move) and chess.square_name(move.to_square) == sq
                  and NAME[b0.piece_type_at(move.to_square)] == taken
                  and NAME[b0.piece_type_at(move.from_square)] == taker
                  and was_loose == (move.to_square in mine_before)
                  and promo == (NAME[move.promotion] if move.promotion else None))
        elif kind == "en passant":
            ok = b0.is_en_passant(move)
        elif kind == "castle":
            ok = b0.is_castling(move) and c[1] == ("kingside" if chess.square_file(move.to_square) > 4 else "queenside")
        elif kind == "promotion":
            ok = (not is_capture and move.promotion is not None and NAME[move.promotion] == c[1]
                  and chess.square_name(move.to_square) == c[2])
        elif kind == "move":
            ok = (not is_capture and not b0.is_castling(move) and move.promotion is None
                  and NAME[b0.piece_type_at(move.from_square)] == c[1] and chess.square_name(move.to_square) == c[2])
        elif kind == "check":
            ok = b1.is_check()
        else:
            squares = {"leaves": new_yours, "attacks": new_mine, "still": old_yours}[kind]
            owner = you if kind in ("leaves", "still") else (not you)
            items, more = c[1], c[2]
            want, want_more = named(b1, squares)
            ok = (items == want and more == want_more
                  and all(b1.piece_type_at(chess.parse_square(s)) is not None
                          and b1.color_at(chess.parse_square(s)) == owner for _, s in items))
        if not ok:
            wrong.append(repr(c))
    # A claim missing is not a false claim, but a check that is not announced, or a capture announced as a move, is.
    if b1.is_check() and ("check",) not in claims:
        wrong.append("check not announced")
    return wrong, new_yours, b1


def main():
    args = sys.argv[1:]
    show = int(args[args.index("--show") + 1]) if "--show" in args else 0
    if "--show" in args:
        del args[args.index("--show"):args.index("--show") + 2]
    pgn = Path(args[0]) if args else ROOT / "tests" / "fixtures" / "games_sample.pgn"
    engine = subprocess.Popen([str(ENGINE)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
    engine.stdin.write("setoption name Narrate value true\n")

    def sentence_after(moves):
        engine.stdin.write(f"position startpos moves {' '.join(moves)}\ngo depth 1\n")
        engine.stdin.flush()
        said = ""
        while True:
            line = engine.stdout.readline().strip()
            if not line:
                sys.exit("the engine closed early")
            if line.startswith("info string say: "):
                said = line[len("info string say: "):]
            elif line.startswith("bestmove "):
                return said.split(" | ")[0]

    moves_checked = claims_checked = false_claims = recall_cases = recall_hits = too_long = 0
    problems = []
    with open(pgn, encoding="utf-8") as f:
        while (game := chess.pgn.read_game(f)) is not None:
            board = game.board()
            played = []
            for move in game.mainline_moves():
                b0 = board.copy()
                board.push(move)
                played.append(move.uci())
                text = sentence_after(played)
                moves_checked += 1
                if moves_checked <= show:
                    side = "w" if b0.turn else "b"
                    print(f"{(len(played) + 1) // 2}{side} {b0.san(move):<7} {text}")
                if len(text) > 140:
                    too_long += 1
                try:
                    claims = claims_of(text)
                except ValueError as error:
                    false_claims += 1
                    problems.append(f"{game.headers['Site'][-8:]} {len(played)}: {error}: {text}")
                    continue
                wrong, new_yours, b1 = judge(b0, move, claims)
                claims_checked += len(claims)
                if wrong:
                    false_claims += len(wrong)
                    problems.append(f"{game.headers['Site'][-8:]} {len(played)}: {wrong}: {text}")
                if new_yours:
                    recall_cases += 1
                    top, _ = named(b1, new_yours)
                    leaves = [c for c in claims if c[0] == "leaves"]
                    if leaves and leaves[0][1] and leaves[0][1][0] == top[0]:
                        recall_hits += 1
                    else:
                        problems.append(f"{game.headers['Site'][-8:]} {len(played)}: most valuable new loose piece {top[0]} not named: {text}")
    engine.stdin.write("quit\n")
    engine.stdin.flush()
    engine.wait()
    print(f"moves checked      {moves_checked}")
    print(f"claims checked     {claims_checked}")
    print(f"false claims       {false_claims}")
    print(f"sentences > 140    {too_long}")
    pct = 100 * recall_hits / recall_cases if recall_cases else 100.0
    print(f"newly loose named  {recall_hits} of {recall_cases} ({pct:.1f}%)")
    for p in problems[:25]:
        print("  ", p)
    sys.exit(1 if false_claims else 0)


if __name__ == "__main__":
    main()
