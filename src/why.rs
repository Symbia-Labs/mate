//! Reasons (B16): the quiet voice, and the facts it may give as a reason.
//!
//! A reason is a fact about the board that the engine can check. Two kinds are used. A tactical fact is one the
//! `narrate` sentences already state (a loose piece, a capture, a mate). A block fact is a change in one evaluation
//! block, read by running that block alone: her evaluation is the sum of five blocks, so "my pieces have more room"
//! is the mobility block's own number going up, not a guess about it. Three opening facts are also used, because the
//! move alone proves them: a knight or bishop leaves its starting square, a pawn reaches the center, a rook reaches
//! a file with no pawns on it.
//!
//! Nothing here changes what she plays. `me_quiet` is the Me lane's sentence for the quiet voice; `you_reason`
//! is the clause the You lane adds to a grade. The Game lane's quiet form is `narrate::say_theirs_quiet_within`.
//!
//! B17: the Me lane no longer says development or block gains, and its ordinary lines wait for `COOLDOWN` of her
//! moves. A mate, a capture of a loose piece and a promotion are always said.

use crate::blocks::value;
use crate::board::{Board, Color, Move, MoveKind, Piece, Square, square_name};
use crate::graph::Graph;
use crate::judge::Verdict;
use crate::narrate::{
    Mention, Said, appears, is_are, list, looked, loose, move_words, name, not_in,
};
use crate::search::{MATE, MATE_BOUND, Report};
use crate::tables::FILE_A;

/// How far a block's number must move for a move to be credited with it, in centipawns.
pub const MOBILITY_CP: f32 = 20.0;
pub const KING_CP: f32 = 25.0;
pub const PAWNS_CP: f32 = 20.0;

/// A pawn that reaches the center is said through this move number.
pub const DEVELOPMENT_MOVES: u16 = 20;

/// An ordinary Me line is said only when she has made at least this many moves since her last Me line, so two of her
/// moves pass in silence between ordinary lines (B17). A mate, a capture of a loose piece and a promotion do not wait.
pub const COOLDOWN: usize = 3;

/// Whether an ordinary Me line must wait. `spoke` holds, for each of her earlier moves in order, whether she said a
/// line on it.
pub fn cooling(spoke: &[bool]) -> bool {
    spoke
        .iter()
        .rposition(|s| *s)
        .is_some_and(|last| spoke.len() - last < COOLDOWN)
}

/// Material counts as the reason for a lead when it is at least this much, and between these shares of the lead.
pub const MATERIAL_CP: f32 = 90.0;
const SHARE_LOW: f32 = 0.4;
const SHARE_HIGH: f32 = 2.0;

/// Where each standing word starts, by level 1 to 4. Level 0 is equal. Level 5 is a mate.
const LEVEL_FROM: [i32; 5] = [0, 30, 80, 180, 350];
const LEVEL_WORD: [&str; 5] = [
    "equal",
    "slightly better",
    "better",
    "clearly better",
    "winning",
];

/// A score has to pass the edge of a standing word by this much before the word changes, so a score that sits on
/// an edge does not flip the word every move.
const HOLD_CP: i32 = 10;

/// The evaluation blocks that can be a reason, each in its own graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Term {
    Mobility,
    King,
    Pawns,
}

const TERMS: [Term; 3] = [Term::Mobility, Term::King, Term::Pawns];

