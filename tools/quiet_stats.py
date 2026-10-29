#!/usr/bin/env python3
"""Replay Symbia's played games through `uci coach` in both voices and measure the quiet one (B16).

Usage: .tools/lichess-bot/.venv/bin/python tools/quiet_stats.py --uci target/release/uci
           [--games matches/symbia_lichess_games.ndjson] [--depth 5] [--limit N] [--out matches/b16_lines.tsv]

For each game (standard, from the start) the side named Symbia is the tutor and the other side the student. The tutor's
depth and score come from the engine's own search of each position (cached in matches/b16_tutor_cache.json), and the
move sent is the one that was played. Each game gets its own coach process, as in the bot, and each voice is run
through the same events.

Reported: lines per move pair in each voice; what the quiet voice dropped that the every voice said and matters
(mistakes, blunders, mates, loose captures and threats); and every reason the quiet voice gave, checked against the
board by python-chess with its own copy of the mobility, king and pawn blocks (it does not call the engine's blocks).
"""
import argparse
import collections
import json
import re
import statistics
import subprocess
import sys
import threading
import time
from pathlib import Path

import chess
import chess.engine

ROOT = Path(__file__).resolve().parent.parent
LIMIT = 140
VALUES = {chess.PAWN: 100, chess.KNIGHT: 320, chess.BISHOP: 330, chess.ROOK: 500, chess.QUEEN: 900, chess.KING: 10000}
NAMES = {"pawn": chess.PAWN, "knight": chess.KNIGHT, "bishop": chess.BISHOP, "rook": chess.ROOK, "queen": chess.QUEEN}
THRESHOLD = {"mobility": 20, "king": 25, "pawns": 20}
LEVEL_FROM = [0, 30, 80, 180, 350]
WORDS = {"slightly better": 1, "better": 2, "clearly better": 3, "winning": 4}
HOLD = 10
NUMBER = {"a": 1, "two": 2, "three": 3}


# ---- the blocks, written again from their descriptions -------------------------------------------------------------

def mobility(board, color):
    own = board.occupied_co[color]
    weights = {chess.KNIGHT: 4, chess.BISHOP: 5, chess.ROOK: 2, chess.QUEEN: 1}
    total = 0
    for piece, weight in weights.items():
        for sq in board.pieces(piece, color):
            total += weight * chess.popcount(board.attacks_mask(sq) & ~own)
    return total


def king(board, color):
    """Minus the exposure: the king block's number for `color`, higher is safer."""
    them = not color
    ksq = board.king(color)
    file, rank = chess.square_file(ksq), chess.square_rank(ksq)
    pawns = board.pieces(chess.PAWN, color)
    forward = 1 if color == chess.WHITE else -1
    home_side = rank <= 1 if color == chess.WHITE else rank >= 6
    cp = 0
    if home_side and 0 <= rank + forward < 8:
        for f in range(max(file - 1, 0), min(file + 1, 7) + 1):
            if chess.square(f, rank + forward) not in pawns:
                cp += 15
    for f in range(max(file - 1, 0), min(file + 1, 7) + 1):
        if not any(chess.square_file(p) == f for p in pawns):
            cp += 10
    zone = chess.BB_KING_ATTACKS[ksq] | chess.BB_SQUARES[ksq]
    for piece in (chess.KNIGHT, chess.BISHOP, chess.ROOK, chess.QUEEN):
        for sq in board.pieces(piece, them):
            if board.attacks_mask(sq) & zone:
                cp += 10
    home = 0 if color == chess.WHITE else 7
    if rank == home:
        r = rank + forward
        shut = all(board.occupied_co[color] & chess.BB_SQUARES[chess.square(f, r)]
                   for f in range(max(file - 1, 0), min(file + 1, 7) + 1))
        heavy = board.pieces(chess.ROOK, them) | board.pieces(chess.QUEEN, them)
        if shut and heavy:
            cp += 30
    return -cp


