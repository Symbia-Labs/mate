#!/usr/bin/env python3
"""Ask the engine for a move in every position of the sample games, and write down everything it answers.

Usage: python3 tools/fingerprint.py --uci target/release/uci [--depth 5] [--out matches/fp.tsv] [--option sharp=0]

For each game in tests/fixtures/games_sample.moves and for each ply, this sends `position startpos moves ...` (the
way lichess-bot does) and `go depth N`, with Narrate on. One row per position: the move, the depth, the score, the
node count, the principal variation and the sentence. Clock fields are left out, because they differ from run to run.
A fixed depth makes the search deterministic, so two builds of the same source must write the same rows.

The last line printed is the SHA-256 of the file. Equal hashes mean equal answers in every position; when they differ,
`diff` the two files to see the first position that does. Needs only the Python standard library.
"""
import argparse
import hashlib
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MOVES = ROOT / "tests" / "fixtures" / "games_sample.moves"


class Engine:
    def __init__(self, path, options):
        self.proc = subprocess.Popen(
            [path], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, bufsize=1
        )
        self.send("uci")
        self.until("uciok")
        for name, value in options:
            self.send(f"setoption name {name} value {value}")
        self.send("isready")
        self.until("readyok")

    def send(self, line):
        self.proc.stdin.write(line + "\n")
        self.proc.stdin.flush()

    def until(self, stop):
        lines = []
        for line in self.proc.stdout:
            line = line.rstrip("\n")
            lines.append(line)
            if line.split(" ", 1)[0] == stop:
                return lines
        raise SystemExit(f"the engine ended before it printed {stop!r}")

    def quit(self):
        self.send("quit")
        self.proc.wait(timeout=10)


def field(tokens, name):
    """The token after `name` in an info line, or an empty string."""
    return tokens[tokens.index(name) + 1] if name in tokens else ""


def row(game, ply, lines):
    info = [l for l in lines if l.startswith("info depth")]
    last = info[-1].split() if info else []
    score = " ".join(last[last.index("score") + 1: last.index("score") + 3]) if "score" in last else ""
    pv = " ".join(last[last.index("pv") + 1:]) if "pv" in last else ""
    say = next((l[len("info string say: "):] for l in lines if l.startswith("info string say: ")), "")
    best = lines[-1].split()[1] if lines and lines[-1].startswith("bestmove") else ""
    return [str(game), str(ply), best, field(last, "depth"), score, field(last, "nodes"), pv, say]


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--uci", required=True, help="the engine binary")
    parser.add_argument("--depth", type=int, default=5)
    parser.add_argument("--out", default=str(ROOT / "matches" / "fingerprint.tsv"))
    parser.add_argument("--moves", default=str(MOVES))
    parser.add_argument("--option", action="append", default=[], metavar="NAME=VALUE", help="a UCI option to set")
    parser.add_argument("--limit", type=int, default=0, help="stop after this many positions (0 = all)")
    args = parser.parse_args()

    options = [("Narrate", "true")] + [tuple(o.split("=", 1)) for o in args.option]
    engine = Engine(args.uci, options)
    rows, nodes, started = [], 0, time.monotonic()
    games = [line.split() for line in Path(args.moves).read_text().splitlines() if line.strip()]
    done = False
    for g, moves in enumerate(games):
        for ply in range(len(moves)):
            played = " ".join(moves[:ply])
            engine.send("position startpos" + (f" moves {played}" if played else ""))
            engine.send(f"go depth {args.depth}")
            r = row(g, ply, engine.until("bestmove"))
            rows.append(r)
            nodes += int(r[5] or 0)
            if args.limit and len(rows) >= args.limit:
                done = True
                break
        if done:
            break
    engine.quit()
    elapsed = time.monotonic() - started

    body = "".join("\t".join(r) + "\n" for r in rows)
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(body)
    digest = hashlib.sha256(body.encode()).hexdigest()
    print(f"{len(rows)} positions at depth {args.depth}, {nodes} nodes, {elapsed:.1f} s, {nodes / elapsed / 1e6:.2f} Mnps")
    print(f"{digest}  {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
