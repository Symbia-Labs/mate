//! Gates for the block model. Each prints one line and names what failed and where.
//!
//! - `catalogue_is_described`: every block has a doc, every argument a doc and a default inside its range, every
//!   meter a name, and no two blocks share a name.
//! - `symmetric`: a position and its mirror score the same for the other side, block by block, over every ply of
//!   the sample games and a set of awkward positions. An evaluation that favours a colour is a bug.
//! - `loose_matches_the_detector`: the loose block finds the same pieces, at the same loss, as the Python
//!   detector that read Brian's archive, on every ply of the fixture games under `tests/fixtures/loose_*.json`.
//! - `repeatable`: two fresh graphs with a random modulator turn the same game into identical traces.
//! - `material_is_exact`: the material block counts known positions to the centipawn.

use chess::blocks::{self, loose::loose_pieces};
use chess::board::{Color, square_name};
use chess::graph::{DEFAULT_GRAPH, Graph};
use chess::json::{self, Json};
use chess::{Board, pgn};

const SAMPLE: &str = r#"[GameId "a"]

1. e4 Nc6 2. d4 e5 3. d5 Nce7 4. Nf3 Ng6 5. Bc4 Bc5 6. O-O d6 7. c3 Nf6 8. Bg5 h6 9. Bxf6 Qxf6 10. Nbd2 O-O 11. b4 Bb6 12. a4 a5 13. b5 Nf4 14. Kh1 Bg4 15. Qe1 Bxf3 16. Nxf3 Qg6 17. g3 Nh3 18. Qe2 Qg4 19. Rg1 Nxg1 20. Rxg1 Qxf3+ 0-1

[GameId "b"]

1. d4 d5 2. c4 dxc4 3. e4 e5 4. dxe5 Qxd1+ 5. Kxd1 Nc6 6. Bxc4 Be6 7. Bxe6 fxe6 8. Nc3 O-O-O+ 9. Ke2 Nxe5 10. f4 Nc6 11. Nf3 Nf6 12. e5 Nd5 13. Nxd5 Rxd5 14. Be3 Bb4 15. Rhd1 Rhd8 16. Rxd5 Rxd5 17. a3 Be7 18. b4 a6 19. Rc1 Kb8 20. g4 h6 1/2-1/2
"#;

const AWKWARD: &[&str] = &[
    "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
    "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
    "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8",
    "6k1/5ppp/8/8/8/8/5PPP/3R2K1 w - - 0 1",
    "3r2k1/5ppp/8/8/8/8/5PPP/6K1 b - - 0 1",
    "r1bqkb1r/pppp1ppp/2n2n2/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R w KQkq - 4 4",
    "8/8/8/3k4/8/8/4P3/4K3 w - - 0 1",
];

fn positions() -> Vec<Board> {
    let mut out: Vec<Board> = AWKWARD
        .iter()
        .map(|f| Board::from_fen(f).unwrap())
        .collect();
    for g in pgn::read(SAMPLE) {
        out.push(g.start().unwrap());
        pgn::replay(&g, |_, _, _, b| out.push(b.clone())).unwrap();
    }
    out
}

#[test]
fn catalogue_is_described() {
    let mut names = Vec::new();
    for b in blocks::CATALOGUE {
        assert!(!b.doc.is_empty(), "block {} has no doc", b.name);
        assert!(!names.contains(&b.name), "two blocks are named {}", b.name);
        names.push(b.name);
        assert!(!b.meters.is_empty(), "block {} reports no meters", b.name);
        for a in b.args {
            assert!(!a.doc.is_empty(), "{} {} has no doc", b.name, a.name);
            assert!(a.min < a.max, "{} {} has an empty range", b.name, a.name);
            assert_eq!(
                a.clean(a.default),
                a.default,
                "{} {} default is outside its range",
                b.name,
                a.name
            );
        }
    }
    println!("gate catalogue_is_described: {} blocks", names.len());
}

#[test]
fn symmetric() {
    let mut g = Graph::from_json(DEFAULT_GRAPH).unwrap();
    let mut n = 0;
    for b in positions() {
        let m = b.mirror();
        assert_eq!(
            m.mirror().fen(),
            b.fen(),
            "mirroring twice returns {}",
            b.fen()
        );
        let here = g.eval(&b, 20);
        let meters = g.meters();
        let there = g.eval(&m, 20);
        let mirrored = g.meters();
        assert!(
            (here.white() + there.white()).abs() < 1e-3,
            "{} scores {} but its mirror scores {}",
            b.fen(),
            here.white(),
            there.white()
        );
        for ((k, v), (_, w)) in meters.iter().zip(mirrored.iter()) {
            let other = if let Some(rest) = k.strip_suffix("white") {
                format!("{rest}black")
            } else if let Some(rest) = k.strip_suffix("black") {
                format!("{rest}white")
            } else if k.contains("white_") {
                k.replace("white_", "black_")
            } else if k.contains("black_") {
                k.replace("black_", "white_")
            } else {
                k.clone()
            };
            let swapped = mirrored
                .iter()
                .find(|(n, _)| *n == other)
                .map(|(_, x)| *x)
                .unwrap_or(*w);
            assert!(
                (v - swapped).abs() < 1e-3,
                "{}: meter {} is {} but the mirror's {} is {}",
                b.fen(),
                k,
                v,
                other,
                swapped
            );
        }
        n += 1;
    }
    println!("gate symmetric: {} positions", n);
}

