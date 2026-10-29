//! Narration: one plain sentence about the move the search chose, for the game chat.
//!
//! The sentence comes from fixed templates over facts the engine already computes: the pieces that are loose
//! (the `loose` block's own list), the piece a capture takes, and the score, said as who is ahead and how many moves
//! the search looked ahead (see `standing` and `looked`). It runs once, after the search, on a
//! copy of the board; it cannot change the move. The same position always gets the same words.

use crate::blocks::loose::loose_pieces;
use crate::blocks::value;
use crate::board::{Board, Color, Move, MoveKind, Piece, Square, square_name};
use crate::search::{MATE, MATE_BOUND, Report};

/// Lichess refuses chat lines longer than this.
pub const MAX_CHARS: usize = 140;

/// A piece the sentence names, kept so a test can check the sentence against the board.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mention {
    pub side: Color,
    pub piece: Piece,
    pub square: Square,
    /// Whether the piece is on that square after the move (true) or before it (false).
    pub after: bool,
}

#[derive(Clone, Debug)]
pub struct Said {
    pub text: String,
    pub mentions: Vec<Mention>,
}

pub(crate) fn name(p: Piece) -> &'static str {
    match p {
        Piece::Pawn => "pawn",
        Piece::Knight => "knight",
        Piece::Bishop => "bishop",
        Piece::Rook => "rook",
        Piece::Queen => "queen",
        Piece::King => "king",
    }
}

/// A move from `before` in words: "knight to f3", "pawn takes knight on d5", "castling kingside".
pub fn move_words(before: &Board, m: Move) -> String {
    let mover = before.piece_on(m.from).map_or("piece", |(_, p)| name(p));
    let to = square_name(m.to);
    let plain = match m.kind {
        MoveKind::CastleKing => return "castling kingside".to_string(),
        MoveKind::CastleQueen => return "castling queenside".to_string(),
        MoveKind::EnPassant => format!("pawn takes pawn en passant on {}", to),
        MoveKind::Capture => {
            let taken = before.piece_on(m.to).map_or("piece", |(_, p)| name(p));
            format!("{} takes {} on {}", mover, taken, to)
        }
        MoveKind::Quiet | MoveKind::DoublePush => format!("{} to {}", mover, to),
    };
    match m.promo {
        Some(p) => format!("{} and promotes to a {}", plain, name(p)),
        None => plain,
    }
}

/// The squares of `side`'s loose pieces: attacked and either undefended or attacked by something cheaper.
pub(crate) fn loose(board: &Board, side: Color) -> Vec<Square> {
    loose_pieces(board, side, 1.0, 1.0)
        .into_iter()
        .map(|(sq, _)| sq)
        .collect()
}

/// "queen on d6, rook on f5 and 2 more": the two most valuable pieces by name, then a count.
pub(crate) fn list(
    board: &Board,
    squares: &[Square],
    side: Color,
    after: bool,
    said: &mut Vec<Mention>,
) -> String {
    let mut found: Vec<(Piece, Square)> = squares
        .iter()
        .filter_map(|&sq| board.piece_on(sq).map(|(_, p)| (p, sq)))
        .collect();
    found.sort_by(|a, b| value(b.0).total_cmp(&value(a.0)).then(a.1.cmp(&b.1)));
    let mut items: Vec<String> = Vec::new();
    for &(piece, square) in found.iter().take(2) {
        items.push(format!("{} on {}", name(piece), square_name(square)));
        said.push(Mention {
            side,
            piece,
            square,
            after,
        });
    }
    if found.len() > 2 {
        items.push(format!("{} more", found.len() - 2));
    }
    match items.len() {
        0 => String::new(),
        1 => items.remove(0),
        _ => {
            let last = items.pop().unwrap_or_default();
            format!("{} and {}", items.join(", "), last)
        }
    }
}

/// Whether the sentence still says "<piece> on <square>" (or "<piece> to <square>") for this mention.
pub(crate) fn appears(text: &str, m: &Mention) -> bool {
    let (piece, square) = (name(m.piece), square_name(m.square));
    text.contains(&format!("{} on {}", piece, square))
        || text.contains(&format!("{} to {}", piece, square))
}

