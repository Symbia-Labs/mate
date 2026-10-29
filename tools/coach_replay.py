#!/usr/bin/env python3
"""Replay the fingerprint's positions through `uci coach` and compare each line with the sentence the engine wrote.

Usage: .tools/lichess-bot/.venv/bin/python tools/coach_replay.py --uci target/release/uci [--reference matches/fp_b11_mac.tsv]

The reference is a fingerprint (tools/fingerprint.py): for every position of the sample games, the engine's move,
depth and score, and the sentence "<about the last move> | <about the engine's move>". For each position this sends the
coach a `student` event (the last move of the game, from the position before it) and a `tutor` event (the engine's
move, depth and score, from the position itself), waits for its lines, and checks:

* a Game or Me line that fits the chat limit must equal the reference sentence;
* a line that would pass the limit must come back refitted: within the limit and not the reference text.

The You lane has no reference sentence. Its lines are checked on their own: opening with "You ", within the limit, and
true of the board (a piece it says it can take stands where it says, a mate in 1 it names exists). With --follow the
events follow the game as it was played instead: odd plies are the student's and even plies the tutor's, so the coach
can use the search it ran while the student "thought" (--pause seconds between the tutor's event and the next
student's). Game and Me lines are then not compared with the reference. --depth and --nodes set the judge's limits.

--out FILE writes a row for every You line (game, ply, class, line). --compare A B reads two such files and says how
often the class agrees; it does not start the coach.

It needs python-chess, which the lichess-bot virtual environment has. The last line printed is a SHA-256 of every
line the coach wrote, in order, so two builds that agree print the same digest.
"""
import argparse
import collections
import hashlib
import re
import statistics
import subprocess
import sys
import time
from pathlib import Path

import chess

ROOT = Path(__file__).resolve().parent.parent
LIMIT = 140
OLD_TAGS = ("Game: ", "You: ", "Me: ")  # B14 removed the tags: no line may start with one.
LOSING = {"Inaccuracy", "Mistake", "Blunder"}
TAKE = re.compile(r"I can take your (\w+) on ([a-h][1-8])\.")
MATE = re.compile(r"You (allowed|missed) mate in (\d+)")


def class_of(text):
    """The class a You line states, from the words it starts with. A line about a mate names no class: "Mate"."""
    for prefix, word in (("You made a blunder", "Blunder"), ("You made a mistake", "Mistake"),
                         ("You made an inaccuracy", "Inaccuracy"), ("You allowed mate", "Mate"),
                         ("You missed mate", "Mate"), ("You played a good move", "Good")):
        if text.startswith(prefix):
            return word
    return "Best"


def has_mate_in_one(board):
    for move in list(board.legal_moves):
        board.push(move)
        mate = board.is_checkmate()
        board.pop()
        if mate:
            return True
    return False


def check_you(text, before_fen, move):
    """Why a You line is wrong about the board, or None. `move` is the student's, played from `before_fen`."""
    if not text.startswith("You ") or len(text) > LIMIT or not text.endswith("."):
        return "does not open with You, or is past the limit, or does not end with a period"
    before = chess.Board(before_fen)
    student = before.turn
    after = before.copy()
    after.push_uci(move)
    for piece, square in TAKE.findall(text):
        found = after.piece_at(chess.parse_square(square))
        if found is None or found.color != student or chess.piece_name(found.piece_type) != piece:
            return f"no {piece} of the student's on {square}"
    for kind, k in MATE.findall(text):
        if k == "1" and not has_mate_in_one(after if kind == "allowed" else before):
            return f"no mate in 1 to be had ({kind})"
    return None


