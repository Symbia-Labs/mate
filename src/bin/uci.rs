//! The UCI front end: the engine as any chess GUI or the Lichess bot sees it.
//!
//! Options: `Graph` (a personality file, or `default`) and one string option per graph parameter.
//! `go` takes depth, nodes, movetime, wtime/btime/winc/binc and infinite.

use chess::board::{Board, Move};
use chess::graph::{DEFAULT_GRAPH, Graph};
use chess::movegen::legal_moves;
use chess::search::{Limits, Searcher};
use std::io::{BufRead, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

fn load(path: &str) -> Result<Graph, String> {
    if path == "default" {
        Graph::from_json(DEFAULT_GRAPH)
    } else {
        Graph::from_json(&std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path, e))?)
    }
}

fn uci_move(b: &mut Board, text: &str) -> Option<Move> {
    legal_moves(b).into_iter().find(|m| m.to_string() == text)
}

fn main() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    let mut graph_path = String::from("default");
    let mut settings: Vec<(String, f32)> = Vec::new();
    let mut board = Board::start();
    let mut history: Vec<u64> = vec![board.hash()];
    let stop = Arc::new(AtomicBool::new(false));
    let mut worker: Option<std::thread::JoinHandle<()>> = None;

    for line in stdin.lock().lines() {
        let line = line.unwrap_or_default();
        let mut words = line.split_whitespace();
        let Some(cmd) = words.next() else { continue };
        match cmd {
            "uci" => {
                println!("id name chess (graph)");
                println!("id author Symbia Labs");
                println!("option name Graph type string default default");
                if let Ok(g) = load(&graph_path) {
                    for p in g.params() {
                        println!("option name {} type string default {}", p.id, p.default);
                    }
                }
                println!("uciok");
            }
            "isready" => println!("readyok"),
            "setoption" => {
                let rest: Vec<&str> = words.collect();
                let name_at = rest.iter().position(|w| *w == "name");
                let value_at = rest.iter().position(|w| *w == "value");
                if let (Some(n), Some(v)) = (name_at, value_at) {
                    let name = rest[n + 1..v].join(" ");
                    let value = rest[v + 1..].join(" ");
                    if name == "Graph" {
                        match load(&value) {
                            Ok(_) => graph_path = value,
                            Err(e) => println!("info string {}", e),
                        }
                    } else if let Ok(x) = value.parse::<f32>() {
                        settings.retain(|(k, _)| *k != name);
                        settings.push((name, x));
                    }
                }
            }
            "ucinewgame" => {
                board = Board::start();
                history = vec![board.hash()];
            }
            "position" => {
                let rest: Vec<&str> = words.collect();
                let moves_at = rest.iter().position(|w| *w == "moves");
                let spec = &rest[..moves_at.unwrap_or(rest.len())];
                board = if spec.first() == Some(&"fen") {
                    Board::from_fen(&spec[1..].join(" ")).unwrap_or_else(|e| {
                        println!("info string bad fen: {}", e);
                        Board::start()
                    })
                } else {
                    Board::start()
                };
                history = vec![board.hash()];
                if let Some(i) = moves_at {
                    for m in &rest[i + 1..] {
                        match uci_move(&mut board, m) {
                            Some(mv) => {
                                board.make(mv);
                                history.push(board.hash());
                            }
                            None => {
                                println!("info string illegal move {}", m);
                                break;
                            }
                        }
                    }
                }
            }
            "go" => {
                if let Some(w) = worker.take() {
                    stop.store(true, Ordering::Relaxed);
                    let _ = w.join();
                }
                let rest: Vec<&str> = words.collect();
                let num = |key: &str| {
                    rest.iter()
                        .position(|w| *w == key)
                        .and_then(|i| rest.get(i + 1))
                        .and_then(|v| v.parse::<u64>().ok())
                };
                let limits = Limits {
                    depth: num("depth").map(|d| d as u32),
                    nodes: num("nodes"),
                    movetime: num("movetime").map(Duration::from_millis),
                    wtime: num("wtime").map(Duration::from_millis),
                    btime: num("btime").map(Duration::from_millis),
                    winc: num("winc").map(Duration::from_millis),
                    binc: num("binc").map(Duration::from_millis),
                    infinite: rest.contains(&"infinite"),
                };
                let mut graph = match load(&graph_path) {
                    Ok(g) => g,
                    Err(e) => {
                        println!("info string {}", e);
                        Graph::from_json(DEFAULT_GRAPH).unwrap()
                    }
                };
                for (k, v) in &settings {
                    if let Err(e) = graph.set(k, *v) {
                        println!("info string {}", e);
                    }
                }
                let mut searcher = Searcher::new(graph);
                searcher.stop = stop.clone();
                searcher.history = history.clone();
                searcher.root_ply = history.len() as u32 - 1;
                stop.store(false, Ordering::Relaxed);
                let mut b = board.clone();
                worker = Some(std::thread::spawn(move || {
                    let r = searcher.search(&mut b, &limits, |r| {
                        let ms = r.elapsed.as_millis().max(1) as u64;
                        println!(
                            "info depth {} score {} nodes {} nps {} time {} pv {}",
                            r.depth,
                            r.uci_score(),
                            r.nodes,
                            r.nodes * 1000 / ms,
                            ms,
                            r.pv.iter()
                                .map(|m| m.to_string())
                                .collect::<Vec<_>>()
                                .join(" ")
                        );
                    });
                    match r.best() {
                        Some(m) => println!("bestmove {}", m),
                        None => println!("bestmove 0000"),
                    }
                    let _ = std::io::stdout().flush();
                }));
            }
            "stop" => {
                stop.store(true, Ordering::Relaxed);
                if let Some(w) = worker.take() {
                    let _ = w.join();
                }
            }
            "quit" => {
                stop.store(true, Ordering::Relaxed);
                if let Some(w) = worker.take() {
                    let _ = w.join();
                }
                break;
            }
            "d" => println!("{}", board),
            _ => {}
        }
        let _ = out.flush();
    }
}
