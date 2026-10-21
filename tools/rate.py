#!/usr/bin/env python3
"""Fits one rating for the engine from ladder PGNs written by tools/ladder.sh.

Usage: tools/rate.py FILE_OR_DIR...

The opponent's level is read from its name (sf1500 is Stockfish at UCI_Elo 1500). The fitted rating R makes the
engine's scores sum to the sum of 1 / (1 + 10^((level - R) / 400)) over all games. The 95% interval treats the
two games played from one opening (same level and round) as one observation, because their results are not
independent. Illegal moves, crashes and time losses by the engine are counted and listed.
"""
import math
import re
import sys
from pathlib import Path

TAG = re.compile(r'\[(\w+) "([^"]*)"\]')
ENGINE = "chess"
BAD = ("illegal", "disconnect", "stall", "crash", "time forfeit", "loses on time", "timeout", "time loss")


def games(paths):
    files = []
    for p in map(Path, paths):
        files += sorted(p.glob("*.pgn")) if p.is_dir() else [p]
    for f in files:
        text = f.read_text(errors="replace")
        for block in re.split(r"\n\s*\n(?=\[Event )", text.strip()):
            tags = dict(TAG.findall(block))
            if "Result" in tags:
                yield f"{f.parent.name}/{f.name}", tags, block


def expected(level, r):
    return 1.0 / (1.0 + 10.0 ** ((level - r) / 400.0))


def main(paths):
    rows = []  # (level, score, cluster)
    per = {}
    incidents = []
    for name, tags, block in games(paths):
        white, black, result = tags.get("White"), tags.get("Black"), tags["Result"]
        if ENGINE not in (white, black) or result not in ("1-0", "0-1", "1/2-1/2"):
            continue
        opp = black if white == ENGINE else white
        m = re.fullmatch(r"sf(\d+)", opp or "")
        if not m:
            continue
        level = int(m.group(1))
        s_white = {"1-0": 1.0, "0-1": 0.0, "1/2-1/2": 0.5}[result]
        s = s_white if white == ENGINE else 1.0 - s_white
        rows.append((level, s, (name, tags.get("Round", ""))))
        per.setdefault(level, [0, 0, 0])[0 if s == 1.0 else 2 if s == 0.0 else 1] += 1
        text = (tags.get("Termination", "") + " " + " ".join(re.findall(r"\{([^}]*)\}", block))).lower()
        if s == 0.0 and any(b in text for b in BAD):
            incidents.append((name, tags.get("Round", ""), text.strip()[:80]))
    if not rows:
        sys.exit("no games found")
    print("level   games   W-D-L        score")
    for level in sorted(per):
        w, d, l = per[level]
        n = w + d + l
        print(f"{level:<7} {n:<7} {w}-{d}-{l:<9}{(w + d / 2) / n:6.1%}")
    total = sum(sum(v) for v in per.values())
    lo, hi = 0.0, 4000.0
    for _ in range(80):
        mid = (lo + hi) / 2
        if sum(s - expected(L, mid) for L, s, _ in rows) > 0:
            lo = mid
        else:
            hi = mid
    r = (lo + hi) / 2
    clusters = {}
    for L, s, c in rows:
        clusters[c] = clusters.get(c, 0.0) + (s - expected(L, r))
    slope = sum(expected(L, r) * (1 - expected(L, r)) for L, _, _ in rows) * math.log(10) / 400.0
    half = 1.96 * math.sqrt(sum(v * v for v in clusters.values())) / slope
    edge = " (at the edge of the ladder: scores are all wins or all losses)" if r < 1 or r > 3999 else ""
    print(f"\nrating {r:.0f} +/- {half:.0f} (95%) over {total} games{edge}")
    print(f"engine losses by illegal move, crash or time: {len(incidents)}")
    for item in incidents[:10]:
        print("  ", *item)


if __name__ == "__main__":
    main(sys.argv[1:])
