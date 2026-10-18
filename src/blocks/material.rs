//! Material: each side's pieces at the values given.

use super::SIDES;
use crate::block::{ArgInfo, Block, BlockInfo, Frame};
use crate::board::{Board, Piece};
use crate::expr::Ctx;

pub const INFO: BlockInfo = BlockInfo {
    name: "material",
    doc: "Counts each side's pieces at the values given. The classic scale is 100, 320, 330, 500, 900.",
    args: &[
        ArgInfo::num("pawn", "cp", 0.0, 300.0, 100.0, "A pawn."),
        ArgInfo::num("knight", "cp", 0.0, 1000.0, 320.0, "A knight."),
        ArgInfo::num("bishop", "cp", 0.0, 1000.0, 330.0, "A bishop."),
        ArgInfo::num("rook", "cp", 0.0, 1500.0, 500.0, "A rook."),
        ArgInfo::num("queen", "cp", 0.0, 3000.0, 900.0, "A queen."),
    ],
    meters: &["white", "black"],
    make: || Box::new(Material::default()),
};

#[derive(Default)]
pub struct Material {
    values: [f32; 5],
    meters: [f32; 2],
}

impl Block for Material {
    fn control(&mut self, args: &[f32]) {
        self.values.copy_from_slice(&args[..5]);
    }

    fn process(&mut self, board: &Board, _ctx: &Ctx, io: &mut Frame) {
        for (i, side) in SIDES.iter().enumerate() {
            let mut total = 0.0;
            for (p, v) in [
                Piece::Pawn,
                Piece::Knight,
                Piece::Bishop,
                Piece::Rook,
                Piece::Queen,
            ]
            .iter()
            .zip(self.values)
            {
                total += board.pieces(*side, *p).count_ones() as f32 * v;
            }
            self.meters[i] = total;
            io.ch[i] += total;
        }
    }

    fn meter(&self, index: usize) -> f32 {
        self.meters[index]
    }
}