#[test]
fn loose_matches_the_detector() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("loose_") && n.ends_with(".json"))
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    assert!(
        !files.is_empty(),
        "no loose_*.json fixtures under tests/fixtures; run tools/loose_fixture.py on a game"
    );
    let mut plies = 0;
    for path in files {
        let text = std::fs::read_to_string(&path).unwrap();
        let j = json::parse(&text).unwrap();
        let game = j
            .get("game")
            .and_then(Json::as_str)
            .unwrap_or("?")
            .to_string();
        for (i, ply) in j
            .get("plies")
            .and_then(Json::as_array)
            .unwrap()
            .iter()
            .enumerate()
        {
            let fen = ply.get("fen").and_then(Json::as_str).unwrap();
            let b = Board::from_fen(fen).unwrap();
            for (side, key) in [(Color::White, "white"), (Color::Black, "black")] {
                let mut got: Vec<(String, f32)> = loose_pieces(&b, side, 0.5, 0.5)
                    .into_iter()
                    .map(|(sq, loss)| (square_name(sq), loss))
                    .collect();
                got.sort_by(|a, b| a.0.cmp(&b.0));
                let mut want: Vec<(String, f32)> = ply
                    .get(key)
                    .and_then(Json::as_array)
                    .unwrap()
                    .iter()
                    .map(|e| {
                        let a = e.as_array().unwrap();
                        (
                            a[0].as_str().unwrap().to_string(),
                            a[1].as_f64().unwrap() as f32,
                        )
                    })
                    .collect();
                want.sort_by(|a, b| a.0.cmp(&b.0));
                assert_eq!(
                    got.len(),
                    want.len(),
                    "{} ply {} ({}): {} loose pieces are {:?}, the detector found {:?}",
                    game,
                    i,
                    fen,
                    key,
                    got,
                    want
                );
                for (g, w) in got.iter().zip(want.iter()) {
                    assert_eq!(
                        g.0, w.0,
                        "{} ply {} ({}): {} loose piece on {} vs {}",
                        game, i, fen, key, g.0, w.0
                    );
                    assert!(
                        (g.1 - w.1).abs() < 0.01,
                        "{} ply {} ({}): {} on {} is charged {} vs {}",
                        game,
                        i,
                        fen,
                        key,
                        g.0,
                        g.1,
                        w.1
                    );
                }
            }
            plies += 1;
        }
    }
    println!("gate loose_matches_the_detector: {} plies", plies);
}

#[test]
fn repeatable() {
    let text = r#"{"modulators":[{"id":"dice","shape":"random","rate":1,"seed":9}],
        "parallel":[{"node":{"block":"material"}},{"node":{"block":"loose"},"gain":"0.5 + 0.5 * dice"}]}"#;
    let trace = |g: &mut Graph| {
        let mut out = Vec::new();
        for game in pgn::read(SAMPLE) {
            pgn::replay(&game, |ply, _, _, b| {
                out.push(g.eval(b, ply).white());
                out.extend(g.meters().into_iter().map(|(_, v)| v));
            })
            .unwrap();
        }
        out
    };
    let a = trace(&mut Graph::from_json(text).unwrap());
    let b = trace(&mut Graph::from_json(text).unwrap());
    assert_eq!(a, b, "two fresh graphs disagree");
    assert!(a.iter().any(|v| *v != a[0]));
    println!("gate repeatable: {} values", a.len());
}

#[test]
fn material_is_exact() {
    let mut g = Graph::from_json(r#"{"block":"material"}"#).unwrap();
    for (fen, want) in [
        ("4k3/8/8/8/8/8/8/4K3 w - - 0 1", 0.0),
        ("4k3/8/8/8/8/8/8/R3K3 w - - 0 1", 500.0),
        ("4k3/pppppppp/8/8/8/8/8/4KQ2 w - - 0 1", 100.0),
        (
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBN1 w Qkq - 0 1",
            -500.0,
        ),
    ] {
        let got = g.eval(&Board::from_fen(fen).unwrap(), 0).white();
        assert_eq!(got, want, "{fen}");
    }
    println!("gate material_is_exact: 4 positions");
}
