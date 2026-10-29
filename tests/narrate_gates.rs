//! Gates for B6, the sentence about each move. They run the 300 loose-piece positions of
//! `tests/fixtures/puzzles_sample.json` to depth 4.
//!
//! 1. The words do not touch the move: the `uci` binary plays the same move with `Narrate` on and off.
//! 2. The sentences are true to the board: at most 140 characters, and every piece they name stands where they say.
//! 3. Hanging pieces get named: the fixture marks one loose piece per position, and the sentence names its square.

use chess::Board;
use chess::board::{Piece, parse_square, square_name};
use chess::graph::{DEFAULT_GRAPH, Graph};
use chess::json::{self, Json};
use chess::narrate::{self, MAX_CHARS};
use chess::search::{Limits, Searcher};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

const DEPTH: u32 = 4;

struct Case {
    fen: String,
    loose: String,
}

fn cases() -> Vec<Case> {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/puzzles_sample.json");
    let text = std::fs::read_to_string(&path).unwrap();
    let parsed = json::parse(&text).unwrap();
    parsed
        .as_array()
        .unwrap()
        .iter()
        .map(|p| Case {
            fen: p.get("fen").and_then(Json::as_str).unwrap().to_string(),
            loose: p.get("loose").and_then(Json::as_str).unwrap().to_string(),
        })
        .collect()
}

fn fen_positions(cases: &[Case]) -> Vec<String> {
    cases.iter().map(|c| format!("fen {}", c.fen)).collect()
}

/// Every position in the sample games from which a move was played: `startpos moves ...` up to just before it.
/// 602 in all, twelve of them the start position.
fn game_positions() -> Vec<String> {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/games_sample.pgn");
    let text = std::fs::read_to_string(&path).unwrap();
    let mut out = Vec::new();
    for game in chess::pgn::read(&text) {
        let mut moves: Vec<String> = Vec::new();
        chess::pgn::replay(&game, |_, _, mv, _| {
            out.push(if moves.is_empty() {
                "startpos".to_string()
            } else {
                format!("startpos moves {}", moves.join(" "))
            });
            moves.push(mv.to_string());
        })
        .unwrap();
    }
    out
}

/// What the engine binary printed for each position: its move, and the sentence if it printed one.
fn through_uci(
    narrate_on: bool,
    positions: &[String],
    depth: u32,
) -> Vec<(String, Option<String>)> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_uci"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    if narrate_on {
        writeln!(stdin, "setoption name Narrate value true").unwrap();
    }
    let mut result = Vec::new();
    for position in positions {
        writeln!(stdin, "position {}", position).unwrap();
        writeln!(stdin, "go depth {}", depth).unwrap();
        stdin.flush().unwrap();
        let mut said = None;
        loop {
            let mut line = String::new();
            assert!(out.read_line(&mut line).unwrap() > 0, "engine closed early");
            if let Some(s) = line.trim().strip_prefix("info string say: ") {
                said = Some(s.to_string());
            }
            if let Some(m) = line.trim().strip_prefix("bestmove ") {
                result.push((m.to_string(), said));
                break;
            }
        }
    }
    writeln!(stdin, "quit").unwrap();
    drop(stdin);
    let _ = child.wait();
    result
}

#[test]
fn the_words_do_not_touch_the_move() {
    let cases = cases();
    let fens = fen_positions(&cases);
    let off = through_uci(false, &fens, DEPTH);
    let on = through_uci(true, &fens, DEPTH);
    assert!(
        off.iter().all(|(_, s)| s.is_none()),
        "words printed with Narrate off"
    );
    let same = off.iter().zip(&on).filter(|(a, b)| a.0 == b.0).count();
    eprintln!(
        "same move with Narrate on and off: {} of {}",
        same,
        cases.len()
    );
    assert_eq!(same, cases.len());
}

#[test]
fn the_sentences_are_true_to_the_board_and_name_the_hanging_piece() {
    let cases = cases();
    let on = through_uci(true, &fen_positions(&cases), DEPTH);
    let mut named = 0;
    let mut mates = 0;
    let mut missed: Vec<String> = Vec::new();
    for (case, (_, printed)) in cases.iter().zip(&on) {
        let mut before = Board::from_fen(&case.fen).unwrap();
        let mut searcher = Searcher::new(Graph::from_json(DEFAULT_GRAPH).unwrap());
        searcher.history = vec![before.hash()];
        let report = searcher.search(
            &mut before,
            &Limits {
                depth: Some(DEPTH),
                ..Default::default()
            },
            |_| {},
        );
        let mv = report.best().expect("a move");
        let said = narrate::say(&before, &report).expect("a sentence");
        let mut after = before.clone();
        after.make(mv);

        assert_eq!(Some(&said.text), printed.as_ref(), "{}", case.fen);
        assert!(
            said.text.chars().count() <= MAX_CHARS,
            "too long ({}): {}",
            said.text.chars().count(),
            said.text
        );
        assert_eq!(
            said.text.matches(" on ").count(),
            said.mentions.len(),
            "a piece is named that is not in the mention list: {}",
            said.text
        );
        for m in &said.mentions {
            let board = if m.after { &after } else { &before };
            assert_eq!(
                board.piece_on(m.square),
                Some((m.side, m.piece)),
                "{} names a {:?} on {} that is not there ({})",
                said.text,
                m.piece,
                square_name(m.square),
                case.fen
            );
            assert_ne!(m.piece, Piece::King, "{}", said.text);
        }
        let loose = parse_square(&case.loose).unwrap();
        let opp = before.turn.flip();
        if said
            .mentions
            .iter()
            .any(|m| m.side == opp && m.square == loose)
        {
            named += 1;
        } else if said.text.starts_with("I see a forced mate") {
            // B18: a mate outranks the loose piece, and the search now sees a mate on its last ply.
            mates += 1;
        } else {
            missed.push(format!("{} | {} | {}", case.loose, said.text, case.fen));
        }
    }
    let rate = (named + mates) as f32 / cases.len() as f32;
    eprintln!(
        "names the fixture's loose square: {} of {}, names a forced mate instead: {} ({:.1}% in all)",
        named,
        cases.len(),
        mates,
        rate * 100.0
    );
    for m in &missed {
        eprintln!("  missed: {}", m);
    }
    assert!(
        rate >= 0.90,
        "only {:.1}% name the loose piece or a mate",
        rate * 100.0
    );
}

/// B7. The sample games are real games, so the engine is given the move list and the last move gets a sentence too.
/// The sentences about the played move are checked against python-chess by `tools/narrate_check.py`.
#[test]
fn narrating_their_moves_does_not_touch_the_move_and_fits_the_chat() {
    let positions = game_positions();
    assert_eq!(positions.len(), 602);
    let off = through_uci(false, &positions, 3);
    let on = through_uci(true, &positions, 3);
    let same = off.iter().zip(&on).filter(|(a, b)| a.0 == b.0).count();
    eprintln!(
        "same move with their moves narrated: {} of {}",
        same,
        positions.len()
    );
    assert_eq!(same, positions.len());
    let mut both = 0;
    for ((_, said), position) in on.iter().zip(&positions) {
        let said = said.as_ref().expect("a sentence");
        let parts: Vec<&str> = said.split(" | ").collect();
        let expected = if position == "startpos" { 1 } else { 2 };
        assert_eq!(parts.len(), expected, "{} -> {}", position, said);
        both += usize::from(parts.len() == 2);
        for p in parts {
            assert!(p.chars().count() <= MAX_CHARS, "too long: {}", p);
        }
    }
    eprintln!("lines carrying both sentences: {}", both);
}