pub(crate) fn is_are(n: usize) -> &'static str {
    if n == 1 { "is" } else { "are" }
}

pub(crate) fn not_in(a: &[Square], b: &[Square]) -> Vec<Square> {
    a.iter().copied().filter(|s| !b.contains(s)).collect()
}

/// Where each size word starts, in centipawns, largest first. Below the last the position is called equal.
pub(crate) const STANDING: [(i32, &str); 4] = [
    (350, "winning"),
    (180, "clearly better"),
    (80, "better"),
    (30, "slightly better"),
];

/// Who is ahead, in words. `score` is in centipawns for the side that just moved, which the sentence calls "I".
pub(crate) fn standing(score: i32) -> String {
    match STANDING.iter().find(|(from, _)| score.abs() >= *from) {
        None => "The position is about equal.".to_string(),
        Some((_, word)) => format!("{} {}.", if score > 0 { "I am" } else { "You are" }, word),
    }
}

/// How far the search looked, in moves. The search counts plies; a move is a ply from each side, and an odd depth
/// ends on the mover's own ply, so the count rounds up.
pub(crate) fn looked(depth: u32) -> String {
    let moves = depth.div_ceil(2);
    let unit = if moves == 1 { "move" } else { "moves" };
    format!("I looked {} {} ahead.", moves, unit)
}

/// The sentence for the move `report` chose from `board`, or `None` when there is no move.
pub fn say(board: &Board, report: &Report) -> Option<Said> {
    say_within(board, report, MAX_CHARS)
}