def pawns(board, color):
    own = board.pieces(chess.PAWN, color)
    enemy = board.pieces(chess.PAWN, not color)
    cp = 0
    for sq in own:
        f, rank = chess.square_file(sq), chess.square_rank(sq)

        def ahead(s):
            r = chess.square_rank(s)
            return r > rank if color == chess.WHITE else r < rank

        if any(chess.square_file(o) == f and ahead(o) for o in own):
            cp -= 10
        if not any(abs(chess.square_file(o) - f) == 1 for o in own):
            cp -= 15
        if not any(abs(chess.square_file(e) - f) <= 1 and ahead(e) for e in enemy):
            advanced = rank - 1 if color == chess.WHITE else 6 - rank
            cp += 20 * max(advanced, 0)
    return cp


BLOCKS = {"mobility": mobility, "king": king, "pawns": pawns}


def loose(board, color):
    out = set()
    for piece in (chess.KNIGHT, chess.BISHOP, chess.ROOK, chess.QUEEN):
        for sq in board.pieces(piece, color):
            attackers = board.attackers(not color, sq)
            if not attackers:
                continue
            if not board.attackers(color, sq):
                out.add(sq)
                continue
            if min(VALUES[board.piece_type_at(a)] for a in attackers) < VALUES[piece]:
                out.add(sq)
    return out


def material_diff(board, color):
    return {name: len(board.pieces(piece, color)) - len(board.pieces(piece, not color)) for name, piece in NAMES.items()}


def material_cp(diff):
    return sum(n * VALUES[NAMES[name]] for name, n in diff.items())


# ---- the claims ----------------------------------------------------------------------------------------------------

DEV = re.compile(r"I developed my (knight|bishop) to ([a-h][1-8])\.")
CENTER = re.compile(r"I played pawn to ([a-h][1-8]) to take space in the center\.")
OPEN = re.compile(r"I played rook to ([a-h][1-8]) to use the open ([a-h])-file\.")
GAIN = re.compile(r"I played (\w+) to ([a-h][1-8]) to (give my pieces more room|make my king safer|"
                  r"improve my pawn structure|cramp your pieces|expose your king|weaken your pawns)\.")
CASTLE = re.compile(r"I castled (kingside|queenside)( to make my king safer)?\.")
STAND = re.compile(r"(I am|You are) (slightly better|better|clearly better|winning)(?: because ([^.]+))?\.")
EQUAL = re.compile(r"The position is about equal\.")
TOOK_LOOSE = re.compile(r"I took your loose (\w+) on ([a-h][1-8])")
ATTACKING = re.compile(r"I am attacking your ([^.]+?), which (?:is|are) loose\.")
MISSED = re.compile(r"You could have taken my loose (\w+) on ([a-h][1-8])\.")
ANSWER = re.compile(r"I can answer with (castling (?:kingside|queenside)|\w+ to [a-h][1-8]), which "
                    r"(gives check|attacks your (\w+) on ([a-h][1-8]))\.")
# B17: the Me lines that never wait for the cooldown. Threats ("I am attacking your") may wait.
ALWAYS = ("I see a forced mate", "I may be getting mated", "I took your loose", "I promoted")
COOLDOWN = 3
YOU_REASON = {"It cramps your pieces.": "mobility", "It leaves your king more exposed.": "king",
              "It weakens your pawn structure.": "pawns"}
GAINS = {"give my pieces more room": ("mobility", True), "make my king safer": ("king", True),
         "improve my pawn structure": ("pawns", True), "cramp your pieces": ("mobility", False),
         "expose your king": ("king", False), "weaken your pawns": ("pawns", False)}
HOME = {chess.WHITE: {chess.KNIGHT: (chess.B1, chess.G1), chess.BISHOP: (chess.C1, chess.F1)},
        chess.BLACK: {chess.KNIGHT: (chess.B8, chess.G8), chess.BISHOP: (chess.C8, chess.F8)}}


