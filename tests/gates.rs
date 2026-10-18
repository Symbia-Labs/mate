//! Gates for move generation. Each is a perft count that is known exactly for a standard position, so a
//! failure names the position and the depth. A move generator that passes all six is right about pins,
//! en passant, promotion, castling and check.

use chess::Board;
use chess::perft::perft;

struct Gate {
    name: &'static str,
    fen: &'static str,
    counts: &'static [u64],
}

const GATES: &[Gate] = &[
    Gate {
        name: "start",
        fen: "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
        counts: &[20, 400, 8902, 197281, 4865609],
    },
    Gate {
        name: "kiwipete",
        fen: "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
        counts: &[48, 2039, 97862, 4085603],
    },
    Gate {
        name: "position 3",
        fen: "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
        counts: &[14, 191, 2812, 43238, 674624],
    },
    Gate {
        name: "position 4",
        fen: "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1",
        counts: &[6, 264, 9467, 422333],
    },
    Gate {
        name: "position 5",
        fen: "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8",
        counts: &[44, 1486, 62379, 2103487],
    },
    Gate {
        name: "position 6",
        fen: "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10",
        counts: &[46, 2079, 89890, 3894594],
    },
];

fn run(gate: &Gate) {
    let mut b = Board::from_fen(gate.fen).unwrap();
    for (i, &want) in gate.counts.iter().enumerate() {
        let depth = i as u32 + 1;
        let got = perft(&mut b, depth);
        assert_eq!(
            got, want,
            "perft({depth}) of {} is {got}, not {want}",
            gate.name
        );
        assert_eq!(
            b.fen(),
            gate.fen,
            "{}: the board changed after perft({depth})",
            gate.name
        );
    }
    println!("gate {}: {} depths match", gate.name, gate.counts.len());
}

#[test]
fn start_position() {
    run(&GATES[0]);
}

#[test]
fn kiwipete() {
    run(&GATES[1]);
}

#[test]
fn position_3() {
    run(&GATES[2]);
}

#[test]
fn position_4() {
    run(&GATES[3]);
}

#[test]
fn position_5() {
    run(&GATES[4]);
}

#[test]
fn position_6() {
    run(&GATES[5]);
}

/// Deeper counts, run with `cargo test --release -- --ignored`.
#[test]
#[ignore]
fn deep() {
    let deep: &[(&str, u32, u64)] = &[
        (
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
            6,
            119060324,
        ),
        (
            "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
            5,
            193690690,
        ),
        ("8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1", 6, 11030083),
        (
            "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1",
            5,
            15833292,
        ),
        (
            "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8",
            5,
            89941194,
        ),
        (
            "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10",
            5,
            164075551,
        ),
    ];
    for (fen, depth, want) in deep {
        let got = perft(&mut Board::from_fen(fen).unwrap(), *depth);
        assert_eq!(got, *want, "perft({depth}) of {fen}");
    }
}
