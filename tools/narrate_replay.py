#!/usr/bin/env python3
"""Read back what the bot would say in a finished game.

Usage: tools/narrate_replay.py PGN [--game ID] [--bot NAME] [--depth N]

For each position where the named player was to move, this asks the real UCI engine (Narrate on) for its move at a
fixed depth and prints the sentence beside the move that was actually played. The move can differ from the played
one, since the bot searched by clock and this searches by depth; the sentence describes the move printed here.
Needs python-chess, which lichess-bot's environment has:
    .tools/lichess-bot/.venv/bin/python tools/narrate_replay.py "bot/games/Symbia games.pgn" --game d1c4L3tW
"""
import argparse
import subprocess
import sys
from pathlib import Path

import chess
import chess.pgn

ENGINE = Path(__file__).resolve().parent.parent / "target" / "release" / "uci"


def games(path):
    with open(path, encoding="utf-8") as f:
        while (g := chess.pgn.read_game(f)) is not None:
            yield g


def ask(engine, moves, depth):
    """The engine's move and sentence for the position after `moves` (UCI text)."""
    engine.stdin.write(f"position startpos moves {' '.join(moves)}\ngo depth {depth}\n")
    engine.stdin.flush()
    said = ""
    while True:
        line = engine.stdout.readline()
        if not line:
            sys.exit("the engine closed early")
        line = line.strip()
        if line.startswith("info string say: "):
            said = line[len("info string say: "):]
        elif line.startswith("bestmove "):
            return line.split()[1], said


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("pgn")
    ap.add_argument("--game", help="a game id, as in the Site tag")
    ap.add_argument("--bot", default="Symbia", help="the player to speak for")
    ap.add_argument("--depth", type=int, default=5)
    args = ap.parse_args()

    engine = subprocess.Popen([str(ENGINE)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
    engine.stdin.write("setoption name Narrate value true\n")
    found = False
    for game in games(args.pgn):
        site = game.headers.get("Site", "")
        if args.game and not site.endswith(args.game):
            continue
        found = True
        if game.headers.get("White") == args.bot:
            side = chess.WHITE
        elif game.headers.get("Black") == args.bot:
            side = chess.BLACK
        else:
            continue
        print(f"# {site}  {game.headers.get('White')} vs {game.headers.get('Black')}  {game.headers.get('Result')}")
        board = game.board()
        played = []
        for move in game.mainline_moves():
            if board.turn == side:
                best, said = ask(engine, played, args.depth)
                number = f"{board.fullmove_number}{'.' if side == chess.WHITE else '...'}"
                mine = board.san(chess.Move.from_uci(best))
                print(f"{number:<6}played {board.san(move):<7} engine {mine:<7} {said}")
            played.append(move.uci())
            board.push(move)
    engine.stdin.write("quit\n")
    engine.stdin.flush()
    engine.wait()
    if not found:
        sys.exit("no game matched")


if __name__ == "__main__":
    main()
