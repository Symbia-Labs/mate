//! The UCI front end: the engine as any chess GUI or the Lichess bot sees it.
//!
//! Options: `Graph` (a personality file, or `default`), `Narrate` (before `bestmove`, print one line
//! `info string say: <the last move in the position> | <the move chosen>`; off by default) and one string option
//! per graph parameter.
//! Sparring (B15): `Sparring` (off by default), `Opponent` (the opponent's rating) and `SparSeed`. With `Sparring` on,
//! when she stands more than a pawn up the move may be a slightly worse one that leaves her closer to level (see
//! `chess::spar`). With it off the engine answers exactly as before.
//! `go` takes depth, nodes, movetime, wtime/btime/winc/binc and infinite.
//!
//! `uci coach` runs the coach instead of the engine: the bot's three commentary lanes (see `chess::coach`). It takes
//! `depth=N` and `nodes=N` for the judge's searches, and `voice=every` for a line on every move in every lane (the
//! B15 voice). The default is `voice=quiet` (B16): a lane speaks when something happened and says why, and writes
//! `skip <ply> <lane>` when it has nothing to say.

use chess::board::{Board, Color, Move};
use chess::coach::Mode;
use chess::graph::{DEFAULT_GRAPH, Graph};
use chess::judge::Settings;
use chess::movegen::legal_moves;
use chess::search::{Limits, Report, Searcher};
use std::io::{BufRead, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn load(path: &str) -> Result<Graph, String> {
    if path == "default" {
        Graph::from_json(DEFAULT_GRAPH)
    } else {
        Graph::from_json(&std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path, e))?)
    }
}

/// The `info` line for a finished depth.
fn info_line(r: &Report) -> String {
    let ms = r.elapsed.as_millis().max(1) as u64;
    format!(
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
    )
}

fn uci_move(b: &mut Board, text: &str) -> Option<Move> {
    legal_moves(b).into_iter().find(|m| m.to_string() == text)
}

/// The judge's limits from `depth=6 nodes=500000`, for measuring, and the voice from `voice=quiet` or
/// `voice=every`; all are optional, and the voice is quiet unless it says otherwise.
fn coach_settings(args: &[String]) -> Result<(Settings, Mode), String> {
    let mut settings = Settings::default();
    let mut mode = Mode::Quiet;
    for arg in args {
        match arg.split_once('=') {
            Some(("voice", v)) => {
                mode = Mode::parse(v)
                    .ok_or_else(|| format!("not a voice: {} (use every or quiet)", v))?
            }
            Some(("depth", v)) => {
                settings.depth = v.parse().map_err(|_| format!("not a depth: {}", v))?
            }
            Some(("nodes", v)) => {
                settings.nodes = v.parse().map_err(|_| format!("not a node count: {}", v))?
            }
            _ => {
                return Err(format!(
                    "unknown argument: {} (use depth=N, nodes=N or voice=every|quiet)",
                    arg
                ));
            }
        }
    }
    Ok((settings, mode))
}

fn main() {
    // `uci coach` is the commentary process: events in, chat lines out. See `chess::coach`.
    if std::env::args().nth(1).as_deref() == Some("coach") {
        let args: Vec<String> = std::env::args().skip(2).collect();
        match coach_settings(&args) {
            Ok((settings, mode)) => {
                chess::coach::run_voiced(
                    std::io::stdin().lock(),
                    std::io::stdout(),
                    settings,
                    mode,
                );
            }
            Err(why) => {
                eprintln!("uci coach: {}", why);
                std::process::exit(2);
            }
        }
        return;
    }
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    let mut graph_path = String::from("default");
    let mut settings: Vec<(String, f32)> = Vec::new();
    let mut narrate = false;
    // Sparring (B15). `opponent` is the opponent's rating and `spar_seed` varies the choices from game to game.
    let mut sparring = false;
    let mut opponent: i32 = 0;
    let mut spar_seed: u64 = 0;
    // The board before the last move in the `position` command, and that move, for `Narrate`.
    let mut last: Option<(Board, Move)> = None;
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
                println!("option name Narrate type check default false");
                println!("option name Sparring type check default false");
                println!("option name Opponent type spin default 0 min 0 max 4000");
                println!("option name SparSeed type string default 0");
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
                    } else if name == "Narrate" {
                        narrate = value.eq_ignore_ascii_case("true");
                    } else if name == "Sparring" {
                        sparring = value.eq_ignore_ascii_case("true");
                    } else if name == "Opponent" {
                        opponent = value.parse().unwrap_or(0);
                    } else if name == "SparSeed" {
                        spar_seed = value.parse().unwrap_or(0);
                    } else if let Ok(x) = value.parse::<f32>() {
                        settings.retain(|(k, _)| *k != name);
                        settings.push((name, x));
                    }
                }
            }
            "ucinewgame" => {
                last = None;
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
                last = None;
                if let Some(i) = moves_at {
                    let listed = &rest[i + 1..];
                    for (k, m) in listed.iter().enumerate() {
                        match uci_move(&mut board, m) {
                            Some(mv) => {
                                if k + 1 == listed.len() {
                                    last = Some((board.clone(), mv));
                                }
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
                // Sparring is off with less than twenty seconds on the clock, and a search with no clock is not timed.
                let my_clock = if board.turn == Color::White {
                    limits.wtime
                } else {
                    limits.btime
                };
                let spar = if sparring
                    && my_clock.is_none_or(|t| t.as_millis() as u64 >= chess::spar::MIN_CLOCK_MS)
                {
                    chess::spar::Settings::for_rating(opponent, spar_seed)
                } else {
                    None
                };
                let halted = stop.clone();
                let mut b = board.clone();
                let root = board.clone();
                let theirs = last.clone();
                worker = Some(std::thread::spawn(move || {
                    let mut r = searcher.search(&mut b, &limits, |r| println!("{}", info_line(r)));
                    // Sparring (B15): when she is more than a pawn up she may play a slightly weaker move instead,
                    // and the last `info` line then describes that move.
                    let started = Instant::now();
                    if let Some(s) = &spar
                        && !halted.load(Ordering::Relaxed)
                        && let Some(c) = chess::spar::choose(&mut searcher, &root, &r, s)
                    {
                        // The choosing is part of the move's cost, so the line counts its nodes and its time.
                        r = Report {
                            score: c.score,
                            pv: c.pv,
                            nodes: r.nodes + c.nodes,
                            elapsed: r.elapsed + started.elapsed(),
                            ..r
                        };
                        println!("{}", info_line(&r));
                    }
                    if narrate {
                        // Their move first, then mine, joined by a bar; the bot splits them again.
                        let theirs = theirs.map(|(b, m)| chess::narrate::say_theirs(&b, m).text);
                        let mine = chess::narrate::say(&root, &r).map(|s| s.text);
                        let line = match (theirs, mine) {
                            (Some(t), Some(s)) => Some(format!("{} | {}", t, s)),
                            (Some(only), None) | (None, Some(only)) => Some(only),
                            (None, None) => None,
                        };
                        if let Some(line) = line {
                            println!("info string say: {}", line);
                        }
                    }
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
