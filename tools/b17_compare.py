#!/usr/bin/env python3
"""B17 checks that read the line files `tools/quiet_stats.py` writes.

Usage: python3 tools/b17_compare.py NEW.tsv [OLD.tsv]

With OLD (the B16 lines of the same games): whether the quiet Game and You rows are the same row for row (p2), and how
many of the old threat lines are still said (p6). On NEW alone: the standing words said twice in a row and on how many
of her moves the standing is said (p7), and lines longer than 140 characters or without a final period (p9).
"""
import collections
import re
import sys

STAND = re.compile(r"(I am|You are) (slightly better|better|clearly better|winning)|The position is about equal")


def load(path):
    rows = []
    for line in open(path):
        voice, game, ply, lane, kind, text = line.rstrip("\n").split("\t", 5)
        rows.append((voice, int(game), int(ply), lane, kind, text))
    return rows


def main():
    new = load(sys.argv[1])
    quiet = [r for r in new if r[0] == "quiet"]
    if len(sys.argv) > 2:
        old = [r for r in load(sys.argv[2]) if r[0] == "quiet"]
        for lane in ("game", "you"):
            a = sorted(r for r in old if r[3] == lane)
            b = sorted(r for r in quiet if r[3] == lane)
            diff = len(set(a) ^ set(b))
            print(f"p2 {lane}: {len(a)} B16 rows, {len(b)} B17 rows, {diff} differ")
        threats_old = {(r[1], r[2]) for r in old if r[3] == "me" and r[4] == "say" and r[5].startswith("I am attacking your")}
        threats_new = {(r[1], r[2]) for r in quiet if r[3] == "me" and r[4] == "say" and r[5].startswith("I am attacking your")}
        print(f"p6: {len(threats_old)} B16 threat lines, {len(threats_old & threats_new)} still said, "
              f"{len(threats_new - threats_old)} new")
    words = collections.defaultdict(list)
    for r in sorted(quiet, key=lambda r: (r[1], r[2])):
        if r[3] == "me" and r[4] == "say":
            m = STAND.search(r[5])
            if m:
                words[r[1]].append(m.group(0))
    repeats = sum(1 for seq in words.values() for a, b in zip(seq, seq[1:]) if a == b)
    print(f"p7: standing said {sum(len(s) for s in words.values())} times, the same words twice in a row {repeats} times")
    bad = [r for r in quiet if r[4] == "say" and (len(r[5]) > 140 or not r[5].endswith("."))]
    print(f"p9: {len(bad)} quiet lines over 140 characters or without a final period")
    for r in bad[:5]:
        print("  ", r)


if __name__ == "__main__":
    main()
