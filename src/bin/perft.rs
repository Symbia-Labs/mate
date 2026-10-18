//! perft <depth> [fen]            total nodes and the time taken
//! perft <depth> [fen] --divide   the count under each root move

use chess::Board;
use chess::board::START_FEN;
use chess::perft::{divide, perft};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let depth: u32 = args
        .first()
        .and_then(|a| a.parse().ok())
        .unwrap_or_else(|| {
            eprintln!("usage: perft <depth> [fen] [--divide]");
            std::process::exit(2);
        });
    let div = args.iter().any(|a| a == "--divide");
    let fen = args
        .iter()
        .skip(1)
        .find(|a| *a != "--divide")
        .map(String::as_str)
        .unwrap_or(START_FEN);
    let mut b = match Board::from_fen(fen) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("bad FEN: {e}");
            std::process::exit(2);
        }
    };
    let start = Instant::now();
    if div {
        let mut total = 0;
        for (mv, n) in divide(&mut b, depth) {
            println!("{mv} {n}");
            total += n;
        }
        println!("total {total}");
    } else {
        let n = perft(&mut b, depth);
        let secs = start.elapsed().as_secs_f64();
        println!(
            "{n} nodes in {secs:.3} s ({:.0} nps)",
            n as f64 / secs.max(1e-9)
        );
    }
}
