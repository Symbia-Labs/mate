//! Gates for the search. Each prints one line and names what failed and where.
//!
//! - `mates_in_one`: every mate-in-one position, and its mirror, is found at depth 2 and scored as mate in 1.
//! - `mates_in_two`: a forced mate in two is found at depth 3 and scored as mate in 2.
//! - `wins_free_pieces`: on the puzzle positions cut from Brian's archive (`tests/fixtures/puzzles_sample.json`),
//!   the engine at depth 2 takes the loose piece, takes something worth at least as much, or finds a mate, at
//!   least 75% of the time. (77% on the first build; the rest is studied in `tests/puzzle_study.rs`: mostly
//!   puzzles where the mover's own queen is en prise, which the detector does not see.)
//! - `repeatable`: the same search twice gives the same line and the same node count.
//! - `speed`: at least 300,000 nodes per second from the start position at depth 5, in release.
//! - `uci_round_trip`: the `uci` binary answers `uciok`, `readyok`, finds a mate in one and stops cleanly.

use chess::Board;
use chess::graph::{DEFAULT_GRAPH, Graph};
use chess::json::{self, Json};
use chess::search::{Limits, MATE, MATE_BOUND, Report, Searcher};
use std::io::Write;
use std::process::{Command, Stdio};

const VAL: [i32; 6] = [100, 320, 330, 500, 900, 10000];

fn value_on(b: &Board, sq: &str) -> i32 {
    let s = chess::board::parse_square(sq).unwrap();
    b.piece_on(s).map_or(0, |(_, p)| VAL[p as usize])
}

fn run(fen: &str, depth: u32) -> Report {
    let mut s = Searcher::new(Graph::from_json(DEFAULT_GRAPH).unwrap());
    let mut b = Board::from_fen(fen).unwrap();
    s.search(
        &mut b,
        &Limits {
            depth: Some(depth),
            ..Default::default()
        },
        |_| {},
    )
}

const MATE_IN_ONE: &[(&str, &str)] = &[
    ("6k1/5ppp/8/8/8/8/8/R5K1 w - - 0 1", "a1a8"),
    (
        "r1bqkb1r/pppp1ppp/2n2n2/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR w KQkq - 4 4",
        "h5f7",
    ),
    ("7k/5Q2/6K1/8/8/8/8/8 w - - 0 1", "f7g7"),
    ("3r2k1/5ppp/8/8/8/8/5PPP/3R2K1 w - - 0 1", "d1d8"),
    (
        "rnbqkbnr/pppp1ppp/8/4p3/6P1/5P2/PPPPP2P/RNBQKBNR b KQkq g3 0 2",
        "d8h4",
    ),
];

#[test]
fn mates_in_one() {
    let mut n = 0;
    for (fen, want) in MATE_IN_ONE {
        let r = run(fen, 2);
        assert_eq!(
            r.best().map(|m| m.to_string()).as_deref(),
            Some(*want),
            "{fen}"
        );
        assert_eq!(r.score, MATE - 1, "{fen} is not scored as mate in 1");
        let m = Board::from_fen(fen).unwrap().mirror();
        let r = run(&m.fen(), 2);
        assert_eq!(
            r.score,
            MATE - 1,
            "the mirror of {fen} is not scored as mate in 1"
        );
        n += 2;
    }
    println!("gate mates_in_one: {} positions", n);
}

#[test]
fn mates_in_two() {
    // King and queen against a cornered king: 1.Kb6 Kb8 2.Qh8#, and nothing faster.
    let r = run("k7/8/2K5/8/8/8/8/7Q w - - 0 1", 3);
    assert_eq!(
        r.score,
        MATE - 3,
        "scored {} not mate in 2; line {:?}",
        r.score,
        r.pv.iter().map(|m| m.to_string()).collect::<Vec<_>>()
    );
    assert_eq!(r.pv.len(), 3, "the line should be three plies long");
    println!("gate mates_in_two: 1 position");
}

#[test]
fn wins_free_pieces() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/puzzles_sample.json");
    let text = std::fs::read_to_string(&path)
        .expect("tests/fixtures/puzzles_sample.json; cut it from the puzzle index");
    let puzzles = json::parse(&text).unwrap();
    let list = puzzles.as_array().unwrap();
    let mut s = Searcher::new(Graph::from_json(DEFAULT_GRAPH).unwrap());
    let (mut found, mut total) = (0, 0);
    let mut misses = Vec::new();
    for p in list {
        let fen = p.get("fen").and_then(Json::as_str).unwrap();
        let loose = p.get("loose").and_then(Json::as_str).unwrap();
        let mut b = Board::from_fen(fen).unwrap();
        let r = s.search(
            &mut b,
            &Limits {
                depth: Some(2),
                ..Default::default()
            },
            |_| {},
        );
        let Some(mv) = r.best() else { continue };
        let best = mv.to_string();
        let prize = value_on(&b, loose);
        let taken = mv.is_capture()
            && b.piece_on(mv.to)
                .is_some_and(|(_, p)| VAL[p as usize] >= prize);
        total += 1;
        if best.ends_with(loose) || r.score >= MATE_BOUND || taken {
            found += 1;
        } else if misses.len() < 5 {
            misses.push(format!(
                "{} ({}): played {} not x{}",
                p.get("id").and_then(Json::as_str).unwrap_or("?"),
                fen,
                best,
                loose
            ));
        }
    }
    let rate = found as f64 / total as f64;
    assert!(
        rate >= 0.75,
        "took the loose piece, as much, or a mate in {found} of {total} ({:.1}%); first misses: {:?}",
        rate * 100.0,
        misses
    );
    println!("gate wins_free_pieces: {found} of {total}");
}

#[test]
fn repeatable() {
    let fen = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
    let a = run(fen, 4);
    let b = run(fen, 4);
    assert_eq!(a.nodes, b.nodes, "node counts differ");
    assert_eq!(a.pv, b.pv, "lines differ");
    assert_eq!(a.score, b.score);
    println!("gate repeatable: {} nodes twice", a.nodes);
}

#[test]
fn speed() {
    let r = run(chess::board::START_FEN, 5);
    let nps = r.nodes as f64 / r.elapsed.as_secs_f64().max(1e-6);
    if cfg!(debug_assertions) {
        println!("gate speed: {nps:.0} nps (debug build, not judged)");
        return;
    }
    assert!(nps >= 300_000.0, "{nps:.0} nodes per second");
    println!("gate speed: {nps:.0} nps at depth {}", r.depth);
}

#[test]
fn uci_round_trip() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_uci"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("the uci binary starts");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"uci\nisready\nposition fen r1bqkb1r/pppp1ppp/2n2n2/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR w KQkq - 4 4\ngo depth 3\nquit\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("uciok"), "{text}");
    assert!(text.contains("readyok"), "{text}");
    assert!(text.contains("score mate 1"), "{text}");
    assert!(text.lines().any(|l| l.trim() == "bestmove h5f7"), "{text}");
    println!("gate uci_round_trip: ok");
}