impl Term {
    fn block(self) -> &'static str {
        match self {
            Term::Mobility => r#"{"block":"mobility"}"#,
            Term::King => r#"{"block":"king"}"#,
            Term::Pawns => r#"{"block":"pawns"}"#,
        }
    }

    pub fn threshold(self) -> f32 {
        match self {
            Term::Mobility => MOBILITY_CP,
            Term::King => KING_CP,
            Term::Pawns => PAWNS_CP,
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// Each side's number from each block, for one position. A block's number is positive when it is good for that side
/// (the king block's is minus the exposure).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Reading {
    terms: [[f32; 2]; 3],
}

impl Reading {
    pub fn get(&self, term: Term, side: Color) -> f32 {
        self.terms[term.index()][side as usize]
    }
}

/// The blocks, one graph each, so a position can be read block by block.
pub struct Terms {
    graphs: Vec<Graph>,
}

impl Terms {
    pub fn new() -> Result<Terms, String> {
        let graphs = TERMS
            .iter()
            .map(|t| Graph::from_json(t.block()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Terms { graphs })
    }

    pub fn read(&mut self, board: &Board) -> Reading {
        let mut r = Reading::default();
        for (i, g) in self.graphs.iter_mut().enumerate() {
            let f = g.eval(board, 0);
            r.terms[i] = f.ch;
        }
        r
    }
}

/// What a move did to the position, in one of the blocks, from the point of view of the side that moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gain {
    MyRoom,
    MyKing,
    MyPawns,
    TheirRoom,
    TheirKing,
    TheirPawns,
}

impl Gain {
    /// The block this gain is read from.
    pub fn term(self) -> Term {
        match self {
            Gain::MyRoom | Gain::TheirRoom => Term::Mobility,
            Gain::MyKing | Gain::TheirKing => Term::King,
            Gain::MyPawns | Gain::TheirPawns => Term::Pawns,
        }
    }

    /// The purpose the move served, to follow "to": "give my pieces more room".
    pub fn purpose(self) -> &'static str {
        match self {
            Gain::MyRoom => "give my pieces more room",
            Gain::MyKing => "make my king safer",
            Gain::MyPawns => "improve my pawn structure",
            Gain::TheirRoom => "cramp your pieces",
            Gain::TheirKing => "expose your king",
            Gain::TheirPawns => "weaken your pawns",
        }
    }
}

/// The largest change a move made, in units of its block's threshold, when any block moved by its threshold.
pub fn best_gain(before: &Reading, after: &Reading, me: Color) -> Option<Gain> {
    let opp = me.flip();
    let mut best: Option<(f32, Gain)> = None;
    for term in TERMS {
        let (mine, theirs) = match term {
            Term::Mobility => (Gain::MyRoom, Gain::TheirRoom),
            Term::King => (Gain::MyKing, Gain::TheirKing),
            Term::Pawns => (Gain::MyPawns, Gain::TheirPawns),
        };
        let up = after.get(term, me) - before.get(term, me);
        let down = before.get(term, opp) - after.get(term, opp);
        for (change, gain) in [(up, mine), (down, theirs)] {
            if change >= term.threshold() {
                let size = change / term.threshold();
                if best.is_none_or(|(s, _)| size > s) {
                    best = Some((size, gain));
                }
            }
        }
    }
    best.map(|(_, g)| g)
}

fn file_of(sq: Square) -> u8 {
    sq % 8
}

fn file_letter(sq: Square) -> char {
    (b'a' + file_of(sq)) as char
}

/// Whether no pawn of either side stands on the file of `sq`.
fn open_file(board: &Board, sq: Square) -> bool {
    let pawns = board.pieces(Color::White, Piece::Pawn) | board.pieces(Color::Black, Piece::Pawn);
    pawns & (FILE_A << file_of(sq)) == 0
}

fn is_center(sq: Square) -> bool {
    matches!(sq, 27 | 28 | 35 | 36)
}

fn count_words(n: u32, piece: Piece) -> String {
    let word = name(piece);
    match n {
        1 => format!("a {}", word),
        2 => format!("two {}s", word),
        3 => format!("three {}s", word),
        n => format!("{} {}s", n, word),
    }
}

/// The signed material difference for `side` (own minus the other's), by piece, pawn to queen.
fn material_diff(board: &Board, side: Color) -> [i32; 5] {
    let mut d = [0i32; 5];
    for (i, p) in [
        Piece::Pawn,
        Piece::Knight,
        Piece::Bishop,
        Piece::Rook,
        Piece::Queen,
    ]
    .iter()
    .enumerate()
    {
        d[i] = board.pieces(side, *p).count_ones() as i32
            - board.pieces(side.flip(), *p).count_ones() as i32;
    }
    d
}

const MATERIAL_PIECES: [Piece; 5] = [
    Piece::Pawn,
    Piece::Knight,
    Piece::Bishop,
    Piece::Rook,
    Piece::Queen,
];

