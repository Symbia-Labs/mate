//! King exposure: missing shield pawns, open files beside the king, enemy pieces bearing on the king's zone,
//! and a king shut in on its back rank with a rook or queen still on the board.

use super::{SIDES, Squares};
use crate::block::{ArgInfo, Block, BlockInfo, Frame};
use crate::board::{Board, Color, Piece};
use crate::expr::Ctx;
use crate::tables::{FILE_A, KING, KNIGHT, bishop_attacks, queen_attacks, rook_attacks};

pub const INFO: BlockInfo = BlockInfo {
    name: "king",
    doc: "Charges each side for an exposed king: pawns missing from the shield in front of it, open files beside it, enemy pieces bearing on the squares around it, and a king shut in on its back rank while the other side still has a rook or queen.",
    args: &[
        ArgInfo::num(
            "shield",
            "cp",
            0.0,
            60.0,
            15.0,
            "Per pawn missing from the three squares in front of the king.",
        ),
        ArgInfo::num(
            "open",
            "cp",
            0.0,
            60.0,
            10.0,
            "Per file beside or under the king with no own pawn.",
        ),
        ArgInfo::num(
            "attack",
            "cp",
            0.0,
            60.0,
            10.0,
            "Per enemy piece attacking a square next to the king.",
        ),
        ArgInfo::num(
            "luft",
            "cp",
            0.0,
            150.0,
            30.0,
            "A king on its back rank with no empty square in front of it, while the other side has a rook or queen.",
        ),
    ],
    meters: &["white_exposure", "black_exposure"],
    make: || Box::new(King::default()),
};

#[derive(Default)]
pub struct King {
    shield: f32,
    open: f32,
    attack: f32,
    luft: f32,
    meters: [f32; 2],
}

fn exposure(board: &Board, side: Color, k: &King) -> f32 {
    let them = side.flip();
    let ksq = board.king(side);
    let file = (ksq % 8) as i32;
    let rank = (ksq / 8) as i32;
    let own_pawns = board.pieces(side, Piece::Pawn);
    let forward = if side == Color::White { 1 } else { -1 };
    let mut cp = 0.0;

    // Shield: the three squares one rank ahead, when the king is on its first two ranks.
    let home_side = if side == Color::White {
        rank <= 1
    } else {
        rank >= 6
    };
    if home_side {
        let r = rank + forward;
        if (0..8).contains(&r) {
            for f in (file - 1).max(0)..=(file + 1).min(7) {
                if own_pawns & (1u64 << (r * 8 + f)) == 0 {
                    cp += k.shield;
                }
            }
        }
    }

    // Open files beside and under the king.
    for f in (file - 1).max(0)..=(file + 1).min(7) {
        if own_pawns & (FILE_A << f) == 0 {
            cp += k.open;
        }
    }

    // Enemy pieces bearing on the zone around the king.
    let zone = KING[ksq as usize] | (1u64 << ksq);
    let occ = board.all();
    let mut attackers = 0;
    for sq in Squares(board.pieces(them, Piece::Knight)) {
        attackers += (KNIGHT[sq as usize] & zone != 0) as u32;
    }
    for sq in Squares(board.pieces(them, Piece::Bishop)) {
        attackers += (bishop_attacks(sq as usize, occ) & zone != 0) as u32;
    }
    for sq in Squares(board.pieces(them, Piece::Rook)) {
        attackers += (rook_attacks(sq as usize, occ) & zone != 0) as u32;
    }
    for sq in Squares(board.pieces(them, Piece::Queen)) {
        attackers += (queen_attacks(sq as usize, occ) & zone != 0) as u32;
    }
    cp += attackers as f32 * k.attack;

    // Back rank: no empty square in front of the king on its home rank, and heavy pieces still about.
    let home = if side == Color::White { 0 } else { 7 };
    if rank == home {
        let r = rank + forward;
        let mut shut = true;
        for f in (file - 1).max(0)..=(file + 1).min(7) {
            if board.occ[side as usize] & (1u64 << (r * 8 + f)) == 0 {
                shut = false;
            }
        }
        let heavy = board.pieces(them, Piece::Rook) | board.pieces(them, Piece::Queen);
        if shut && heavy != 0 {
            cp += k.luft;
        }
    }
    cp
}

impl Block for King {
    fn control(&mut self, args: &[f32]) {
        self.shield = args[0];
        self.open = args[1];
        self.attack = args[2];
        self.luft = args[3];
    }

    fn process(&mut self, board: &Board, _ctx: &Ctx, io: &mut Frame) {
        for (i, side) in SIDES.iter().enumerate() {
            let cp = exposure(board, *side, self);
            self.meters[i] = cp;
            io.ch[i] -= cp;
        }
    }

    fn meter(&self, index: usize) -> f32 {
        self.meters[index]
    }
}
