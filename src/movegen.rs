//! Move generation: pseudo-legal moves from the bitboards, then each one made, checked and unmade, so the
//! rules for pins, en passant discoveries and castling through check all fall out of `is_attacked`.

use crate::board::{BK, BQ, Board, Color, Move, MoveKind, PROMOTIONS, Piece, Square, WK, WQ};
use crate::tables::{
    BLACK_PAWN, Bitboard, KING, KNIGHT, RANK_2, RANK_7, WHITE_PAWN, bishop_attacks, queen_attacks,
    rook_attacks,
};

#[inline]
fn pop(bb: &mut Bitboard) -> Square {
    let sq = bb.trailing_zeros() as Square;
    *bb &= *bb - 1;
    sq
}

/// Every pseudo-legal move for the side to move: the piece can move that way, but the king may be left in check.
pub fn pseudo_legal(b: &Board, out: &mut Vec<Move>) {
    out.clear();
    let us = b.turn;
    let them = us.flip();
    let own = b.occ[us as usize];
    let enemy = b.occ[them as usize];
    let all = own | enemy;
    let empty = !all;

    // Pawns.
    let pawns = b.pieces(us, Piece::Pawn);
    let (push_shift, start_rank, promo_rank, attacks): (i8, Bitboard, Bitboard, &[Bitboard; 64]) =
        match us {
            Color::White => (8, RANK_2, RANK_7, &WHITE_PAWN),
            Color::Black => (-8, RANK_7, RANK_2, &BLACK_PAWN),
        };
    let mut p = pawns;
    while p != 0 {
        let from = pop(&mut p);
        let from_bb = 1u64 << from;
        let one = (from as i8 + push_shift) as Square;
        let promoting = from_bb & promo_rank != 0;
        if empty & (1u64 << one) != 0 {
            if promoting {
                for promo in PROMOTIONS {
                    out.push(Move {
                        from,
                        to: one,
                        promo: Some(promo),
                        kind: MoveKind::Quiet,
                    });
                }
            } else {
                out.push(Move {
                    from,
                    to: one,
                    promo: None,
                    kind: MoveKind::Quiet,
                });
                if from_bb & start_rank != 0 {
                    let two = (one as i8 + push_shift) as Square;
                    if empty & (1u64 << two) != 0 {
                        out.push(Move {
                            from,
                            to: two,
                            promo: None,
                            kind: MoveKind::DoublePush,
                        });
                    }
                }
            }
        }
        let mut caps = attacks[from as usize] & enemy;
        while caps != 0 {
            let to = pop(&mut caps);
            if promoting {
                for promo in PROMOTIONS {
                    out.push(Move {
                        from,
                        to,
                        promo: Some(promo),
                        kind: MoveKind::Capture,
                    });
                }
            } else {
                out.push(Move {
                    from,
                    to,
                    promo: None,
                    kind: MoveKind::Capture,
                });
            }
        }
        if let Some(ep) = b.ep
            && attacks[from as usize] & (1u64 << ep) != 0
        {
            out.push(Move {
                from,
                to: ep,
                promo: None,
                kind: MoveKind::EnPassant,
            });
        }
    }

    // Knights, bishops, rooks, queens, king.
    let mut push_targets = |from: Square, targets: Bitboard| {
        let mut quiet = targets & empty;
        while quiet != 0 {
            out.push(Move {
                from,
                to: pop(&mut quiet),
                promo: None,
                kind: MoveKind::Quiet,
            });
        }
        let mut caps = targets & enemy;
        while caps != 0 {
            out.push(Move {
                from,
                to: pop(&mut caps),
                promo: None,
                kind: MoveKind::Capture,
            });
        }
    };
    let mut n = b.pieces(us, Piece::Knight);
    while n != 0 {
        let from = pop(&mut n);
        push_targets(from, KNIGHT[from as usize]);
    }
    let mut bi = b.pieces(us, Piece::Bishop);
    while bi != 0 {
        let from = pop(&mut bi);
        push_targets(from, bishop_attacks(from as usize, all));
    }
    let mut r = b.pieces(us, Piece::Rook);
    while r != 0 {
        let from = pop(&mut r);
        push_targets(from, rook_attacks(from as usize, all));
    }
    let mut q = b.pieces(us, Piece::Queen);
    while q != 0 {
        let from = pop(&mut q);
        push_targets(from, queen_attacks(from as usize, all));
    }
    let king = b.king(us);
    push_targets(king, KING[king as usize]);

    // Castling: the right, empty squares between, and the king neither in check nor passing through one.
    let (ks, qs, home) = match us {
        Color::White => (WK, WQ, 4u8),
        Color::Black => (BK, BQ, 60u8),
    };
    if b.castling & (ks | qs) != 0 && king == home && !b.is_attacked(home, them) {
        if b.castling & ks != 0
            && all & (0b0110_0000u64 << (home - 4)) == 0
            && !b.is_attacked(home + 1, them)
            && !b.is_attacked(home + 2, them)
        {
            out.push(Move {
                from: home,
                to: home + 2,
                promo: None,
                kind: MoveKind::CastleKing,
            });
        }
        if b.castling & qs != 0
            && all & (0b0000_1110u64 << (home - 4)) == 0
            && !b.is_attacked(home - 1, them)
            && !b.is_attacked(home - 2, them)
        {
            out.push(Move {
                from: home,
                to: home - 2,
                promo: None,
                kind: MoveKind::CastleQueen,
            });
        }
    }
}

/// Every legal move for the side to move.
pub fn legal_moves(b: &mut Board) -> Vec<Move> {
    let mut moves = Vec::with_capacity(64);
    pseudo_legal(b, &mut moves);
    let us = b.turn;
    moves.retain(|&mv| {
        b.make(mv);
        let ok = !b.is_attacked(b.king(us), us.flip());
        b.unmake();
        ok
    });
    moves
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(fen: &str) -> usize {
        legal_moves(&mut Board::from_fen(fen).unwrap()).len()
    }

    #[test]
    fn the_start_position_has_twenty_moves() {
        assert_eq!(count(crate::board::START_FEN), 20);
    }

    #[test]
    fn a_pinned_piece_stays_put_and_a_king_cannot_castle_through_check() {
        assert_eq!(
            count("4k3/8/8/8/8/8/4R3/4K3 b - - 0 1"),
            4,
            "a king facing a rook has four squares off the file"
        );
        assert_eq!(
            count("4k3/8/8/8/8/8/8/R3K2R w KQ - 0 1"),
            26,
            "both castles are legal"
        );
        assert_eq!(
            count("4k3/8/8/8/8/5r2/8/R3K2R w KQ - 0 1"),
            23,
            "no castling king side across f1; e1-f1 and e1-f2 are out"
        );
    }

    #[test]
    fn en_passant_that_exposes_the_king_is_refused() {
        // White king a5 and pawn b5, black pawn c5 just pushed, black rook d5: bxc6 en passant would uncover the rook.
        assert_eq!(
            count("8/8/8/KPpr4/8/8/8/4k3 w - c6 0 1"),
            4,
            "bxc6 is illegal; b6 and three king moves remain"
        );
    }
}