def report_you(yous, out):
    """What the You lines came to: the classes, the waits, the wrong ones, and a digest of the words."""
    classes = collections.Counter(class_of(y["text"]) for y in yous)
    waits = sorted(y["wait"] for y in yous)
    wrong = [y for y in yous if y["why"]]
    print(f"{len(yous)} You lines: " + ", ".join(f"{c} {n}" for c, n in classes.most_common()))
    if waits:
        p95 = waits[min(len(waits) - 1, int(len(waits) * 0.95))]
        print(f"wait from the student's event to the You line: median {statistics.median(waits):.3f} s, "
              f"95th {p95:.3f} s, longest {waits[-1]:.3f} s")
    print(f"{len(wrong)} You lines wrong about the board")
    for y in wrong[:10]:
        print("  wrong:", y["game"], y["ply"], y["text"], "->", y["why"])
    print(hashlib.sha256("\n".join(y["text"] for y in yous).encode()).hexdigest(), "(every You line, in order)")
    if out:
        Path(out).write_text("".join(f"{y['game']}\t{y['ply']}\t{class_of(y['text'])}\t{y['text']}\n" for y in yous))


def compare(a, b):
    """How often two --out files agree on the class of the same move."""
    def rows(path):
        return {tuple(r.split("\t")[:2]): r.split("\t")[2] for r in Path(path).read_text().splitlines()}
    first, second = rows(a), rows(b)
    shared = sorted(set(first) & set(second))
    def agree(x, y):
        # A line about a mate (B14) names no class, so "Mate" agrees with any losing class.
        return x == y or (x == "Mate" and y in LOSING) or (y == "Mate" and x in LOSING)

    same = sum(agree(first[k], second[k]) for k in shared)
    print(f"{len(shared)} moves in both; the class agrees for {same} ({100 * same / max(1, len(shared)):.1f}%)")
    pairs = collections.Counter((first[k], second[k]) for k in shared if not agree(first[k], second[k]))
    for (x, y), n in pairs.most_common():
        print(f"  {x} -> {y}: {n}")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--uci", help="the engine binary; it is started as `uci coach` (not needed with --compare)")
    parser.add_argument("--reference", default=str(ROOT / "matches" / "fp_b11_mac.tsv"))
    parser.add_argument("--moves", default=str(ROOT / "tests" / "fixtures" / "games_sample.moves"))
    parser.add_argument("--depth", type=int, help="the judge's depth (default: the coach's own)")
    parser.add_argument("--nodes", type=int, help="the judge's node limit (default: the coach's own)")
    parser.add_argument("--follow", action="store_true", help="send the events as the game was played")
    parser.add_argument("--pause", type=float, default=0.0, help="with --follow, seconds the student 'thinks'")
    parser.add_argument("--out", help="write a row for every You line to this file")
    parser.add_argument("--compare", nargs=2, metavar=("A", "B"), help="compare two --out files and stop")
    args = parser.parse_args()
    if args.compare:
        return compare(*args.compare)
    if not args.uci:
        parser.error("--uci is required unless --compare is given")

    rows = [line.rstrip("\n").split("\t") for line in Path(args.reference).read_text().splitlines()]
    games = [line.split() for line in Path(args.moves).read_text().splitlines() if line.strip()]
    reference = {(int(r[0]), int(r[1])): r for r in rows}

    limits = [f"{name}={value}" for name, value in (("depth", args.depth), ("nodes", args.nodes)) if value]
    # B16: the coach is quiet unless told otherwise. This script compares every line with the sentence the engine wrote
    # for the move, so it asks for the B15 voice.
    coach = subprocess.Popen([args.uci, "coach", "voice=every", *limits], stdin=subprocess.PIPE,
                             stdout=subprocess.PIPE, text=True, bufsize=1)

    def send(line):
        coach.stdin.write(line + "\n")
        coach.stdin.flush()

    def lines_for(count):
        """The next `count` say or drop lines; notes are kept aside."""
        got = []
        while len(got) < count:
            line = coach.stdout.readline()
            if not line:
                raise SystemExit("the coach ended early")
            line = line.rstrip("\n")
            if line.startswith(("say ", "drop ")):
                got.append((line, time.monotonic()))
            else:
                print("note from the coach:", line, file=sys.stderr)
        return got

    yous = []

    def take_you(g, ply, line, at, sent_at, fen, move):
        """Check a You line against the board it is about, and keep it."""
        kind, _, lane, text = (line.split(" ", 3) + [""])[:4]
        why = check_you(text, fen, move) if kind == "say" else line
        yous.append({"game": g, "ply": ply, "text": text if kind == "say" else line, "wait": at - sent_at, "why": why})

    if args.follow:
        for g, moves in enumerate(games):
            send("takeback 1")
            board = chess.Board()
            for i, move in enumerate(moves, start=1):
                row = reference[(g, i - 1)]
                fen, sent_at = board.fen(), time.monotonic()
                if i % 2 == 1:  # the student's move: a Game line and a You line
                    send(f"student {i} {move} {fen}")
                    for line, at in lines_for(2):
                        if " you " in line[:12]:
                            take_you(g, i, line, at, sent_at, fen, move)
                else:  # the tutor's move: a Me line, and the coach searches for the student while he thinks
                    send(f"tutor {i} {move} {row[3]} {row[4]} {fen}")
                    lines_for(1)
                    time.sleep(args.pause)
                board.push_uci(move)
        send("quit")
        coach.stdin.close()
        coach.wait(timeout=10)
        report_you(yous, args.out)
        return 1 if any(y["why"] for y in yous) else 0

    exact = refit = 0
    wrong, every, shape = [], [], []
    voice = collections.defaultdict(collections.Counter)  # lane -> first word of each line -> how many
    for g, moves in enumerate(games):
        send("takeback 1")  # A new game starts at ply 1 again.
        board, previous = chess.Board(), None
        for ply, move in enumerate(moves):
            row = reference[(g, ply)]
            best, depth, score, sentence = row[2], row[3], row[4], row[7]
            parts = sentence.split(" | ")
            theirs, mine = (parts if len(parts) == 2 else ([None, parts[0]] if ply == 0 else [parts[0], None]))
            expect, sent_at = {}, time.monotonic()
            if previous is not None:
                send(f"student {ply} {moves[ply - 1]} {previous}")
                expect["game"] = theirs
                expect["you"] = True  # judged, not compared with a sentence
            if best != "0000":
                send(f"tutor {ply + 1} {best} {depth} {score} {board.fen()}")
                expect["me"] = mine
            got = lines_for(len(expect))
            # Which lane's line arrives first is timing, not content, so the digest sorts the lines of an exchange.
            every.extend(sorted(line for line, _ in got))
            for line, at in got:
                kind, said_ply, lane, text = (line.split(" ", 3) + [""])[:4]
                if kind == "say":
                    voice[lane][text.split(" ", 1)[0]] += 1
                    if text.startswith(OLD_TAGS) or len(text) > LIMIT or not text.endswith("."):
                        shape.append((g, ply, line))
                if lane == "you":
                    take_you(g, ply, line, at, sent_at, previous, moves[ply - 1])
                    continue
                if kind != "say" or lane not in expect or expect[lane] is None:
                    wrong.append((g, ply, line, "unexpected line"))
                    continue
                whole = expect[lane]
                if len(whole) <= LIMIT:
                    if text == whole:
                        exact += 1
                    else:
                        wrong.append((g, ply, line, "expected " + whole))
                elif len(text) <= LIMIT and text != whole:
                    refit += 1
                    print(f"refit game {g} ply {ply} {lane}: {len(whole)} -> {len(text)} characters")
                else:
                    wrong.append((g, ply, line, "not refitted"))
            previous = board.fen()
            board.push_uci(move)
    send("quit")
    coach.stdin.close()
    coach.wait(timeout=10)

    print(f"{len(rows)} positions: {exact} lines equal the reference, {refit} refitted, {len(wrong)} wrong")
    for item in wrong[:10]:
        print("  wrong:", *item)
    report_you(yous, args.out)
    for lane in ("game", "you", "me"):
        total = sum(voice[lane].values())
        print(f"{lane}: {total} lines; first words: " + ", ".join(f"{w} {n}" for w, n in voice[lane].most_common(6)))
    print(f"{len(shape)} lines with an old tag, past the limit, or not ending with a period")
    for item in shape[:10]:
        print("  shape:", *item)
    print(hashlib.sha256("\n".join(every).encode()).hexdigest(), "(every say and drop line, in order)")
    return 1 if wrong or shape or any(y["why"] for y in yous) else 0


if __name__ == "__main__":
    sys.exit(main())
