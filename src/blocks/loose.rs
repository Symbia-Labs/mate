//! Loose pieces: a knight, bishop, rook or queen that is attacked and either undefended or attacked by a cheaper
//! piece. The term is the likely loss. A piece of the side to move is charged less, since that side can still
//! save it.

use super::{SIDES, Squares, value};
use crate::block::{ArgInfo, Block, BlockInfo, Frame};
use crate::board::{Board, Color, Piece};
use crate::expr::Ctx;

pub const INFO: BlockInfo = BlockInfo {
    name: "loose",
    doc: "Charges each side for pieces that can be won: attacked and undefended, or attacked by something cheaper. The side to move is charged a fraction, since it can still save the piece.",
    args: &[
        ArgInfo::num(
            "undefended",
            "",
            0.0,
            1.0,
            0.5,
            "Fraction of an undefended piece's value charged.",
        ),
        ArgInfo::num(
            "cheaper",
            "",
            0.0,
            1.0,
            0.5,
            "Fraction of the difference charged when a cheaper piece attacks a defended one.",
        ),
        ArgInfo::num(
            "own_turn",
            "",
            0.0,
            1.0,
            0.25,
            "Scale for the side to move, which can still save its piece.",
        ),
    ],
    meters: &["white_loose", "black_loose", "white_cp", "black_cp"],
    make: || Box::new(Loose::default()),
};

#[derive(Default)]
pub struct Loose {
    undefended: f32,
    cheaper: f32,
    own_turn: f32,
    meters: [f32; 4],
}

/// The pieces of `side` that can be won, each with the loss it is charged at full weight.
pub fn loose_pieces(board: &Board, side: Color, undefended: f32, cheaper: f32) -> Vec<(u8, f32)> {
    let them = side.flip();
    let mut out = Vec::new();
    for p in [Piece::Knight, Piece::Bishop, Piece::Rook, Piece::Queen] {
        for sq in Squares(board.pieces(side, p)) {
            let attackers = board.attackers(sq, them);
            if attackers == 0 {
                continue;
            }
            let v = value(p);
            if board.attackers(sq, side) == 0 {
                out.push((sq, v * undefended));
                continue;
            }
            let cheapest = Squares(attackers)
                .map(|a| value(board.piece_on(a).map(|(_, p)| p).unwrap_or(Piece::King)))
                .fold(f32::INFINITY, f32::min);
            if cheapest < v {
                out.push((sq, (v - cheapest) * cheaper));
            }
        }
    }
    out
}

impl Block for Loose {
    fn control(&mut self, args: &[f32]) {
        self.undefended = args[0];
        self.cheaper = args[1];
        self.own_turn = args[2];
    }

    fn process(&mut self, board: &Board, _ctx: &Ctx, io: &mut Frame) {
        for (i, side) in SIDES.iter().enumerate() {
            let pieces = loose_pieces(board, *side, self.undefended, self.cheaper);
            let scale = if board.turn == *side {
                self.own_turn
            } else {
                1.0
            };
            let cp: f32 = pieces.iter().map(|(_, loss)| loss * scale).sum();
            self.meters[i] = pieces.len() as f32;
            self.meters[2 + i] = cp;
            io.ch[i] -= cp;
        }
    }

    fn meter(&self, index: usize) -> f32 {
        self.meters[index]
    }
}
