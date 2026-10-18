//! Perft counts every leaf of the legal move tree to a depth. The counts for the standard positions are known
//! exactly, which makes them the gate for move generation: one wrong rule and a number is off.

use crate::board::{Board, Move};
use crate::movegen::legal_moves;

pub fn perft(b: &mut Board, depth: u32) -> u64 {
    if depth == 0 {
        return 1;
    }
    let moves = legal_moves(b);
    if depth == 1 {
        return moves.len() as u64;
    }
    let mut nodes = 0;
    for mv in moves {
        b.make(mv);
        nodes += perft(b, depth - 1);
        b.unmake();
    }
    nodes
}

/// The count under each root move, for finding which move's subtree is wrong.
pub fn divide(b: &mut Board, depth: u32) -> Vec<(Move, u64)> {
    let mut out = Vec::new();
    for mv in legal_moves(b) {
        b.make(mv);
        out.push((mv, perft(b, depth.saturating_sub(1))));
        b.unmake();
    }
    out
}