/// How much more material `side` has, in centipawns, and the words for it: "up a knight", "up the exchange" or
/// "ahead in material".
fn material_lead(board: &Board, side: Color) -> (f32, String) {
    let d = material_diff(board, side);
    let cp: f32 = d
        .iter()
        .zip(MATERIAL_PIECES.iter())
        .map(|(n, p)| *n as f32 * value(*p))
        .sum();
    if d.iter().all(|n| *n >= 0) {
        // Largest pieces first, at most two kinds.
        let mut items: Vec<String> = Vec::new();
        for i in (0..5).rev() {
            if d[i] > 0 && items.len() < 2 {
                items.push(count_words(d[i] as u32, MATERIAL_PIECES[i]));
            }
        }
        if !items.is_empty() && d.iter().filter(|n| **n > 0).count() <= 2 {
            return (cp, format!("up {}", items.join(" and ")));
        }
    }
    (cp, "ahead in material".to_string())
}

/// The reason a side leads by `score` centipawns (positive: the side that just moved, `me`), if the board gives
/// one: material first, then the block that leads by the most. `skip` is a block the line has already given as the
/// reason for the move, which is not said twice.
fn lead_reason(
    after: &Board,
    reading: &Reading,
    me: Color,
    score: i32,
    skip: Option<Term>,
) -> Option<String> {
    let leader = if score > 0 { me } else { me.flip() };
    let other = leader.flip();
    let size = score.abs() as f32;
    let first = leader == me;
    let (cp, words) = material_lead(after, leader);
    if cp >= MATERIAL_CP && cp >= SHARE_LOW * size && cp <= SHARE_HIGH * size {
        return Some(if first {
            format!("I am {}", words)
        } else {
            format!("you are {}", words)
        });
    }
    let mut best: Option<(f32, Term)> = None;
    for term in TERMS {
        if skip == Some(term) {
            continue;
        }
        let lead = reading.get(term, leader) - reading.get(term, other);
        if lead >= term.threshold().max(SHARE_LOW * size) {
            let ratio = lead / term.threshold();
            if best.is_none_or(|(r, _)| ratio > r) {
                best = Some((ratio, term));
            }
        }
    }
    best.map(|(_, term)| {
        match (term, first) {
            (Term::Mobility, true) => "my pieces have more room",
            (Term::Mobility, false) => "your pieces have more room",
            (Term::King, true) => "your king is more exposed",
            (Term::King, false) => "my king is more exposed",
            (Term::Pawns, true) => "my pawn structure is better",
            (Term::Pawns, false) => "your pawn structure is better",
        }
        .to_string()
    })
}

/// The standing level of a score, from the side that moved: 0 equal, 1 to 4 for the words, 5 for a mate, negative
/// when the other side is ahead.
pub fn level_of(score: i32) -> i8 {
    if score.abs() >= MATE_BOUND {
        return if score > 0 { 5 } else { -5 };
    }
    let a = score.abs();
    let l = (0..5).rev().find(|i| a >= LEVEL_FROM[*i]).unwrap_or(0) as i8;
    if score < 0 { -l } else { l }
}

/// The level after `score`, given the level she last held. A level changes only when the score has passed the edge
/// of the word by `HOLD_CP`.
pub fn next_level(prev: i8, score: i32) -> i8 {
    let raw = level_of(score);
    if raw == prev || raw.abs() == 5 || prev.abs() == 5 {
        return raw;
    }
    let same_side = prev == 0 || raw == 0 || raw.signum() == prev.signum();
    if !same_side {
        return raw;
    }
    // The score is measured along the side that was ahead, so a score on the other side always leaves the level
    // (B17: before, "I am slightly better" could hold at -24, and a standing said late could say it).
    let a = if prev == 0 {
        score.abs()
    } else {
        score * i32::from(prev.signum())
    };
    let (p, r) = (prev.unsigned_abs() as usize, raw.unsigned_abs() as usize);
    if r > p {
        // A jump of more than one word lands on the highest word whose edge the score has passed by HOLD_CP (B17:
        // before, a jump that fell short of the far word kept the old one, "about equal" at +182).
        (p + 1..=r)
            .rev()
            .find(|l| a >= LEVEL_FROM[*l] + HOLD_CP)
            .map_or(prev, |l| raw.signum() * l as i8)
    } else if a <= LEVEL_FROM[p] - HOLD_CP {
        raw
    } else {
        prev
    }
}

