//! render <graph.json|default> <file.pgn> [--game <id>] [--set id=value]... [--json]
//!
//! Plays every game (or the one named) through the graph and prints one line per ply: the ply, the move, the
//! score from White's side, and every block's meters. `--json` prints one object per game instead, for a page.

use chess::graph::{DEFAULT_GRAPH, Graph};
use chess::pgn;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut plain = Vec::new();
    let mut game_id = None;
    let mut sets = Vec::new();
    let mut json = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--game" => {
                game_id = args.get(i + 1).cloned();
                i += 1;
            }
            "--set" => {
                sets.push(args.get(i + 1).cloned().unwrap_or_default());
                i += 1;
            }
            "--json" => json = true,
            a => plain.push(a.to_string()),
        }
        i += 1;
    }
    let (Some(graph_path), Some(pgn_path)) = (plain.first(), plain.get(1)) else {
        eprintln!(
            "usage: render <graph.json|default> <file.pgn> [--game <id>] [--set id=value]... [--json]"
        );
        std::process::exit(2);
    };
    let text = if graph_path == "default" {
        DEFAULT_GRAPH.to_string()
    } else {
        std::fs::read_to_string(graph_path)
            .unwrap_or_else(|e| fail(&format!("{}: {}", graph_path, e)))
    };
    let mut graph = Graph::from_json(&text).unwrap_or_else(|e| fail(&e));
    for s in &sets {
        let (id, v) = s
            .split_once('=')
            .unwrap_or_else(|| fail(&format!("--set takes id=value, not '{}'", s)));
        let v: f32 = v
            .parse()
            .unwrap_or_else(|_| fail(&format!("'{}' is not a number", v)));
        graph.set(id, v).unwrap_or_else(|e| fail(&e));
    }
    let pgn_text =
        std::fs::read_to_string(pgn_path).unwrap_or_else(|e| fail(&format!("{}: {}", pgn_path, e)));
    let games = pgn::read(&pgn_text);
    let selected: Vec<&pgn::Game> = games
        .iter()
        .filter(|g| {
            game_id.as_deref().is_none_or(|id| {
                g.tag("GameId") == Some(id) || g.tag("Site").is_some_and(|s| s.ends_with(id))
            })
        })
        .collect();
    if selected.is_empty() {
        fail("no game matched");
    }
    let mut first = true;
    if json {
        println!("[");
    }
    for g in selected {
        let id = g.tag("GameId").or(g.tag("Site")).unwrap_or("?").to_string();
        if json {
            if !first {
                println!(",");
            }
            first = false;
            print!(
                "{{\"game\":\"{}\",\"white\":\"{}\",\"black\":\"{}\",\"result\":\"{}\",\"meters\":[",
                id,
                g.tag("White").unwrap_or(""),
                g.tag("Black").unwrap_or(""),
                g.tag("Result").unwrap_or("")
            );
        } else {
            println!(
                "# {} {} vs {} {}",
                id,
                g.tag("White").unwrap_or("?"),
                g.tag("Black").unwrap_or("?"),
                g.tag("Result").unwrap_or("")
            );
        }
        let mut rows: Vec<String> = Vec::new();
        let start = g.start().unwrap_or_else(|e| fail(&e));
        let f0 = graph.eval(&start, 0);
        let m0 = graph.meters();
        let names: Vec<String> = m0.iter().map(|(k, _)| k.clone()).collect();
        rows.push(row(json, 0, "", f0.white(), &m0));
        let r = pgn::replay(g, |ply, san, _mv, board| {
            let f = graph.eval(board, ply);
            rows.push(row(json, ply, san, f.white(), &graph.meters()));
        });
        if let Err(e) = r {
            eprintln!("{}: {}", id, e);
        }
        if json {
            println!(
                "{}],\"names\":[{}]}}",
                rows.join(","),
                names
                    .iter()
                    .map(|n| format!("\"{}\"", n))
                    .collect::<Vec<_>>()
                    .join(",")
            );
        } else {
            println!("ply\tmove\tscore\t{}", names.join("\t"));
            for r in rows {
                println!("{}", r);
            }
        }
    }
    if json {
        println!("]");
    }
}

fn row(json: bool, ply: u32, san: &str, score: f32, meters: &[(String, f32)]) -> String {
    if json {
        format!(
            "{{\"ply\":{},\"san\":\"{}\",\"score\":{:.1},\"m\":[{}]}}",
            ply,
            san,
            score,
            meters
                .iter()
                .map(|(_, v)| format!("{:.1}", v + 0.0))
                .collect::<Vec<_>>()
                .join(",")
        )
    } else {
        format!(
            "{}\t{}\t{:.0}\t{}",
            ply,
            san,
            score,
            meters
                .iter()
                .map(|(_, v)| format!("{:.0}", v + 0.0))
                .collect::<Vec<_>>()
                .join("\t")
        )
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("{}", msg);
    std::process::exit(2);
}
