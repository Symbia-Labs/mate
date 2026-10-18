//! Not a gate: a study of how the engine treats the loose-piece puzzles, by depth. Ignored by default.
//!
//! `cargo test --release --test puzzle_study -- --ignored --nocapture`
//!
//! For each depth it prints how often the engine's move takes the loose piece, takes something worth at least
//! as much, mates, or does something else, with the first few of the something-elses.

use chess::Board;
use chess::board::Piece;
use chess::graph::{DEFAULT_GRAPH, Graph};
use chess::json::{self, Json};
use chess::search::{Limits, MATE_BOUND, Searcher};

const VAL: [i32; 6] = [100, 320, 330, 500, 900, 10000];

fn value_on(b: &Board, sq: &str) -> i32 {
    let s = chess::board::parse_square(sq).unwrap();
    b.piece_on(s).map_or(0, |(_, p)| VAL[p as usize])
}

#[test]
#[ignore]
fn rate_by_depth() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/puzzles_sample.json");
    let text = std::fs::read_to_string(&path).unwrap();
    let puzzles = json::parse(&text).unwrap();
    let list = puzzles.as_array().unwrap();
    for depth in 1..=4u32 {
        let mut s = Searcher::new(Graph::from_json(DEFAULT_GRAPH).unwrap());
        let (mut took, mut bigger, mut mate, mut other) = (0, 0, 0, 0);
        let mut examples = Vec::new();
        let start = std::time::Instant::now();
        for p in list {
            let fen = p.get("fen").and_then(Json::as_str).unwrap();
            let loose = p.get("loose").and_then(Json::as_str).unwrap();
            let mut b = Board::from_fen(fen).unwrap();
            let r = s.search(
                &mut b,
                &Limits {
                    depth: Some(depth),
                    ..Default::default()
                },
                |_| {},
            );
            let Some(m) = r.best() else { continue };
            let best = m.to_string();
            let prize = value_on(&b, loose);
            if best.ends_with(loose) {
                took += 1;
            } else if r.score >= MATE_BOUND {
                mate += 1;
            } else if m.is_capture()
                && b.piece_on(m.to)
                    .is_some_and(|(_, p)| VAL[p as usize] >= prize || p == Piece::Queen)
            {
                bigger += 1;
            } else {
                other += 1;
                if examples.len() < 6 {
                    examples.push(format!(
                        "{} {} not x{} ({})",
                        p.get("id").and_then(Json::as_str).unwrap_or("?"),
                        best,
                        loose,
                        fen
                    ));
                }
            }
        }
        let n = list.len();
        println!(
            "depth {depth}: took {took} ({:.1}%), bigger capture {bigger}, mate {mate}, other {other} of {n}; {:.2}s",
            took as f64 * 100.0 / n as f64,
            start.elapsed().as_secs_f64()
        );
        for e in examples {
            println!("  {e}");
        }
    }
}