/// What she remembers of who is ahead. `held` is the level the score has settled on; `spoken` is the level she last
/// said aloud. The standing is said when the two differ, so a line that had to be cut is tried again on the next
/// move, and a level that comes back before it was said is never said.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Standing {
    pub held: i8,
    pub spoken: i8,
}

/// The words for a change of standing, longest first.
fn standing_forms(
    after: &Board,
    reading: &Reading,
    me: Color,
    score: i32,
    level: i8,
    depth: u32,
    skip: Option<Term>,
) -> Vec<String> {
    if level == 0 {
        return vec![
            format!("The position is about equal. {}", looked(depth)),
            "The position is about equal.".to_string(),
        ];
    }
    let (subject, word) = (
        if level > 0 { "I am" } else { "You are" },
        LEVEL_WORD[level.unsigned_abs() as usize],
    );
    let mut forms = Vec::new();
    if let Some(reason) = lead_reason(after, reading, me, score, skip) {
        forms.push(format!("{} {} because {}.", subject, word, reason));
    }
    forms.push(format!("{} {}. {}", subject, word, looked(depth)));
    forms.push(format!("{} {}.", subject, word));
    forms
}

fn castle_side(m: &Move) -> &'static str {
    if m.kind == MoveKind::CastleKing {
        "kingside"
    } else {
        "queenside"
    }
}