def level_of(score):
    a = abs(score)
    level = max(i for i in range(5) if a >= LEVEL_FROM[i])
    return level if score >= 0 else -level


def check_me(text, before, move, score):
    """Wrong claims in a quiet Me line. `before` is the board before the tutor's move."""
    wrong = []
    me = before.turn
    after = before.copy()
    after.push(move)
    piece = before.piece_type_at(move.from_square)
    to = chess.square_name(move.to_square)
    for kind, to_sq in [(m.group(1), m.group(2)) for m in DEV.finditer(text)]:
        if not (NAMES[kind] == piece and move.from_square in HOME[me].get(piece, ()) and to_sq == to
                and before.fullmove_number <= 20 and not before.is_capture(move)):
            wrong.append(f"developed {kind} to {to_sq}")
    for m in CENTER.finditer(text):
        if not (piece == chess.PAWN and m.group(1) == to and to in ("d4", "e4", "d5", "e5")
                and not before.is_capture(move) and before.fullmove_number <= 20):
            wrong.append("center " + m.group(1))
    for m in OPEN.finditer(text):
        file = "abcdefgh".index(m.group(2))
        pawns_on_file = [s for s in after.pieces(chess.PAWN, chess.WHITE) | after.pieces(chess.PAWN, chess.BLACK)
                         if chess.square_file(s) == file]
        if not (piece == chess.ROOK and m.group(1) == to and chess.square_file(move.to_square) == file
                and chess.square_file(move.from_square) != file and not pawns_on_file):
            wrong.append("open file " + m.group(2))
    for m in GAIN.finditer(text):
        name, to_sq, purpose = m.groups()
        term, mine = GAINS[purpose]
        fn = BLOCKS[term]
        if mine:
            change = fn(after, me) - fn(before, me)
        else:
            change = fn(before, not me) - fn(after, not me)
        if not (dict(NAMES, king=chess.KING).get(name) == piece and to_sq == to and change >= THRESHOLD[term]):
            wrong.append(f"{purpose} ({change})")
    for m in CASTLE.finditer(text):
        if not before.is_castling(move) or (m.group(2) and king(after, me) - king(before, me) < THRESHOLD["king"]):
            wrong.append("castle " + m.group(0))
    for m in TOOK_LOOSE.finditer(text):
        taken = before.piece_type_at(move.to_square)
        if not (before.is_capture(move) and taken is not None and chess.piece_name(taken) == m.group(1)
                and m.group(2) == to and move.to_square in loose(before, not me)):
            wrong.append("took loose " + m.group(0))
    for m in ATTACKING.finditer(text):
        new = loose(after, not me) - loose(before, not me)
        squares = re.findall(r"on ([a-h][1-8])", m.group(1))
        if not squares or any(chess.parse_square(s) not in new for s in squares if "more" not in m.group(1)):
            wrong.append("attacking " + m.group(0))
    equal = EQUAL.search(text)
    if equal and abs(score) >= LEVEL_FROM[1] + HOLD:
        wrong.append(f"equal at {score}")
    for m in STAND.finditer(text):
        who, word, reason = m.groups()
        level = WORDS[word]
        a = abs(score)
        high = LEVEL_FROM[level + 1] if level < 4 else 10 ** 9
        if not (LEVEL_FROM[level] - HOLD <= a < high + HOLD) or (who == "I am") != (score > 0):
            wrong.append(f"{who} {word} at {score}")
        if reason:
            wrong.extend(check_lead(reason, after, me, who == "I am", score))
    return wrong


