#!/usr/bin/env python3
"""Show what the coach says in a played game: replay the moves through `uci coach` and print every line.

Usage: .tools/lichess-bot/.venv/bin/python tools/game_lines.py --uci target/release/uci --tutor white|black MOVES

MOVES is a file with the game's moves in SAN on one line (Lichess's export gives them in that form). --tutor says which
side the engine played; the other side is the student. For each of the engine's moves it searches the position to the
given depth (default 5, the depth the fingerprint uses) for the depth and score the bot's hook would have sent, then
sends the same events the hook sends: `tutor` for its moves, `student` for the other side's, in game order. The lines
come back as the coach wrote them, with the lane and the move they are about. It prints them and counts, per lane, how
many moves got a line and how many lines open the same way, which is a quick read on how repetitive a game sounded.
It needs python-chess, which the lichess-bot virtual environment has.
"""
import argparse
import collections
import subprocess
import sys
from pathlib import Path

import chess
import chess.engine


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--uci", required=True, help="the engine binary")
    parser.add_argument("--tutor", required=True, choices=["white", "black"], help="the side the engine played")
    parser.add_argument("--depth", type=int, default=5, help="the search depth for the engine's own moves")
    parser.add_argument("--voice", default="quiet", choices=["quiet", "every"], help="how much the coach says (B16)")
    parser.add_argument("moves", help="a file with the game's moves in SAN")
    args = parser.parse_args()

    san = Path(args.moves).read_text().split()
    tutor_color = chess.WHITE if args.tutor == "white" else chess.BLACK

    searcher = chess.engine.SimpleEngine.popen_uci([args.uci])
    coach = subprocess.Popen([args.uci, "coach", f"voice={args.voice}"], stdin=subprocess.PIPE,
                             stdout=subprocess.PIPE, text=True, bufsize=1)

    def send(line):
        coach.stdin.write(line + "\n")
        coach.stdin.flush()

    def lines_for(count):
        got = []
        while len(got) < count:
            line = coach.stdout.readline()
            if not line:
                raise SystemExit("the coach ended early")
            line = line.rstrip("\n")
            if line.startswith(("say ", "drop ", "skip ")):
                got.append(line)
            else:
                print("note from the coach:", line, file=sys.stderr)
        return got

    send("takeback 1")
    board = chess.Board()
    openers = collections.defaultdict(collections.Counter)
    per_lane = collections.Counter()
    for ply, text in enumerate(san, start=1):
        move = board.parse_san(text)
        fen = board.fen()
        if board.turn == tutor_color:
            info = searcher.analyse(board, chess.engine.Limit(depth=args.depth))
            score = info["score"].pov(board.turn)
            kind, value = ("mate", score.mate()) if score.is_mate() else ("cp", score.score())
            send(f"tutor {ply} {move.uci()} {info.get('depth', args.depth)} {kind} {value} {fen}")
            got = lines_for(1)
            who = "engine"
        else:
            send(f"student {ply} {move.uci()} {fen}")
            got = lines_for(2)
            who = "student"
        print(f"{ply:>3} {who:<7} {text}")
        for line in sorted(got):
            _, _, lane, said = (line.split(" ", 3) + [""])[:4]
            if line.startswith("say "):
                print(f"      {lane:<4} {said}")
            if line.startswith("say "):
                per_lane[lane] += 1
                openers[lane][" ".join(said.split()[:3])] += 1
        board.push(move)
    send("quit")
    coach.stdin.close()
    coach.wait(timeout=10)
    searcher.quit()

    print()
    for lane in ("game", "you", "me"):
        if per_lane[lane]:
            common = ", ".join(f"{k!r} x{n}" for k, n in openers[lane].most_common(3))
            print(f"{lane}: {per_lane[lane]} lines; most common openings: {common}")


if __name__ == "__main__":
    main()
