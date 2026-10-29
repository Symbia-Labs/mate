#!/usr/bin/env python3
"""Run the coach over a fixed list of events and hash every line it writes, so two builds can be compared (B16).

Usage:
  .tools/lichess-bot/.venv/bin/python tools/quiet_events.py gen [--reference matches/fp_b15_mac.tsv] [--out matches/b16_events.txt]
  python3 tools/quiet_events.py run --uci target/release/uci [--voice quiet|every] [--events matches/b16_events.txt] [--out FILE]

`gen` writes the events of the 602 sample positions the way coach_replay.py sends them: a `student` event for the last
move of the game so far and a `tutor` event for the engine's move, with the engine's own depth and score from the
reference fingerprint. `run` needs only the standard library: it starts `uci coach voice=...`, sends one event at a
time, waits for the lines that event brings (two for a student event, one for a tutor event; a `skip` counts), sorts the
lines of an exchange, and prints how many it saw by kind and lane and the SHA-256 of all of them in order. Two builds
that agree print the same digest, which is how the Mac and the Linux build are compared.
"""
import argparse
import collections
import hashlib
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def gen(args):
    import chess

    rows = [line.rstrip("\n").split("\t") for line in Path(args.reference).read_text().splitlines()]
    reference = {(int(r[0]), int(r[1])): r for r in rows}
    games = [line.split() for line in (ROOT / "tests" / "fixtures" / "games_sample.moves").read_text().splitlines()
             if line.strip()]
    out = []
    for g, moves in enumerate(games):
        out.append("G")
        board, previous = chess.Board(), None
        for ply, move in enumerate(moves):
            row = reference[(g, ply)]
            best, depth, score = row[2], row[3], row[4]
            if previous is not None:
                out.append(f"2 student {ply} {moves[ply - 1]} {previous}")
            if best != "0000":
                out.append(f"1 tutor {ply + 1} {best} {depth} {score} {board.fen()}")
            previous = board.fen()
            board.push_uci(move)
    Path(args.out).write_text("\n".join(out) + "\n")
    print(f"{len(out)} lines -> {args.out}")


def run(args):
    coach = subprocess.Popen([args.uci, "coach", f"voice={args.voice}"], stdin=subprocess.PIPE,
                             stdout=subprocess.PIPE, text=True, bufsize=1)
    digest = hashlib.sha256()
    kinds = collections.Counter()
    kept = []
    for raw in Path(args.events).read_text().splitlines():
        if raw == "G":
            coach.stdin.write("takeback 1\n")
            coach.stdin.flush()
            continue
        expect, event = raw.split(" ", 1)
        coach.stdin.write(event + "\n")
        coach.stdin.flush()
        got = []
        while len(got) < int(expect):
            line = coach.stdout.readline()
            if not line:
                raise SystemExit("the coach ended early")
            line = line.rstrip("\n")
            if line.startswith(("say ", "drop ", "skip ")):
                got.append(line)
            else:
                print("note from the coach:", line, file=sys.stderr)
        for line in sorted(got):
            digest.update((line + "\n").encode())
            kept.append(line)
            kinds[tuple(line.split(" ", 3)[0:3:2])] += 1
    coach.stdin.write("quit\n")
    coach.stdin.close()
    coach.wait(timeout=30)
    if args.out:
        Path(args.out).write_text("\n".join(kept) + "\n")
    print(", ".join(f"{k[0]} {k[1]} {n}" for k, n in sorted(kinds.items())))
    print(digest.hexdigest(), f"(every {args.voice} line, in order, {len(kept)} lines)")


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="cmd", required=True)
    g = sub.add_parser("gen")
    g.add_argument("--reference", default=str(ROOT / "matches" / "fp_b15_mac.tsv"))
    g.add_argument("--out", default=str(ROOT / "matches" / "b16_events.txt"))
    r = sub.add_parser("run")
    r.add_argument("--uci", required=True)
    r.add_argument("--voice", default="quiet", choices=["quiet", "every"])
    r.add_argument("--events", default=str(ROOT / "matches" / "b16_events.txt"))
    r.add_argument("--out")
    args = parser.parse_args()
    gen(args) if args.cmd == "gen" else run(args)


if __name__ == "__main__":
    main()