def check_lead(reason, after, me, mine, score):
    """Wrong claims in the reason for a lead."""
    leader = me if mine else not me
    other = not leader
    wrong = []
    m = re.fullmatch(r"(?:I am|you are) up (.+)", reason)
    if m:
        diff = material_diff(after, leader)
        counts = {}
        for part in m.group(1).split(" and "):
            n, name = part.split(" ", 1)
            counts[name.rstrip("s")] = NUMBER[n] if n in NUMBER else int(n)
        if any(diff[name] != n for name, n in counts.items()) or any(d < 0 for d in diff.values()) \
                or any(d > 0 and name not in counts for name, d in diff.items()):
            wrong.append(f"material '{reason}' vs {diff}")
        return wrong
    if re.fullmatch(r"(?:I am|you are) ahead in material", reason):
        if material_cp(material_diff(after, leader)) < 90:
            wrong.append(f"material '{reason}'")
        return wrong
    table = {"my pieces have more room": ("mobility", True), "your pieces have more room": ("mobility", False),
             "your king is more exposed": ("king", True), "my king is more exposed": ("king", False),
             "my pawn structure is better": ("pawns", True), "your pawn structure is better": ("pawns", False)}
    if reason not in table:
        return [f"unknown reason '{reason}'"]
    term, said_mine = table[reason]
    if said_mine != mine:
        wrong.append(f"'{reason}' with the lead on the other side")
    lead = BLOCKS[term](after, leader) - BLOCKS[term](after, other)
    if lead < THRESHOLD[term]:
        wrong.append(f"'{reason}' lead {lead}")
    return wrong


def check_you(text, before, move):
    """Wrong claims in a quiet You line (the take and mate claims are checked by coach_replay.py)."""
    wrong = []
    you = before.turn
    after = before.copy()
    after.push(move)
    for m in MISSED.finditer(text):
        sq = chess.parse_square(m.group(2))
        found = before.piece_at(sq)
        if not (found is not None and found.color != you and chess.piece_name(found.piece_type) == m.group(1)
                and sq in loose(before, not you) and before.is_attacked_by(you, sq) and move.to_square != sq):
            wrong.append("missed capture " + m.group(0))
    for sentence, term in YOU_REASON.items():
        if sentence in text:
            loss = BLOCKS[term](before, you) - BLOCKS[term](after, you)
            if loss < THRESHOLD[term]:
                wrong.append(f"{sentence} (loss {loss})")
    for m in ANSWER.finditer(text):
        wrong.extend(check_answer(m, after, you))
    return wrong


def check_answer(m, after, you):
    """The reply clause against the board: the named move is legal for her after his move, is not a capture, and does
    what the clause says (gives check, or attacks a piece of his that is loose now and was not before)."""
    words = m.group(1)
    if words.startswith("castling"):
        kingside = words.endswith("kingside")
        candidates = [mv for mv in after.legal_moves
                      if after.is_castling(mv) and after.is_kingside_castling(mv) == kingside]
    else:
        name, _, square = words.split(" ")
        candidates = [mv for mv in after.legal_moves
                      if after.piece_type_at(mv.from_square) == dict(NAMES, king=chess.KING)[name]
                      and mv.to_square == chess.parse_square(square) and not after.is_capture(mv)]
    if not candidates:
        return [f"answer {m.group(0)}: no such legal move"]
    problems = []
    for mv in candidates:
        answered = after.copy()
        answered.push(mv)
        if m.group(2) == "gives check":
            if not answered.is_check():
                problems.append(f"answer {m.group(0)}: {mv.uci()} gives no check")
        else:
            sq = chess.parse_square(m.group(4))
            found = answered.piece_at(sq)
            ok = (found is not None and found.color == you and chess.piece_name(found.piece_type) == m.group(3)
                  and sq in loose(answered, you) and sq not in loose(after, you)
                  and mv.to_square in answered.attackers(not you, sq))
            if not ok:
                problems.append(f"answer {m.group(0)}: {mv.uci()} does not do that")
    return problems


def class_of(text):
    for prefix, word in (("You made a blunder", "Blunder"), ("You made a mistake", "Mistake"),
                         ("You made an inaccuracy", "Inaccuracy"), ("You allowed mate", "Mate"),
                         ("You missed mate", "Mate"), ("You played a good move", "Good")):
        if text.startswith(prefix):
            return word
    return "Best"


