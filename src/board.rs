//! The board is the stateful actor. It takes `make` and `unmake` as messages and keeps its own undo stack, so
//! a block that reads it can be driven up and down the search tree without copying anything.
//!
//! Squares are 0 = a1 … 63 = h8, rank-major. Castling rights are four bits: 1 white king side, 2 white queen
//! side, 4 black king side, 8 black queen side.

use crate::tables::{BLACK_PAWN, Bitboard, KING, KNIGHT, WHITE_PAWN, bishop_attacks, rook_attacks};
use std::fmt;

pub type Square = u8;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Color {
    White = 0,
    Black = 1,
}

impl Color {
    #[inline]
    pub fn flip(self) -> Color {
        match self {
            Color::White => Color::Black,
            Color::Black => Color::White,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Piece {
    Pawn = 0,
    Knight = 1,
    Bishop = 2,
    Rook = 3,
    Queen = 4,
    King = 5,
}

pub const PIECES: [Piece; 6] = [
    Piece::Pawn,
    Piece::Knight,
    Piece::Bishop,
    Piece::Rook,
    Piece::Queen,
    Piece::King,
];
pub const PROMOTIONS: [Piece; 4] = [Piece::Queen, Piece::Rook, Piece::Bishop, Piece::Knight];

impl Piece {
    pub fn letter(self, color: Color) -> char {
        let c = match self {
            Piece::Pawn => 'p',
            Piece::Knight => 'n',
            Piece::Bishop => 'b',
            Piece::Rook => 'r',
            Piece::Queen => 'q',
            Piece::King => 'k',
        };
        if color == Color::White {
            c.to_ascii_uppercase()
        } else {
            c
        }
    }

    fn from_letter(c: char) -> Option<(Color, Piece)> {
        let color = if c.is_ascii_uppercase() {
            Color::White
        } else {
            Color::Black
        };
        let piece = match c.to_ascii_lowercase() {
            'p' => Piece::Pawn,
            'n' => Piece::Knight,
            'b' => Piece::Bishop,
            'r' => Piece::Rook,
            'q' => Piece::Queen,
            'k' => Piece::King,
            _ => return None,
        };
        Some((color, piece))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum MoveKind {
    Quiet,
    Capture,
    DoublePush,
    EnPassant,
    CastleKing,
    CastleQueen,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Move {
    pub from: Square,
    pub to: Square,
    pub promo: Option<Piece>,
    pub kind: MoveKind,
}

impl Move {
    pub fn is_capture(&self) -> bool {
        matches!(self.kind, MoveKind::Capture | MoveKind::EnPassant)
    }
}

/// Long algebraic, as UCI writes it: e2e4, e7e8q.
impl fmt::Display for Move {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", square_name(self.from), square_name(self.to))?;
        if let Some(p) = self.promo {
            write!(f, "{}", p.letter(Color::Black))?;
        }
        Ok(())
    }
}

pub fn square_name(sq: Square) -> String {
    let file = (b'a' + sq % 8) as char;
    let rank = (b'1' + sq / 8) as char;
    format!("{file}{rank}")
}

pub fn parse_square(s: &str) -> Option<Square> {
    let b = s.as_bytes();
    if b.len() != 2 || !(b'a'..=b'h').contains(&b[0]) || !(b'1'..=b'8').contains(&b[1]) {
        return None;
    }
    Some((b[1] - b'1') * 8 + (b[0] - b'a'))
}

pub const WK: u8 = 1;
pub const WQ: u8 = 2;
pub const BK: u8 = 4;
pub const BQ: u8 = 8;

/// Castling rights that survive a move touching each square: a king or rook leaving home, or a rook being
/// captured at home, drops the matching right.
const CASTLE_MASK: [u8; 64] = {
    let mut m = [0xFu8; 64];
    m[0] = 0xF & !WQ; // a1
    m[4] = 0xF & !(WK | WQ); // e1
    m[7] = 0xF & !WK; // h1
    m[56] = 0xF & !BQ; // a8
    m[60] = 0xF & !(BK | BQ); // e8
    m[63] = 0xF & !BK; // h8
    m
};

#[derive(Clone, Copy, Debug)]
struct Undo {
    mv: Move,
    captured: Option<Piece>,
    castling: u8,
    ep: Option<Square>,
    halfmove: u16,
}

#[derive(Clone, Debug)]
pub struct Board {
    /// Piece bitboards by color then piece.
    pub bb: [[Bitboard; 6]; 2],
    /// All pieces of each color.
    pub occ: [Bitboard; 2],
    pub turn: Color,
    pub castling: u8,
    pub ep: Option<Square>,
    pub halfmove: u16,
    pub fullmove: u16,
    undo: Vec<Undo>,
}

pub const START_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

impl Board {
    pub fn start() -> Board {
        Board::from_fen(START_FEN).expect("the start position parses")
    }

    pub fn from_fen(fen: &str) -> Result<Board, String> {
        let mut parts = fen.split_whitespace();
        let placement = parts.next().ok_or("the FEN is empty")?;
        let mut b = Board {
            bb: [[0; 6]; 2],
            occ: [0; 2],
            turn: Color::White,
            castling: 0,
            ep: None,
            halfmove: 0,
            fullmove: 1,
            undo: Vec::with_capacity(128),
        };
        let ranks: Vec<&str> = placement.split('/').collect();
        if ranks.len() != 8 {
            return Err(format!("the placement has {} ranks, not 8", ranks.len()));
        }
        for (i, rank) in ranks.iter().enumerate() {
            let r = 7 - i;
            let mut f = 0usize;
            for c in rank.chars() {
                if let Some(n) = c.to_digit(10) {
                    f += n as usize;
                } else {
                    let (color, piece) =
                        Piece::from_letter(c).ok_or(format!("'{c}' is not a piece"))?;
                    if f > 7 {
                        return Err(format!("rank {} has more than 8 squares", r + 1));
                    }
                    b.put(color, piece, (r * 8 + f) as Square);
                    f += 1;
                }
            }
            if f != 8 {
                return Err(format!("rank {} has {} squares, not 8", r + 1, f));
            }
        }
        b.turn = match parts.next().unwrap_or("w") {
            "w" => Color::White,
            "b" => Color::Black,
            other => return Err(format!("'{other}' is not a side to move")),
        };
        for c in parts.next().unwrap_or("-").chars() {
            b.castling |= match c {
                'K' => WK,
                'Q' => WQ,
                'k' => BK,
                'q' => BQ,
                '-' => 0,
                other => return Err(format!("'{other}' is not a castling right")),
            };
        }
        b.ep = match parts.next().unwrap_or("-") {
            "-" => None,
            s => Some(parse_square(s).ok_or(format!("'{s}' is not an en passant square"))?),
        };
        b.halfmove = parts
            .next()
            .unwrap_or("0")
            .parse()
            .map_err(|_| "the halfmove clock is not a number")?;
        b.fullmove = parts
            .next()
            .unwrap_or("1")
            .parse()
            .map_err(|_| "the move number is not a number")?;
        Ok(b)
    }

    pub fn fen(&self) -> String {
        let mut s = String::new();
        for r in (0..8).rev() {
            let mut empty = 0;
            for f in 0..8 {
                match self.piece_on((r * 8 + f) as Square) {
                    Some((c, p)) => {
                        if empty > 0 {
                            s.push(char::from_digit(empty, 10).unwrap());
                            empty = 0;
                        }
                        s.push(p.letter(c));
                    }
                    None => empty += 1,
                }
            }
            if empty > 0 {
                s.push(char::from_digit(empty, 10).unwrap());
            }
            if r > 0 {
                s.push('/');
            }
        }
        s.push(' ');
        s.push(if self.turn == Color::White { 'w' } else { 'b' });
        s.push(' ');
        if self.castling == 0 {
            s.push('-');
        } else {
            for (bit, c) in [(WK, 'K'), (WQ, 'Q'), (BK, 'k'), (BQ, 'q')] {
                if self.castling & bit != 0 {
                    s.push(c);
                }
            }
        }
        s.push(' ');
        match self.ep {
            Some(sq) => s.push_str(&square_name(sq)),
            None => s.push('-'),
        }
        s.push_str(&format!(" {} {}", self.halfmove, self.fullmove));
        s
    }

    #[inline]
    pub fn all(&self) -> Bitboard {
        self.occ[0] | self.occ[1]
    }

    #[inline]
    pub fn pieces(&self, color: Color, piece: Piece) -> Bitboard {
        self.bb[color as usize][piece as usize]
    }

    #[inline]
    pub fn king(&self, color: Color) -> Square {
        self.pieces(color, Piece::King).trailing_zeros() as Square
    }

    pub fn piece_on(&self, sq: Square) -> Option<(Color, Piece)> {
        let m = 1u64 << sq;
        for color in [Color::White, Color::Black] {
            if self.occ[color as usize] & m != 0 {
                for p in PIECES {
                    if self.bb[color as usize][p as usize] & m != 0 {
                        return Some((color, p));
                    }
                }
            }
        }
        None
    }

    #[inline]
    fn put(&mut self, color: Color, piece: Piece, sq: Square) {
        let m = 1u64 << sq;
        self.bb[color as usize][piece as usize] |= m;
        self.occ[color as usize] |= m;
    }

    #[inline]
    fn take(&mut self, color: Color, piece: Piece, sq: Square) {
        let m = 1u64 << sq;
        self.bb[color as usize][piece as usize] &= !m;
        self.occ[color as usize] &= !m;
    }

    #[inline]
    fn piece_of(&self, color: Color, sq: Square) -> Piece {
        let m = 1u64 << sq;
        for p in PIECES {
            if self.bb[color as usize][p as usize] & m != 0 {
                return p;
            }
        }
        unreachable!("no {:?} piece on {}", color, square_name(sq))
    }

    /// Whether `by` attacks `sq` on the board as it stands.
    pub fn is_attacked(&self, sq: Square, by: Color) -> bool {
        let s = sq as usize;
        let them = by as usize;
        let pawns = match by {
            Color::White => BLACK_PAWN[s],
            Color::Black => WHITE_PAWN[s],
        };
        if pawns & self.bb[them][Piece::Pawn as usize] != 0 {
            return true;
        }
        if KNIGHT[s] & self.bb[them][Piece::Knight as usize] != 0 {
            return true;
        }
        if KING[s] & self.bb[them][Piece::King as usize] != 0 {
            return true;
        }
        let occ = self.all();
        let diag = self.bb[them][Piece::Bishop as usize] | self.bb[them][Piece::Queen as usize];
        if diag != 0 && bishop_attacks(s, occ) & diag != 0 {
            return true;
        }
        let line = self.bb[them][Piece::Rook as usize] | self.bb[them][Piece::Queen as usize];
        line != 0 && rook_attacks(s, occ) & line != 0
    }

    pub fn in_check(&self) -> bool {
        self.is_attacked(self.king(self.turn), self.turn.flip())
    }

    /// Plays a pseudo-legal move. Legality is the caller's to check, with `in_check` after `make` from the
    /// mover's side, or through `movegen::legal_moves`.
    pub fn make(&mut self, mv: Move) {
        let us = self.turn;
        let them = us.flip();
        let piece = self.piece_of(us, mv.from);
        let mut undo = Undo {
            mv,
            captured: None,
            castling: self.castling,
            ep: self.ep,
            halfmove: self.halfmove,
        };
        self.ep = None;
        self.halfmove += 1;
        match mv.kind {
            MoveKind::Capture => {
                let captured = self.piece_of(them, mv.to);
                self.take(them, captured, mv.to);
                undo.captured = Some(captured);
                self.halfmove = 0;
            }
            MoveKind::EnPassant => {
                let cap = if us == Color::White {
                    mv.to - 8
                } else {
                    mv.to + 8
                };
                self.take(them, Piece::Pawn, cap);
                undo.captured = Some(Piece::Pawn);
                self.halfmove = 0;
            }
            MoveKind::DoublePush => self.ep = Some((mv.from + mv.to) / 2),
            MoveKind::CastleKing => {
                let (rf, rt) = if us == Color::White { (7, 5) } else { (63, 61) };
                self.take(us, Piece::Rook, rf);
                self.put(us, Piece::Rook, rt);
            }
            MoveKind::CastleQueen => {
                let (rf, rt) = if us == Color::White { (0, 3) } else { (56, 59) };
                self.take(us, Piece::Rook, rf);
                self.put(us, Piece::Rook, rt);
            }
            MoveKind::Quiet => {}
        }
        self.take(us, piece, mv.from);
        self.put(us, mv.promo.unwrap_or(piece), mv.to);
        if piece == Piece::Pawn {
            self.halfmove = 0;
        }
        self.castling &= CASTLE_MASK[mv.from as usize] & CASTLE_MASK[mv.to as usize];
        if us == Color::Black {
            self.fullmove += 1;
        }
        self.turn = them;
        self.undo.push(undo);
    }

    pub fn unmake(&mut self) {
        let undo = self.undo.pop().expect("unmake with nothing to undo");
        let mv = undo.mv;
        let us = self.turn.flip();
        let them = self.turn;
        let placed = self.piece_of(us, mv.to);
        self.take(us, placed, mv.to);
        self.put(
            us,
            if mv.promo.is_some() {
                Piece::Pawn
            } else {
                placed
            },
            mv.from,
        );
        match mv.kind {
            MoveKind::Capture => self.put(them, undo.captured.unwrap(), mv.to),
            MoveKind::EnPassant => {
                let cap = if us == Color::White {
                    mv.to - 8
                } else {
                    mv.to + 8
                };
                self.put(them, Piece::Pawn, cap);
            }
            MoveKind::CastleKing => {
                let (rf, rt) = if us == Color::White { (7, 5) } else { (63, 61) };
                self.take(us, Piece::Rook, rt);
                self.put(us, Piece::Rook, rf);
            }
            MoveKind::CastleQueen => {
                let (rf, rt) = if us == Color::White { (0, 3) } else { (56, 59) };
                self.take(us, Piece::Rook, rt);
                self.put(us, Piece::Rook, rf);
            }
            MoveKind::Quiet | MoveKind::DoublePush => {}
        }
        self.castling = undo.castling;
        self.ep = undo.ep;
        self.halfmove = undo.halfmove;
        if us == Color::Black {
            self.fullmove -= 1;
        }
        self.turn = us;
    }

    /// How many moves have been made and not unmade.
    pub fn depth(&self) -> usize {
        self.undo.len()
    }
}

impl fmt::Display for Board {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for r in (0..8).rev() {
            for file in 0..8 {
                let c = self
                    .piece_on((r * 8 + file) as Square)
                    .map(|(c, p)| p.letter(c))
                    .unwrap_or('.');
                write!(f, "{c} ")?;
            }
            writeln!(f)?;
        }
        write!(f, "{}", self.fen())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fen_round_trips() {
        for fen in [
            START_FEN,
            "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
            "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
            "rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq e6 0 2",
        ] {
            assert_eq!(Board::from_fen(fen).unwrap().fen(), fen);
        }
    }

    #[test]
    fn a_bad_fen_says_what_is_wrong() {
        assert!(
            Board::from_fen("rnbqkbnr/pppppppp/8/8 w - - 0 1")
                .unwrap_err()
                .contains("ranks")
        );
        assert!(
            Board::from_fen("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNX w - - 0 1")
                .unwrap_err()
                .contains("'X'")
        );
    }

    #[test]
    fn make_then_unmake_restores_the_board() {
        let mut b = Board::start();
        let before = b.fen();
        let e4 = Move {
            from: 12,
            to: 28,
            promo: None,
            kind: MoveKind::DoublePush,
        };
        b.make(e4);
        assert_eq!(b.ep, Some(20), "e3 is the en passant square after e4");
        assert_eq!(b.turn, Color::Black);
        b.unmake();
        assert_eq!(b.fen(), before);
    }

    #[test]
    fn attacks_see_through_nothing_and_stop_at_pieces() {
        let b = Board::from_fen("4k3/8/8/8/8/8/4P3/4K2R w K - 0 1").unwrap();
        assert!(
            b.is_attacked(parse_square("d3").unwrap(), Color::White),
            "the e2 pawn attacks d3"
        );
        assert!(
            b.is_attacked(parse_square("h7").unwrap(), Color::White),
            "the h1 rook sees up the file"
        );
        assert!(
            !b.is_attacked(parse_square("e8").unwrap(), Color::White),
            "the e2 pawn blocks the king file"
        );
    }
}
