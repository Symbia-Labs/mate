//! Pawn structure: doubled and isolated pawns cost, passed pawns earn more the further they are.

use super::{SIDES, Squares};
use crate::block::{ArgInfo, Block, BlockInfo, Frame};
use crate::board::{Board, Color, Piece};
use crate::expr::Ctx;
use crate::tables::{Bitboard, FILE_A};

pub const INFO: BlockInfo = BlockInfo {
    name: "pawns",
    doc: "Charges doubled and isolated pawns and rewards passed pawns, more for each rank they have advanced.",
    args: &[
        ArgInfo::num(
            "doubled",
            "cp",
            0.0,
            60.0,
            10.0,
            "Per pawn behind another of its own on the same file.",
        ),
        ArgInfo::num(
            "isolated",
            "cp",
            0.0,
            60.0,
            15.0,
            "Per pawn with no own pawn on either neighbouring file.",
        ),
        ArgInfo::num(
            "passed",
            "cp",
            0.0,
            100.0,
            20.0,
            "Per rank advanced, for a pawn no enemy pawn can stop.",
        ),
    ],
    meters: &["white", "black"],
    make: || Box::new(Pawns::default()),
};

#[derive(Default)]
pub struct Pawns {
    doubled: f32,
    isolated: f32,
    passed: f32,
    meters: [f32; 2],
}

fn file_bb(f: i32) -> Bitboard {
    if (0..8).contains(&f) { FILE_A << f } else { 0 }
}

/// Squares ahead of sq on its file and the two beside it, from `side`'s point of view.
fn front_span(sq: u8, side: Color) -> Bitboard {
    let f = (sq % 8) as i32;
    let r = (sq / 8) as i32;
    let files = file_bb(f - 1) | file_bb(f) | file_bb(f + 1);
    let ahead = if side == Color::White {
        !0u64 << (8 * (r + 1).min(8))
    } else {
        (1u64 << (8 * r)) - 1
    };
    files & ahead
}

impl Block for Pawns {
    fn control(&mut self, args: &[f32]) {
        self.doubled = args[0];
        self.isolated = args[1];
        self.passed = args[2];
    }

    fn process(&mut self, board: &Board, _ctx: &Ctx, io: &mut Frame) {
        for (i, side) in SIDES.iter().enumerate() {
            let own = board.pieces(*side, Piece::Pawn);
            let enemy = board.pieces(side.flip(), Piece::Pawn);
            let mut cp = 0.0;
            for sq in Squares(own) {
                let f = (sq % 8) as i32;
                let rank = (sq / 8) as i32;
                let on_file = own & file_bb(f);
                let ahead_own = on_file & front_span(sq, *side) & file_bb(f);
                if ahead_own != 0 {
                    cp -= self.doubled;
                }
                if own & (file_bb(f - 1) | file_bb(f + 1)) == 0 {
                    cp -= self.isolated;
                }
                if enemy & front_span(sq, *side) == 0 {
                    let advanced = if *side == Color::White {
                        rank - 1
                    } else {
                        6 - rank
                    };
                    cp += self.passed * advanced.max(0) as f32;
                }
            }
            self.meters[i] = cp;
            io.ch[i] += cp;
        }
    }

    fn meter(&self, index: usize) -> f32 {
        self.meters[index]
    }
}