# ---- the replay ----------------------------------------------------------------------------------------------------

def load_games(path, limit):
    games = []
    for line in Path(path).read_text().splitlines():
        g = json.loads(line)
        if g.get("variant", "standard") != "standard" or not g.get("moves"):
            continue
        white = g["players"]["white"].get("user", {}).get("id", "")
        black = g["players"]["black"].get("user", {}).get("id", "")
        if "symbia" not in (white, black):
            continue
        games.append({"id": g["id"], "san": g["moves"].split(), "tutor": chess.WHITE if white == "symbia" else chess.BLACK})
    return games[:limit] if limit else games


def analyse(uci, games, depth, cache_path):
    cache = json.loads(cache_path.read_text()) if cache_path.exists() else {}
    engine = chess.engine.SimpleEngine.popen_uci([uci])
    for g in games:
        board = chess.Board()
        for san in g["san"]:
            move = board.parse_san(san)
            if board.turn == g["tutor"]:
                key = f"{depth} {board.fen()}"
                if key not in cache:
                    info = engine.analyse(board, chess.engine.Limit(depth=depth))
                    score = info["score"].pov(board.turn)
                    kind, value = ("mate", score.mate()) if score.is_mate() else ("cp", score.score())
                    cache[key] = [info.get("depth", depth), kind, value]
            board.push(move)
    engine.quit()
    cache_path.write_text(json.dumps(cache))
    return cache