/// `say`, with the line held to `max` characters. The coach passes the chat limit.
pub fn say_within(board: &Board, report: &Report, max: usize) -> Option<Said> {
    let m = report.best()?;
    let me = board.turn;
    let opp = me.flip();
    let mut after = board.clone();
    after.make(m);

    let opp_before = loose(board, opp);
    let me_before = loose(board, me);
    let opp_after = loose(&after, opp);
    let me_after = loose(&after, me);

    let mut said: Vec<Mention> = Vec::new();
    let mut named_opp: Vec<Square> = Vec::new();
    let mate = report.score.abs() >= MATE_BOUND;
    let gives_check = after.in_check() && !mate;

    let captured = match m.kind {
        MoveKind::Capture => board.piece_on(m.to).map(|(_, p)| p),
        _ => None,
    };
    let promo = m
        .promo
        .map(|p| format!("promoted to a {}", name(p)))
        .unwrap_or_default();
    let new_threats = not_in(&opp_after, &opp_before);
    let rescued: Vec<Square> = me_before
        .iter()
        .copied()
        .filter(|s| *s != m.from && !me_after.contains(s))
        .collect();
    let moved_loose = me_before.contains(&m.from) && !me_after.contains(&m.to);

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
    } else if let Some(p) = captured {
        said.push(Mention {
            side: opp,
            piece: p,
            square: m.to,
            after: false,
        });
        let loose_word = if opp_before.contains(&m.to) {
            "loose "
        } else {
            ""
        };
        let tail = if promo.is_empty() {
            ".".to_string()
        } else {
            format!(" and {}.", promo)
        };
        Some(format!(
            "I took your {}{} on {}{}",
            loose_word,
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
        board
            .piece_on(m.from)
            .map(|(_, p)| format!("I moved my loose {} out of danger.", name(p)))
    } else if !rescued.is_empty() {
        let who = list(&after, &rescued[..1], me, true, &mut said);
        Some(format!("I defended my {}.", who))
    } else if gives_check {
        Some("I gave check.".to_string())
    } else if matches!(m.kind, MoveKind::CastleKing | MoveKind::CastleQueen) {
        let side = if m.kind == MoveKind::CastleKing {
            "kingside"
        } else {
            "queenside"
        };
        Some(format!("I castled {}.", side))
    } else if opp_after.is_empty() && me_after.is_empty() {
        Some("I made a quiet move.".to_string())
    } else {
        None
    };

    // Whatever else is loose after the move is worth saying, whichever side owns it.
    let mut clauses: Vec<String> = Vec::new();
    if gives_check && main.as_deref() != Some("I gave check.") {
        clauses.push("It gives check.".to_string());
    }
    if !mate {
        let rest = not_in(&opp_after, &named_opp);
        if !rest.is_empty() {
            let who = list(&after, &rest, opp, true, &mut said);
            clauses.push(format!("Your {} {} loose.", who, is_are(rest.len().min(2))));
        }
        if !me_after.is_empty() {
            let who = list(&after, &me_after, me, true, &mut said);
            clauses.push(format!(
                "My {} {} loose.",
                who,
                is_are(me_after.len().min(2))
            ));
        }
    }
    if main.is_none() && clauses.is_empty() {
        main = Some("I made a quiet move.".to_string());
    }

    let mut parts: Vec<String> = main.into_iter().chain(clauses).collect();
    // Who is ahead and how far the search looked, each dropped whole when the line would pass the chat limit.
    let evals: Vec<String> = if mate {
        vec![String::new()]
    } else {
        vec![
            format!("{} {}", standing(report.score), looked(report.depth)),
            standing(report.score),
            String::new(),
        ]
    };
    let mut text = String::new();
    for e in &evals {
        let mut all = parts.clone();
        if !e.is_empty() {
            all.push(e.clone());
        }
        let candidate = all.join(" ");
        if candidate.chars().count() <= max {
            text = candidate;
            break;
        }
    }
    if text.is_empty() {
        // Too long even without the score: keep whole clauses from the front.
        while parts.len() > 1 && parts.join(" ").chars().count() > max {
            parts.pop();
        }
        text = parts.join(" ").chars().take(max).collect();
    }
    // A mention that did not survive the trimming is not a claim the sentence makes.
    said.retain(|s| appears(&text, s));
    Some(Said {
        text,
        mentions: said,
    })
}

/// The sentence about `m`, a move already played from `before`. "You" is the side that moved, "my" the other side.
pub fn say_theirs(before: &Board, m: Move) -> Said {
    say_theirs_within(before, m, MAX_CHARS)
}

/// `say_theirs`, with the line held to `max` characters.
pub fn say_theirs_within(before: &Board, m: Move, max: usize) -> Said {
    theirs(before, m, max, false).unwrap_or_else(|| Said {
        text: String::new(),
        mentions: Vec::new(),
    })
}

/// The quiet form of `say_theirs_within`: `None` unless the move left a piece of the student's loose or attacked a
/// loose piece of the tutor's. A piece that stays loose from one move to the next is not said again.
pub fn say_theirs_quiet_within(before: &Board, m: Move, max: usize) -> Option<Said> {
    theirs(before, m, max, true)
}

fn theirs(before: &Board, m: Move, max: usize, quiet: bool) -> Option<Said> {
    let you = before.turn;
    let me = you.flip();
    let mut after = before.clone();
    after.make(m);

    // A piece that moves while loose is the same piece, so follow it to its new square.
    let yours_before: Vec<Square> = loose(before, you)
        .into_iter()
        .map(|s| if s == m.from { m.to } else { s })
        .collect();
    let mine_before = loose(before, me);
    let yours_after = loose(&after, you);
    let mine_after = loose(&after, me);

    let mut said: Vec<Mention> = Vec::new();
    let moved = before.piece_on(m.from).map(|(_, p)| p);
    let promo = m.promo.map(name);
    let main = match (m.kind, moved) {
        (MoveKind::Capture, Some(mover)) => match before.piece_on(m.to) {
            Some((_, taken)) => {
                said.push(Mention {
                    side: me,
                    piece: taken,
                    square: m.to,
                    after: false,
                });
                let loose_word = if mine_before.contains(&m.to) {
                    "loose "
                } else {
                    ""
                };
                let tail = promo
                    .map(|p| format!(" and promoted to a {}", p))
                    .unwrap_or_default();
                format!(
                    "You took my {}{} on {} with your {}{}.",
                    loose_word,
                    name(taken),
                    square_name(m.to),
                    name(mover),
                    tail
                )
            }
            None => "You took a piece.".to_string(),
        },
        (MoveKind::EnPassant, _) => "You took my pawn en passant.".to_string(),
        (MoveKind::CastleKing, _) => "You castled kingside.".to_string(),
        (MoveKind::CastleQueen, _) => "You castled queenside.".to_string(),
        (_, Some(mover)) => match m.promo {
            Some(p) => {
                said.push(Mention {
                    side: you,
                    piece: p,
                    square: m.to,
                    after: true,
                });
                format!("You promoted to a {} on {}.", name(p), square_name(m.to))
            }
            None => {
                said.push(Mention {
                    side: you,
                    piece: mover,
                    square: m.to,
                    after: true,
                });
                format!("You moved your {} to {}.", name(mover), square_name(m.to))
            }
        },
        (_, None) => "You moved.".to_string(),
    };

    let mut clauses: Vec<String> = Vec::new();
    if after.in_check() {
        clauses.push("It gives check.".to_string());
    }
    let new_yours = not_in(&yours_after, &yours_before);
    if !new_yours.is_empty() {
        let who = list(&after, &new_yours, you, true, &mut said);
        clauses.push(format!("That leaves your {} loose.", who));
    }
    let new_mine = not_in(&mine_after, &mine_before);
    if !new_mine.is_empty() {
        let who = list(&after, &new_mine, me, true, &mut said);
        clauses.push(format!(
            "It attacks my {}, which {} loose.",
            who,
            is_are(new_mine.len())
        ));
    }
    let old_yours: Vec<Square> = yours_after
        .iter()
        .copied()
        .filter(|s| yours_before.contains(s))
        .collect();
    if quiet && new_yours.is_empty() && new_mine.is_empty() {
        return None;
    }
    if !quiet && !old_yours.is_empty() {
        let who = list(&after, &old_yours, you, true, &mut said);
        clauses.push(format!(
            "Your {} {} still loose.",
            who,
            is_are(old_yours.len())
        ));
    }

    // Too long for the chat: the clauses at the end go first.
    let mut parts = vec![main];
    parts.extend(clauses);
    while parts.len() > 1 && parts.join(" ").chars().count() > max {
        parts.pop();
    }
    let text: String = parts.join(" ").chars().take(max).collect();
    said.retain(|s| appears(&text, s));
    Some(Said {
        text,
        mentions: said,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn sentence(fen: &str, uci: &str, score: i32) -> String {
        let mut b = Board::from_fen(fen).unwrap();
        let r = report(&mut b, uci, score);
        say(&b, &r).unwrap().text
    }

    #[test]
    fn taking_a_loose_piece_says_so() {
        let s = sentence("4k3/8/8/3q4/8/4N3/8/4K3 w - - 0 1", "e3d5", 900);
        assert_eq!(
            s,
            "I took your loose queen on d5. I am winning. I looked 3 moves ahead."
        );
    }

    #[test]
    fn a_new_threat_is_named_by_square() {
        let s = sentence("4k3/8/8/3q4/8/8/8/R3K3 w - - 0 1", "a1d1", 60);
        assert_eq!(
            s,
            "I am attacking your queen on d5, which is loose. I am slightly better. I looked 3 moves ahead."
        );
    }

    #[test]
    fn the_score_is_said_from_the_movers_side_whichever_color_that_is() {
        // Black to move in both. A positive score is good for the mover, so Black says "I", not a number from White's side.
        let ahead = sentence("4k3/8/8/8/8/8/8/4K3 b - - 0 1", "e8d8", 50);
        assert!(
            ahead.ends_with("I am slightly better. I looked 3 moves ahead."),
            "{}",
            ahead
        );
        let behind = sentence("4k3/8/8/8/8/8/8/4K3 b - - 0 1", "e8d8", -50);
        assert!(
            behind.ends_with("You are slightly better. I looked 3 moves ahead."),
            "{}",
            behind
        );
    }

    #[test]
    fn the_size_words_change_at_the_cut_points() {
        for (score, want) in [
            (0, "The position is about equal."),
            (29, "The position is about equal."),
            (-29, "The position is about equal."),
            (30, "I am slightly better."),
            (79, "I am slightly better."),
            (80, "I am better."),
            (179, "I am better."),
            (180, "I am clearly better."),
            (349, "I am clearly better."),
            (350, "I am winning."),
            (-30, "You are slightly better."),
            (-80, "You are better."),
            (-180, "You are clearly better."),
            (-350, "You are winning."),
        ] {
            assert_eq!(standing(score), want, "score {}", score);
        }
    }

    #[test]
    fn the_depth_is_said_in_moves() {
        for (depth, want) in [
            (1, "I looked 1 move ahead."),
            (2, "I looked 1 move ahead."),
            (3, "I looked 2 moves ahead."),
            (4, "I looked 2 moves ahead."),
            (5, "I looked 3 moves ahead."),
            (6, "I looked 3 moves ahead."),
            (7, "I looked 4 moves ahead."),
            (12, "I looked 6 moves ahead."),
        ] {
            assert_eq!(looked(depth), want, "depth {}", depth);
        }
    }

    #[test]
    fn no_number_from_the_old_wording_is_left() {
        for score in [-900, -120, 0, 45, 700] {
            let s = sentence("4k3/8/8/8/8/8/8/4K3 w - - 0 1", "e1e2", score);
            assert!(
                !s.contains("Eval") && !s.contains("depth") && !s.contains('+'),
                "{}",
                s
            );
        }
    }

    #[test]
    fn a_mate_is_called() {
        let s = sentence("6k1/5ppp/8/8/8/8/8/R3K3 w - - 0 1", "a1a8", MATE - 1);
        assert_eq!(s, "I see a forced mate in 1.");
    }

    #[test]
    fn a_check_is_called() {
        let s = sentence("4k3/8/8/8/8/8/8/R3K3 w - - 0 1", "a1a8", 20);
        assert_eq!(
            s,
            "I gave check. The position is about equal. I looked 3 moves ahead."
        );
    }

    fn theirs(fen: &str, uci: &str) -> String {
        let mut b = Board::from_fen(fen).unwrap();
        let mv = legal_moves(&mut b)
            .into_iter()
            .find(|m| m.to_string() == uci)
            .unwrap_or_else(|| panic!("{} is not legal here", uci));
        say_theirs(&b, mv).text
    }

    #[test]
    fn their_capture_names_the_piece_and_the_taker() {
        let s = theirs("4k3/8/8/3q4/8/4N3/8/4K3 w - - 0 1", "e3d5");
        assert_eq!(s, "You took my loose queen on d5 with your knight.");
    }

    #[test]
    fn their_move_and_check_are_called() {
        let s = theirs("4k3/8/8/8/8/8/8/R3K3 w - - 0 1", "a1a8");
        assert_eq!(s, "You moved your rook to a8. It gives check.");
    }

    #[test]
    fn their_castling_is_called() {
        let s = theirs("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1", "e1g1");
        assert_eq!(s, "You castled kingside.");
    }

    #[test]
    fn a_move_that_leaves_a_piece_loose_and_attacks_one_says_both() {
        let s = theirs("4k3/8/8/3q4/8/8/8/R3K3 b - - 0 1", "d5a8");
        assert_eq!(
            s,
            "You moved your queen to a8. That leaves your queen on a8 loose. It attacks my rook on a1, which is loose."
        );
    }

    #[test]
    fn a_quiet_position_says_so() {
        let s = sentence("4k3/8/8/8/8/8/8/4K3 w - - 0 1", "e1e2", 0);
        assert_eq!(
            s,
            "I made a quiet move. The position is about equal. I looked 3 moves ahead."
        );
    }

    #[test]
    fn a_loose_piece_left_alone_is_still_named() {
        // White to move; the black queen on d5 is loose but the move is a quiet king step.
        let s = sentence("4k3/8/8/3q4/8/4N3/8/4K3 w - - 0 1", "e1e2", 100);
        assert!(s.contains("Your queen on d5 is loose."), "{}", s);
    }

    #[test]
    fn no_sentence_runs_past_the_chat_limit() {
        let s = sentence(
            "r1bqkb1r/pppp1ppp/2n2n2/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R w KQkq - 4 4",
            "f3e5",
            250,
        );
        assert!(s.chars().count() <= MAX_CHARS, "{}", s);
    }
}