/// The Me lane's sentence for the quiet voice, and what to remember of the standing. `prev` is what she remembered
/// after her last move. `wait` is true while the cooldown runs (see `cooling`): then only a mate, a capture of a
/// loose piece or a promotion is said. `None` when the move has nothing to say or its line must wait.
pub fn me_quiet(
    terms: &mut Terms,
    board: &Board,
    report: &Report,
    prev: Standing,
    wait: bool,
    max: usize,
) -> (Option<Said>, Standing) {
    let Some(m) = report.best() else {
        return (None, prev);
    };
    let me = board.turn;
    let opp = me.flip();
    let mut after = board.clone();
    after.make(m);

    let opp_before = loose(board, opp);
    let me_before: Vec<Square> = loose(board, me)
        .into_iter()
        .map(|s| if s == m.from { m.to } else { s })
        .collect();
    let opp_after = loose(&after, opp);
    let me_after = loose(&after, me);
    let me_before_raw = loose(board, me);

    let before_reading = terms.read(board);
    let after_reading = terms.read(&after);

    let mut said: Vec<Mention> = Vec::new();
    let mut named_opp: Vec<Square> = Vec::new();
    let mate = report.score.abs() >= MATE_BOUND;
    let gives_check = after.in_check() && !mate;
    let is_capture = matches!(m.kind, MoveKind::Capture | MoveKind::EnPassant);
    let captured = match m.kind {
        MoveKind::Capture => board.piece_on(m.to).map(|(_, p)| p),
        _ => None,
    };
    let promo = m
        .promo
        .map(|p| format!("promoted to a {}", name(p)))
        .unwrap_or_default();
    let new_threats = not_in(&opp_after, &opp_before);
    let rescued: Vec<Square> = me_before_raw
        .iter()
        .copied()
        .filter(|s| *s != m.from && !me_after.contains(s))
        .collect();
    let moved_loose = me_before_raw.contains(&m.from) && !me_after.contains(&m.to);
    let mover = board.piece_on(m.from).map(|(_, p)| p);

    // A capture is said when it takes a loose piece. A capture of a protected piece is a trade and is not said.
    let loose_capture = captured.filter(|_| opp_before.contains(&m.to));
    // The block the line gives as the reason for the move, so the standing does not give it again.
    let mut reason_term: Option<Term> = None;

    let mut main: Option<String> = if report.score >= MATE_BOUND {
        Some(format!(
            "I see a forced mate in {}.",
            (MATE - report.score + 1) / 2
        ))
    } else if report.score <= -MATE_BOUND {
        Some(format!(
            "I may be getting mated in {}; this is my best defense.",
            (MATE + report.score + 1) / 2
        ))
    } else if let Some(p) = loose_capture {
        said.push(Mention {
            side: opp,
            piece: p,
            square: m.to,
            after: false,
        });
        let tail = if promo.is_empty() {
            ".".to_string()
        } else {
            format!(" and {}.", promo)
        };
        Some(format!(
            "I took your loose {} on {}{}",
            name(p),
            square_name(m.to),
            tail
        ))
    } else if !promo.is_empty() {
        Some(format!("I {}.", promo))
    } else if !new_threats.is_empty() {
        let who = list(&after, &new_threats, opp, true, &mut said);
        named_opp = new_threats.clone();
        Some(format!(
            "I am attacking your {}, which {} loose.",
            who,
            is_are(new_threats.len().min(2))
        ))
    } else if moved_loose {
        mover.map(|p| format!("I moved my loose {} out of danger.", name(p)))
    } else if !rescued.is_empty() {
        let who = list(&after, &rescued[..1], me, true, &mut said);
        Some(format!("I defended my {}.", who))
    } else if matches!(m.kind, MoveKind::CastleKing | MoveKind::CastleQueen) {
        let safer =
            after_reading.get(Term::King, me) - before_reading.get(Term::King, me) >= KING_CP;
        if safer {
            reason_term = Some(Term::King);
        }
        Some(if safer {
            format!("I castled {} to make my king safer.", castle_side(&m))
        } else {
            format!("I castled {}.", castle_side(&m))
        })
    } else if is_capture {
        None
    } else if let Some(piece) = mover {
        let words = move_words(board, m);
        let mention = Mention {
            side: me,
            piece,
            square: m.to,
            after: true,
        };
        // B17: a developed piece and a block gain are no longer said; the center and an open file still are.
        let reason =
            if piece == Piece::Pawn && is_center(m.to) && board.fullmove <= DEVELOPMENT_MOVES {
                Some(format!("I played {} to take space in the center.", words))
            } else if piece == Piece::Rook
                && file_of(m.from) != file_of(m.to)
                && open_file(&after, m.to)
            {
                Some(format!(
                    "I played {} to use the open {}-file.",
                    words,
                    file_letter(m.to)
                ))
            } else {
                None
            };
        if reason.is_some() {
            said.push(mention);
        }
        reason
    } else {
        None
    };

    // Loose pieces that are new are worth saying, whichever side owns them; one that stays loose is not said again.
    let mut clauses: Vec<String> = Vec::new();
    if !mate {
        let rest: Vec<Square> = not_in(&opp_after, &opp_before)
            .into_iter()
            .filter(|s| !named_opp.contains(s))
            .collect();
        if !rest.is_empty() {
            let who = list(&after, &rest, opp, true, &mut said);
            clauses.push(format!("Your {} {} loose.", who, is_are(rest.len().min(2))));
        }
        let mut mine: Vec<Square> = not_in(&me_after, &me_before);
        if is_capture {
            mine.retain(|s| *s != m.to);
        }
        if !mine.is_empty() {
            let who = list(&after, &mine, me, true, &mut said);
            clauses.push(format!("My {} {} loose.", who, is_are(mine.len().min(2))));
        }
    }

    let held = next_level(prev.held, report.score);
    let tails: Vec<String> = if !mate && held != prev.spoken {
        standing_forms(
            &after,
            &after_reading,
            me,
            report.score,
            held,
            report.depth,
            reason_term,
        )
    } else {
        Vec::new()
    };
    let silent = Standing {
        held,
        spoken: prev.spoken,
    };

    if main.is_none() && clauses.is_empty() && tails.is_empty() {
        return (None, silent);
    }
    // B17: while the cooldown runs only a mate, a capture of a loose piece or a promotion is said. A standing that
    // waits is said with her next line, because `spoken` is left as it was.
    let always = mate || loose_capture.is_some() || !promo.is_empty();
    if wait && !always {
        return (None, silent);
    }
    if gives_check && (main.is_some() || !clauses.is_empty() || !tails.is_empty()) {
        clauses.insert(0, "It gives check.".to_string());
    }

    let mut parts: Vec<String> = main.take().into_iter().chain(clauses).collect();
    let mut text = String::new();
    let mut said_standing = false;
    for tail in tails.iter().map(Some).chain(std::iter::once(None)) {
        let mut all = parts.clone();
        if let Some(t) = tail {
            all.push(t.clone());
        }
        let candidate = all.join(" ");
        if !candidate.is_empty() && candidate.chars().count() <= max {
            text = candidate;
            said_standing = tail.is_some();
            break;
        }
    }
    if text.is_empty() {
        while parts.len() > 1 && parts.join(" ").chars().count() > max {
            parts.pop();
        }
        text = parts.join(" ").chars().take(max).collect();
    }
    if text.is_empty() {
        return (None, silent);
    }
    said.retain(|s| appears(&text, s));
    (
        Some(Said {
            text,
            mentions: said,
        }),
        Standing {
            held,
            spoken: if said_standing { held } else { prev.spoken },
        },
    )
}

