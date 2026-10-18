//! The evaluation blocks. Each is a term in a side's score, declared in one static table, and each reports
//! meters so a render can show which term moved on the ply a game turned.

pub mod king;
pub mod loose;
pub mod material;
pub mod mobility;
pub mod pawns;

use crate::block::BlockInfo;
use crate::board::{Color, Piece};
use crate::tables::Bitboard;

/// Every block, for the graph to look up by name and for the format guide.
pub const CATALOGUE: &[&BlockInfo] = &[
    &material::INFO,
    &loose::INFO,
    &king::INFO,
    &mobility::INFO,
    &pawns::INFO,
];

pub fn find(name: &str) -> Option<&'static BlockInfo> {
    CATALOGUE.iter().copied().find(|b| b.name == name)
}

pub const SIDES: [Color; 2] = [Color::White, Color::Black];

/// Piece values the threat blocks use to size a loss, in centipawns. The material block has its own arguments.
pub const VALUE: [f32; 6] = [100.0, 320.0, 330.0, 500.0, 900.0, 10000.0];

#[inline]
pub fn value(p: Piece) -> f32 {
    VALUE[p as usize]
}

/// Iterates the squares of a bitboard.
pub struct Squares(pub Bitboard);

impl Iterator for Squares {
    type Item = u8;

    #[inline]
    fn next(&mut self) -> Option<u8> {
        if self.0 == 0 {
            return None;
        }
        let sq = self.0.trailing_zeros() as u8;
        self.0 &= self.0 - 1;
        Some(sq)
    }
}