def run_voice(uci, voice, games, depth, cache, rows):
    for gi, g in enumerate(games):
        coach = subprocess.Popen([uci, "coach", f"voice={voice}"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                 text=True, bufsize=1)
        board = chess.Board()
        for ply, san in enumerate(g["san"], start=1):
            move = board.parse_san(san)
            fen = board.fen()
            if board.turn == g["tutor"]:
                d, kind, value = cache[f"{depth} {fen}"]
                event, expect = f"tutor {ply} {move.uci()} {d} {kind} {value} {fen}", 1
                score = value if kind == "cp" else (99999 if value > 0 else -99999)
            else:
                event, expect, score = f"student {ply} {move.uci()} {fen}", 2, None
            sent = time.monotonic()
            coach.stdin.write(event + "\n")
            coach.stdin.flush()
            got = 0
            while got < expect:
                line = coach.stdout.readline()
                if not line:
                    raise SystemExit("the coach ended early")
                line = line.rstrip("\n")
                if line.startswith(("say ", "drop ", "skip ")):
                    parts = line.split(" ", 3)
                    rows.append({"voice": voice, "game": gi, "ply": ply, "kind": parts[0], "lane": parts[2],
                                 "text": parts[3] if len(parts) > 3 else "", "wait": time.monotonic() - sent,
                                 "fen": fen, "move": move.uci(), "score": score, "tutor": g["tutor"]})
                    got += 1
            board.push(move)
        coach.stdin.write("quit\n")
        coach.stdin.close()
        coach.wait(timeout=30)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--uci", required=True)
    parser.add_argument("--games", default=str(ROOT / "matches" / "symbia_lichess_games.ndjson"))
    parser.add_argument("--depth", type=int, default=5)
    parser.add_argument("--limit", type=int, default=0)
    parser.add_argument("--out", default=str(ROOT / "matches" / "b16_lines.tsv"))
    parser.add_argument("--cache", default=str(ROOT / "matches" / "b16_tutor_cache.json"))
    args = parser.parse_args()

    games = load_games(args.games, args.limit)
    print(f"{len(games)} games; analysing the tutor's positions", flush=True)
    cache = analyse(args.uci, games, args.depth, Path(args.cache))
    rows = {"every": [], "quiet": []}
    threads = [threading.Thread(target=run_voice, args=(args.uci, v, games, args.depth, cache, rows[v]))
               for v in ("every", "quiet")]
    for t in threads:
        t.start()
    for t in threads:
        t.join()

    student_moves = sum(1 for r in rows["every"] if r["lane"] == "game")
    tutor_moves = sum(1 for r in rows["every"] if r["lane"] == "me")
    pairs = (student_moves + tutor_moves) / 2
    print(f"{student_moves} student moves, {tutor_moves} tutor moves, {pairs:.0f} move pairs")
    said = {v: [r for r in rows[v] if r["kind"] == "say"] for v in rows}
    lanes = {v: collections.Counter(r["lane"] for r in said[v]) for v in rows}
    for v in ("every", "quiet"):
        total = len(said[v])
        print(f"{v:>5}: {total} lines ({total / pairs:.2f} per pair); "
              + ", ".join(f"{lane} {lanes[v][lane]} ({lanes[v][lane] / pairs:.2f})" for lane in ("game", "you", "me"))
              + f"; {sum(1 for r in rows[v] if r['kind'] == 'drop')} dropped, "
              + f"{sum(1 for r in rows[v] if r['kind'] == 'skip')} skipped")
    cut = 100 * (1 - len(said["quiet"]) / len(said["every"]))
    print(f"quiet says {cut:.0f}% fewer lines than every")

    # What matters must still be said.
    every_by = {(r["game"], r["ply"], r["lane"]): r for r in said["every"]}
    quiet_by = {(r["game"], r["ply"], r["lane"]): r for r in said["quiet"]}
    must = []
    for key, r in every_by.items():
        t = r["text"]
        if r["lane"] == "you" and class_of(t) in ("Mistake", "Blunder", "Mate"):
            must.append(key)
        if r["lane"] == "me" and (t.startswith(ALWAYS) or "promoted to a" in t):
            must.append(key)
    missing = [k for k in must if k not in quiet_by]
    print(f"{len(must)} lines that matter in every; {len(missing)} missing in quiet")
    for k in missing[:10]:
        print("  missing:", k, every_by[k]["text"])

    # Every reason the quiet voice gave, against the board.
    wrong, checked = [], collections.Counter()
    for r in said["quiet"]:
        board = chess.Board(r["fen"])
        move = chess.Move.from_uci(r["move"])
        if r["lane"] == "me":
            problems = check_me(r["text"], board, move, r["score"])
            for pat, name in ((DEV, "developed"), (CENTER, "center"), (OPEN, "open file"), (GAIN, "block gain"),
                              (CASTLE, "castle"), (STAND, "standing"), (EQUAL, "equal"), (TOOK_LOOSE, "took loose")):
                if pat.search(r["text"]):
                    checked[name] += 1
            for m in STAND.finditer(r["text"]):
                if m.group(3):
                    checked["standing reason"] += 1
        elif r["lane"] == "you":
            problems = check_you(r["text"], board, move)
            for sentence in YOU_REASON:
                if sentence in r["text"]:
                    checked["you reason"] += 1
            if MISSED.search(r["text"]):
                checked["missed capture"] += 1
            for m in ANSWER.finditer(r["text"]):
                checked["answer check" if m.group(2) == "gives check" else "answer attack"] += 1
        else:
            problems = []
        if len(r["text"]) > LIMIT or not r["text"].endswith(".") or r["text"].startswith(("Game: ", "You: ", "Me: ")):
            problems.append("shape")
        for p in problems:
            wrong.append((r["game"], r["ply"], r["lane"], r["text"], p))
    print("checked: " + ", ".join(f"{k} {n}" for k, n in sorted(checked.items())))
    print(f"{len(wrong)} wrong claims in the quiet voice")
    for item in wrong[:15]:
        print("  wrong:", *item)

    # How the quiet moves of the every voice fare.
    quiet_every = [k for k, r in every_by.items() if r["lane"] == "me" and "I made a quiet move" in r["text"]]
    got_reason = sum(1 for k in quiet_every if k in quiet_by)
    said_quiet = sum(1 for k in quiet_by if "quiet move" in quiet_by[k]["text"])
    print(f"{len(quiet_every)} moves every called quiet: {got_reason} now say something, "
          f"{len(quiet_every) - got_reason} are silent, {said_quiet} lines still say 'quiet move'")

    # The grade says why.
    losing = [r for r in said["quiet"] if r["lane"] == "you" and class_of(r["text"]) in ("Inaccuracy", "Mistake", "Blunder")]
    why = [r for r in losing if "I can take your" in r["text"] or MISSED.search(r["text"])
           or any(s in r["text"] for s in YOU_REASON) or ANSWER.search(r["text"])]
    print(f"{len(losing)} You lines for an inaccuracy or worse in quiet; {len(why)} give a capture, an answer or a "
          f"block reason ({100 * len(why) / max(len(losing), 1):.0f}%)")
    mates = [r for r in said["quiet"] if r["lane"] == "you" and class_of(r["text"]) == "Mate"]
    print(f"{len(mates)} You lines about a mate")

    # The standing is said only when it changes.
    me_quiet = sorted((r for r in said["quiet"] if r["lane"] == "me"), key=lambda r: (r["game"], r["ply"]))
    words = collections.defaultdict(list)
    for r in me_quiet:
        m = STAND.search(r["text"]) or EQUAL.search(r["text"])
        if m:
            words[r["game"]].append(m.group(0).split(" because")[0].rstrip("."))
    repeats = sum(1 for seq in words.values() for a, b in zip(seq, seq[1:]) if a == b)
    stated = sum(len(seq) for seq in words.values())
    print(f"the standing is stated on {stated} of {tutor_moves} tutor moves ({stated / tutor_moves:.2f}); "
          f"{repeats} times the same words twice in a row")

    # B17: her lane's share, the cooldown, and the sentences it no longer says.
    me_rows = collections.defaultdict(list)
    for r in rows["quiet"]:
        if r["lane"] == "me" and r["kind"] in ("say", "skip"):
            me_rows[r["game"]].append(r)
    early, removed, threats = [], 0, 0
    for seq in me_rows.values():
        seq.sort(key=lambda r: r["ply"])
        last = None
        for i, r in enumerate(seq):
            if r["kind"] != "say":
                continue
            t = r["text"]
            if DEV.search(t) or GAIN.search(t):
                removed += 1
            if t.startswith("I am attacking your"):
                threats += 1
            always = t.startswith(ALWAYS) or "promoted to a" in t
            if not always and last is not None and i - last < COOLDOWN:
                early.append((r["game"], r["ply"], t))
            last = i
    spoken_me = lanes["quiet"]["me"]
    print(f"her lane speaks on {spoken_me} of {tutor_moves} of her moves ({spoken_me / tutor_moves:.1%}); "
          f"{threats} threat lines; {removed} development or block-purpose lines; "
          f"{len(early)} ordinary lines inside the cooldown")
    for item in early[:10]:
        print("  early:", *item)

    # Speed of the You lane.
    for v in ("every", "quiet"):
        waits = sorted(r["wait"] for r in rows[v] if r["lane"] == "you")
        p95 = waits[min(len(waits) - 1, int(len(waits) * 0.95))]
        print(f"{v:>5} You wait: median {statistics.median(waits):.3f} s, 95th {p95:.3f} s, longest {waits[-1]:.3f} s")

    with open(args.out, "w") as f:
        for v in ("every", "quiet"):
            for r in rows[v]:
                f.write(f"{v}\t{r['game']}\t{r['ply']}\t{r['lane']}\t{r['kind']}\t{r['text']}\n")
    print(f"wrote {args.out}")
    return 1 if wrong or missing or early or removed else 0


if __name__ == "__main__":
    sys.exit(main())