/// What the student's move did to his own position, when it made one block worse by its threshold: the clause the
/// You lane puts after the grade. The largest change wins.
pub fn you_reason(terms: &mut Terms, before: &Board, v: &Verdict) -> Option<String> {
    let you = before.turn;
    // A free piece he did not take: the best move captures a piece of hers that nothing guards.
    if v.best.to != v.played.to
        && v.best.kind == MoveKind::Capture
        && loose(before, you.flip()).contains(&v.best.to)
        && let Some((_, piece)) = before.piece_on(v.best.to)
    {
        return Some(format!(
            "You could have taken my loose {} on {}.",
            name(piece),
            square_name(v.best.to)
        ));
    }
    let mut after = before.clone();
    after.make(v.played);
    // What her best answer to his move does: it gives check, or it attacks a piece of his that nothing guards.
    if let Some(reply) = v.reply
        && reply.kind != MoveKind::Capture
    {
        let words = move_words(&after, reply);
        let mut answered = after.clone();
        answered.make(reply);
        if answered.in_check() {
            return Some(format!("I can answer with {}, which gives check.", words));
        }
        let was = loose(&after, you);
        let hit = loose(&answered, you).into_iter().find(|s| {
            !was.contains(s) && answered.attackers(*s, you.flip()) & (1u64 << reply.to) != 0
        });
        if let Some(sq) = hit
            && let Some((_, piece)) = answered.piece_on(sq)
        {
            return Some(format!(
                "I can answer with {}, which attacks your {} on {}.",
                words,
                name(piece),
                square_name(sq)
            ));
        }
    }
    let was = terms.read(before);
    let now = terms.read(&after);
    let mut worst: Option<(f32, Term)> = None;
    for term in TERMS {
        let loss = was.get(term, you) - now.get(term, you);
        if loss >= term.threshold() {
            let size = loss / term.threshold();
            if worst.is_none_or(|(s, _)| size > s) {
                worst = Some((size, term));
            }
        }
    }
    worst.map(|(_, term)| {
        match term {
            Term::Mobility => "It cramps your pieces.",
            Term::King => "It leaves your king more exposed.",
            Term::Pawns => "It weakens your pawn structure.",
        }
        .to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::judge::Class;
    use crate::movegen::legal_moves;
    use std::time::Duration;

    fn report(b: &mut Board, uci: &str, score: i32) -> Report {
        let mv = legal_moves(b)
            .into_iter()
            .find(|m| m.to_string() == uci)
            .unwrap_or_else(|| panic!("{} is not legal here", uci));
        Report {
            depth: 5,
            score,
            nodes: 0,
            elapsed: Duration::ZERO,
            pv: vec![mv],
        }
    }

    fn line(fen: &str, uci: &str, score: i32, prev: Standing) -> (Option<String>, Standing) {
        line_with(fen, uci, score, prev, false)
    }

    /// `line` with the cooldown running (`wait`) or not.
    fn line_with(
        fen: &str,
        uci: &str,
        score: i32,
        prev: Standing,
        wait: bool,
    ) -> (Option<String>, Standing) {
        let mut b = Board::from_fen(fen).unwrap();
        let r = report(&mut b, uci, score);
        let mut t = Terms::new().unwrap();
        let (said, now) = me_quiet(&mut t, &b, &r, prev, wait, 140);
        (said.map(|s| s.text), now)
    }

    fn at(level: i8) -> Standing {
        Standing {
            held: level,
            spoken: level,
        }
    }

    const START: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";
    const E4_E5: &str = "rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 2";
    const NF3_NC6: &str = "r1bqkbnr/pppp1ppp/2n5/4p3/4P3/5N2/PPPP1PPP/RNBQKB1R w KQkq - 2 3";
    const RUY: &str = "r1bqkbnr/1ppp1ppp/p1n5/1B2p3/4P3/5N2/PPPP1PPP/RNBQK2R w KQkq - 0 4";
    const KNIGHT_QUEEN: &str = "4k3/8/8/3q4/8/4N3/8/4K3 w - - 0 1";
    const ITALIAN: &str = "r1bqk2r/pppp1ppp/2n2n2/2b1p3/2B1P3/5N2/PPPP1PPP/RNBQK2R w KQkq - 4 4";

    #[test]
    fn k1_a_center_pawn_says_why() {
        assert_eq!(
            line(START, "e2e4", 25, at(0)).0.as_deref(),
            Some("I played pawn to e4 to take space in the center.")
        );
    }

    #[test]
    fn k3_a_developed_bishop_is_no_longer_said() {
        // B16 said "I developed my bishop to b5."; B17 drops development.
        assert_eq!(line(NF3_NC6, "f1b5", 30, at(0)).0, None);
    }

    #[test]
    fn k4_a_trade_is_not_announced() {
        assert_eq!(line(RUY, "b5c6", 10, at(0)).0, None);
    }

    #[test]
    fn k5_the_standing_comes_with_its_reason() {
        let (text, now) = line(KNIGHT_QUEEN, "e3d5", 320, at(0));
        assert_eq!(
            text.as_deref(),
            Some("I took your loose queen on d5. I am clearly better because I am up a knight.")
        );
        assert_eq!(now, at(3));
    }

    #[test]
    fn k6_the_same_standing_is_not_said_again() {
        let (text, now) = line(KNIGHT_QUEEN, "e3d5", 320, at(3));
        assert_eq!(text.as_deref(), Some("I took your loose queen on d5."));
        assert_eq!(now, at(3));
    }

    #[test]
    fn a_standing_that_came_back_before_it_was_said_is_never_said() {
        // She said "clearly better" (3); the score has since settled on 4 without a word, and now it is back at 3.
        let prev = Standing { held: 4, spoken: 3 };
        let (text, now) = line(KNIGHT_QUEEN, "e3d5", 320, prev);
        assert_eq!(text.as_deref(), Some("I took your loose queen on d5."));
        assert_eq!(now, at(3));
    }

    #[test]
    fn k7_castling_is_not_called_safer_when_the_block_says_otherwise() {
        let (text, _) = line(ITALIAN, "e1g1", 20, at(0));
        assert_eq!(text.as_deref(), Some("I castled kingside."));
    }

    #[test]
    fn a_score_on_the_edge_of_a_word_does_not_flip_it() {
        assert_eq!(next_level(0, 29), 0);
        assert_eq!(next_level(0, 35), 0, "5 past the edge is not enough");
        assert_eq!(next_level(0, 41), 1);
        assert_eq!(next_level(1, 25), 1, "5 under the edge holds");
        assert_eq!(next_level(1, 19), 0);
        assert_eq!(next_level(1, -50), -1, "the other side is slightly better");
        assert_eq!(next_level(1, -100), -2);
        assert_eq!(level_of(MATE - 3), 5);
    }

    #[test]
    fn b17_a_score_on_the_other_side_leaves_the_level() {
        assert_eq!(
            next_level(1, -24),
            0,
            "the other side is ahead, if only a little"
        );
        assert_eq!(next_level(-1, 24), 0);
        assert_eq!(next_level(2, -50), -1);
        assert_eq!(
            next_level(1, 24),
            1,
            "still inside the hold on her own side"
        );
    }

    #[test]
    fn b17_a_jump_of_more_than_one_word_lands_on_the_highest_word_passed() {
        assert_eq!(
            next_level(0, 182),
            2,
            "past better, short of clearly better"
        );
        assert_eq!(next_level(2, 351), 3);
        assert_eq!(next_level(0, -182), -2);
        assert_eq!(next_level(0, 35), 0, "short of the first edge");
        assert_eq!(next_level(0, 41), 1);
        assert_eq!(next_level(0, 400), 4);
    }

    #[test]
    fn the_start_position_reads_level_in_every_block() {
        let mut t = Terms::new().unwrap();
        let r = t.read(&Board::start());
        for term in TERMS {
            assert_eq!(r.get(term, Color::White), r.get(term, Color::Black));
        }
        let _ = E4_E5;
    }

    const THREAT: &str = "4k3/8/8/3q4/8/8/8/R3K3 w - - 0 1";
    const BARE: &str = "4k3/8/8/8/8/8/8/4K3 w - - 0 1";

    #[test]
    fn b17_the_cooldown_counts_her_moves_since_her_last_line() {
        assert!(!cooling(&[]), "nothing said yet");
        assert!(!cooling(&[false, false]), "nothing said yet");
        assert!(cooling(&[true]), "she spoke on her last move");
        assert!(cooling(&[true, false]), "one silent move since");
        assert!(!cooling(&[true, false, false]), "two silent moves since");
        assert!(cooling(&[true, false, false, true]), "she spoke again");
    }

    #[test]
    fn b17_an_ordinary_line_waits() {
        assert_eq!(line_with(START, "e2e4", 25, at(0), true), (None, at(0)));
    }

    #[test]
    fn b17_a_threat_waits_and_is_said_when_the_cooldown_is_over() {
        assert_eq!(line_with(THREAT, "a1d1", 60, at(1), true).0, None);
        assert_eq!(
            line_with(THREAT, "a1d1", 60, at(1), false).0.as_deref(),
            Some("I am attacking your queen on d5, which is loose.")
        );
    }

    #[test]
    fn b17_a_capture_of_a_loose_piece_does_not_wait() {
        let (text, now) = line_with(KNIGHT_QUEEN, "e3d5", 320, at(0), true);
        assert_eq!(
            text.as_deref(),
            Some("I took your loose queen on d5. I am clearly better because I am up a knight.")
        );
        assert_eq!(now, at(3));
    }

    #[test]
    fn b17_a_standing_that_waits_is_said_with_her_next_line() {
        let (text, now) = line_with(BARE, "e1e2", 100, at(0), true);
        assert_eq!(text, None);
        assert_eq!(now, Standing { held: 2, spoken: 0 });
        let (text, now) = line_with(BARE, "e1e2", 100, now, false);
        assert_eq!(
            text.as_deref(),
            Some("I am better. I looked 3 moves ahead.")
        );
        assert_eq!(now, at(2));
    }

    fn find(b: &mut Board, uci: &str) -> Move {
        legal_moves(b)
            .into_iter()
            .find(|m| m.to_string() == uci)
            .unwrap_or_else(|| panic!("{} is not legal here", uci))
    }

    /// The You reason for the student's move `played` (best was `best`), when her best answer is `reply`.
    fn answer(fen: &str, played: &str, best: &str, reply: &str) -> Option<String> {
        let mut b = Board::from_fen(fen).unwrap();
        let played = find(&mut b, played);
        let best = find(&mut b, best);
        let mut after = b.clone();
        after.make(played);
        let reply = find(&mut after, reply);
        let v = Verdict {
            class: Class::Mistake,
            cost: 200,
            played,
            best,
            best_score: 0,
            played_score: -200,
            depth: 5,
            reply: Some(reply),
        };
        let mut t = Terms::new().unwrap();
        you_reason(&mut t, &b, &v)
    }

    const CHECK_REPLY: &str = "4k3/8/8/8/8/8/r7/4K3 w - - 0 1";
    const ATTACK_REPLY: &str = "4k2r/8/8/8/8/2N5/8/4K3 w - - 0 1";

    #[test]
    fn the_answer_that_gives_check_is_named() {
        let text = answer(CHECK_REPLY, "e1f1", "e1d1", "a2a1");
        assert_eq!(
            text.as_deref(),
            Some("I can answer with rook to a1, which gives check.")
        );
    }

    #[test]
    fn the_answer_that_attacks_an_unguarded_piece_is_named() {
        let text = answer(ATTACK_REPLY, "e1e2", "e1d1", "h8h3");
        assert_eq!(
            text.as_deref(),
            Some("I can answer with rook to h3, which attacks your knight on c3.")
        );
    }

    #[test]
    fn a_quiet_answer_is_not_named() {
        let text = answer(ATTACK_REPLY, "e1e2", "e1d1", "h8h7");
        assert!(
            text.is_none_or(|t| !t.contains("I can answer")),
            "a rook move that does nothing is not a reason"
        );
    }
}
