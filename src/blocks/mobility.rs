//! Mobility: how many squares each side's pieces reach that its own pieces do not occupy.

use super::{SIDES, Squares};
use crate::block::{ArgInfo, Block, BlockInfo, Frame};
use crate::board::{Board, Piece};
use crate::expr::Ctx;
use crate::tables::{KNIGHT, bishop_attacks, queen_attacks, rook_attacks};

pub const INFO: BlockInfo = BlockInfo {
    name: "mobility",
    doc: "Rewards each side for the squares its knights, bishops, rooks and queens reach, not counting squares its own pieces stand on.",
    args: &[
        ArgInfo::num(
            "knight",
            "cp",
            0.0,
            20.0,
            4.0,
            "Per square a knight reaches.",
        ),
        ArgInfo::num(
            "bishop",
            "cp",
            0.0,
            20.0,
            5.0,
            "Per square a bishop reaches.",
        ),
        ArgInfo::num("rook", "cp", 0.0, 20.0, 2.0, "Per square a rook reaches."),
        ArgInfo::num("queen", "cp", 0.0, 20.0, 1.0, "Per square a queen reaches."),
    ],
    meters: &["white", "black"],
    make: || Box::new(Mobility::default()),
};

#[derive(Default)]
pub struct Mobility {
    w: [f32; 4],
    meters: [f32; 2],
}

impl Block for Mobility {
    fn control(&mut self, args: &[f32]) {
        self.w.copy_from_slice(&args[..4]);
    }

    fn process(&mut self, board: &Board, _ctx: &Ctx, io: &mut Frame) {
        let occ = board.all();
        for (i, side) in SIDES.iter().enumerate() {
            let own = board.occ[*side as usize];
            let mut cp = 0.0;
            for sq in Squares(board.pieces(*side, Piece::Knight)) {
                cp += (KNIGHT[sq as usize] & !own).count_ones() as f32 * self.w[0];
            }
            for sq in Squares(board.pieces(*side, Piece::Bishop)) {
                cp += (bishop_attacks(sq as usize, occ) & !own).count_ones() as f32 * self.w[1];
            }
            for sq in Squares(board.pieces(*side, Piece::Rook)) {
                cp += (rook_attacks(sq as usize, occ) & !own).count_ones() as f32 * self.w[2];
            }
            for sq in Squares(board.pieces(*side, Piece::Queen)) {
                cp += (queen_attacks(sq as usize, occ) & !own).count_ones() as f32 * self.w[3];
            }
            self.meters[i] = cp;
            io.ch[i] += cp;
        }
    }

    fn meter(&self, index: usize) -> f32 {
        self.meters[index]
    }
}
