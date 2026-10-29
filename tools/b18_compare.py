#!/usr/bin/env python3
"""Compare two fingerprints (tools/fingerprint.py) and two You-line files (tools/coach_replay.py --out), B18.

Usage: python3 tools/b18_compare.py OLD_FP NEW_FP [OLD_YOU NEW_YOU]

Fingerprint rows are game, ply, move, depth, score, nodes, principal variation, sentence. Printed: how many of the rows
differ in each column, and the total node count of each file. You rows are game, ply, class, line: how many lines
differ, and how many changed class, by pair of classes.
"""
import collections
import sys


def rows(path, width):
    out = {}
    for line in open(path):
        parts = line.rstrip("\n").split("\t")
        out[(parts[0], parts[1])] = parts[2:width]
    return out


def main():
    old, new = rows(sys.argv[1], 8), rows(sys.argv[2], 8)
    names = ["move", "depth", "score", "nodes", "pv", "sentence"]
    keys = sorted(old)
    assert sorted(new) == keys, "the two fingerprints cover different positions"
    print(f"{len(keys)} positions")
    for i, name in enumerate(names):
        n = sum(1 for k in keys if old[k][i] != new[k][i])
        print(f"  {name:>8} differs in {n} ({100 * n / len(keys):.1f}%)")
    total_old = sum(int(old[k][3]) for k in keys)
    total_new = sum(int(new[k][3]) for k in keys)
    print(f"  nodes: {total_old:,} -> {total_new:,} ({100 * (total_new - total_old) / total_old:+.1f}%)")
    mates_old = sum(1 for k in keys if old[k][2].startswith("mate"))
    mates_new = sum(1 for k in keys if new[k][2].startswith("mate"))
    print(f"  mate scores: {mates_old} -> {mates_new}")
    if len(sys.argv) > 4:
        yo, yn = rows(sys.argv[3], 4), rows(sys.argv[4], 4)
        ks = sorted(yo)
        assert sorted(yn) == ks, "the two You files cover different moves"
        text = sum(1 for k in ks if yo[k][1] != yn[k][1])
        moved = collections.Counter((yo[k][0], yn[k][0]) for k in ks if yo[k][0] != yn[k][0])
        print(f"{len(ks)} You lines: {text} differ ({100 * text / len(ks):.1f}%), {sum(moved.values())} changed class")
        for (a, b), n in moved.most_common():
            print(f"  {a} -> {b}: {n}")


if __name__ == "__main__":
    main()
