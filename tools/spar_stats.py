#!/usr/bin/env python3
"""Compares ladder runs with sparring on and off: the score, the losses and where the game stood.

Usage: tools/spar_stats.py DIR [DIR...]      (each DIR holds sf<level>.pgn files from tools/ladder.sh or spar_ladder.sh)

For each run and each level it prints the games, wins/draws/losses, the score, the share of games lost, and the
median of the engine's own evaluation from half-move 20 on, in centipawns from its side (mate counts as 3000, and every
value is held to 3000 either way), and the median of the opponent's evaluation of the same positions ("opp view": the
score Stockfish printed for its reply, turned to the engine's side). The own evaluation is what she believed; the
opponent's is the nearest thing to a stronger judge that the games hold. It also lists the games the engine ended by an
illegal move, a crash or a time forfeit, which must be none. Needs only the Python standard library.
"""
import re
import statistics
import sys
from pathlib import Path

TAG = re.compile(r'\[(\w+) "([^"]*)"\]')
COMMENT = re.compile(r"\{([^}]*)\}")
SCORE = re.compile(r"^([+-]?)(?:(\d+(?:\.\d+)?)|M\d+|#\d+)/\d+")
BAD = ("illegal", "disconnect", "stall", "crash", "time forfeit", "loses on time", "timeout", "time loss")
ENGINE = "chess"
FROM_PLY = 20
CLAMP = 3000


def blocks(path):
    text = path.read_text(errors="replace").strip()
    yield from re.split(r"\n\s*\n(?=\[Event )", text)


def centipawns(comment):
    """The engine's score in a move comment such as '+0.35/12 0.5s', in centipawns, or None."""
    m = SCORE.match(comment.strip())
    if not m:
        return None
    if m.group(2) is None:  # a mate
        return CLAMP if m.group(1) != "-" else -CLAMP
    value = round(float(m.group(2)) * 100)
    return max(-CLAMP, min(CLAMP, -value if m.group(1) == "-" else value))


def scores(tags, block):
    """{ply: centipawns} for every move that has a score in its comment, from the side that made the move."""
    fen = tags.get("FEN", "").split()
    side = 0 if not fen or fen[1] == "w" else 1  # 0 White to move, 1 Black
    number = int(fen[5]) if len(fen) >= 6 else 1
    ply = (number - 1) * 2 + side
    body = block[block.rfind("]") + 1 :]
    out = {}
    for token in re.finditer(r"\{[^}]*\}|[^\s{]+", body):
        text = token.group(0)
        if text.startswith("{"):
            continue
        if re.fullmatch(r"\d+\.+|1-0|0-1|1/2-1/2|\*", text):
            continue
        # A move; its comment follows it.
        after = body[token.end() :].lstrip()
        comment = COMMENT.match(after)
        value = centipawns(comment.group(1)) if comment else None
        if value is not None:
            out[ply] = value
        ply += 1
    return out


def evals(tags, block, engine_color):
    """(ply, own, view) for each of the engine's moves that has a score, ply counted from the starting position.

    `own` is the engine's score after its move. `view` is the opponent's score after the reply, turned to the engine's
    side (None when the game ended or the reply has no score): Stockfish's opinion of the same position.
    """
    seen = scores(tags, block)
    return [
        (ply, value, -seen[ply + 1] if ply + 1 in seen else None)
        for ply, value in sorted(seen.items())
        if ply % 2 == engine_color
    ]


def summarize(directory):
    rows = []
    for path in sorted(Path(directory).glob("sf*.pgn"), key=lambda p: int(re.sub(r"\D", "", p.stem))):
        level = int(re.sub(r"\D", "", path.stem))
        wins = draws = losses = 0
        late = []  # the engine's own score after each of its moves from half-move 20
        seen = []  # the opponent's score for the engine after its reply, the same moves
        incidents = []
        for block in blocks(path):
            tags = dict(TAG.findall(block))
            result = tags.get("Result")
            if result not in ("1-0", "0-1", "1/2-1/2"):
                continue
            white = tags.get("White") == ENGINE
            lost = False
            if result == "1/2-1/2":
                draws += 1
            elif (result == "1-0") == white:
                wins += 1
            else:
                losses += 1
                lost = True
            reason = (tags.get("Termination", "") + " " + block).lower()
            if lost and any(word in reason for word in BAD):  # Only a game the engine lost can be its own fault.
                incidents.append((tags.get("Round", "?"), tags.get("Termination", "")))
            for ply, own, view in evals(tags, block, 0 if white else 1):
                if ply >= FROM_PLY:
                    late.append(own)
                    if view is not None:
                        seen.append(view)
        games = wins + draws + losses
        if games:
            rows.append((level, games, wins, draws, losses, (wins + draws / 2) / games,
                         statistics.median(late) if late else None, statistics.median(seen) if seen else None, incidents))
    return rows


def main(directories):
    for directory in directories:
        print(f"{directory}")
        print(f"  {'level':>5} {'games':>5} {'W':>3} {'D':>3} {'L':>3} {'score':>6} {'lost':>6} {'own eval':>9} {'opp view':>9}   (medians from ply 20)")
        total = [0, 0, 0, 0]
        bad = []
        for level, games, w, d, l, score, median, view, incidents in summarize(directory):
            shown = "-" if median is None else f"{median:+.0f}"
            opinion = "-" if view is None else f"{view:+.0f}"
            print(f"  {level:>5} {games:>5} {w:>3} {d:>3} {l:>3} {score:6.1%} {l / games:6.1%} {shown:>9} {opinion:>9}")
            total = [total[0] + games, total[1] + w, total[2] + d, total[3] + l]
            bad += [(level, *i) for i in incidents]
        if total[0]:
            print(f"  {'all':>5} {total[0]:>5} {total[1]:>3} {total[2]:>3} {total[3]:>3} {(total[1] + total[2] / 2) / total[0]:6.1%} {total[3] / total[0]:6.1%}")
        print(f"  illegal moves, crashes or time forfeits: {bad if bad else 'none'}")


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    main(sys.argv[1:])
