//! Attack tables, built at compile time. Squares are 0 = a1 … 63 = h8, rank-major.

pub type Bitboard = u64;

/// Ray directions, index into `RAYS`. Positive rays (the target square is higher) are N, NE, E, NW;
/// the first blocker on those is the lowest set bit. On the negative rays it is the highest.
pub const N: usize = 0;
pub const NE: usize = 1;
pub const E: usize = 2;
pub const SE: usize = 3;
pub const S: usize = 4;
pub const SW: usize = 5;
pub const W: usize = 6;
pub const NW: usize = 7;
pub const ROOK_DIRS: [usize; 4] = [N, E, S, W];
pub const BISHOP_DIRS: [usize; 4] = [NE, SE, SW, NW];

const DIRS: [(i32, i32); 8] = [
    (0, 1),
    (1, 1),
    (1, 0),
    (1, -1),
    (0, -1),
    (-1, -1),
    (-1, 0),
    (-1, 1),
];

const fn bit(file: i32, rank: i32) -> Bitboard {
    1u64 << (rank * 8 + file)
}

const fn build_rays() -> [[Bitboard; 64]; 8] {
    let mut t = [[0u64; 64]; 8];
    let mut d = 0;
    while d < 8 {
        let mut sq = 0;
        while sq < 64 {
            let (df, dr) = DIRS[d];
            let mut f = (sq % 8) as i32 + df;
            let mut r = (sq / 8) as i32 + dr;
            let mut bb = 0u64;
            while f >= 0 && f < 8 && r >= 0 && r < 8 {
                bb |= bit(f, r);
                f += df;
                r += dr;
            }
            t[d][sq] = bb;
            sq += 1;
        }
        d += 1;
    }
    t
}

const fn build_steps(offsets: &[(i32, i32)]) -> [Bitboard; 64] {
    let mut t = [0u64; 64];
    let mut sq = 0;
    while sq < 64 {
        let mut i = 0;
        let mut bb = 0u64;
        while i < offsets.len() {
            let f = (sq % 8) as i32 + offsets[i].0;
            let r = (sq / 8) as i32 + offsets[i].1;
            if f >= 0 && f < 8 && r >= 0 && r < 8 {
                bb |= bit(f, r);
            }
            i += 1;
        }
        t[sq] = bb;
        sq += 1;
    }
    t
}

pub const RAYS: [[Bitboard; 64]; 8] = build_rays();
pub const KNIGHT: [Bitboard; 64] = build_steps(&[
    (1, 2),
    (2, 1),
    (2, -1),
    (1, -2),
    (-1, -2),
    (-2, -1),
    (-2, 1),
    (-1, 2),
]);
pub const KING: [Bitboard; 64] = build_steps(&[
    (0, 1),
    (1, 1),
    (1, 0),
    (1, -1),
    (0, -1),
    (-1, -1),
    (-1, 0),
    (-1, 1),
]);
/// Squares a white pawn on sq attacks.
pub const WHITE_PAWN: [Bitboard; 64] = build_steps(&[(-1, 1), (1, 1)]);
/// Squares a black pawn on sq attacks.
pub const BLACK_PAWN: [Bitboard; 64] = build_steps(&[(-1, -1), (1, -1)]);

pub const FILE_A: Bitboard = 0x0101_0101_0101_0101;
pub const RANK_1: Bitboard = 0xFF;
pub const RANK_2: Bitboard = RANK_1 << 8;
pub const RANK_7: Bitboard = RANK_1 << 48;
pub const RANK_8: Bitboard = RANK_1 << 56;

/// Attacks along one ray from sq, stopping at the first occupied square (which is included).
#[inline]
pub fn ray_attacks(dir: usize, sq: usize, occ: Bitboard) -> Bitboard {
    let ray = RAYS[dir][sq];
    let blockers = ray & occ;
    if blockers == 0 {
        return ray;
    }
    let first = if dir == N || dir == NE || dir == E || dir == NW {
        blockers.trailing_zeros() as usize
    } else {
        63 - blockers.leading_zeros() as usize
    };
    ray ^ RAYS[dir][first]
}

#[inline]
pub fn rook_attacks(sq: usize, occ: Bitboard) -> Bitboard {
    ROOK_DIRS
        .iter()
        .fold(0, |a, &d| a | ray_attacks(d, sq, occ))
}

#[inline]
pub fn bishop_attacks(sq: usize, occ: Bitboard) -> Bitboard {
    BISHOP_DIRS
        .iter()
        .fold(0, |a, &d| a | ray_attacks(d, sq, occ))
}

#[inline]
pub fn queen_attacks(sq: usize, occ: Bitboard) -> Bitboard {
    rook_attacks(sq, occ) | bishop_attacks(sq, occ)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rays_stop_at_the_first_blocker_in_both_directions() {
        // Rook on d4 (27) with blockers on d6 (43) and b4 (25).
        let occ = (1 << 43) | (1 << 25);
        let a = rook_attacks(27, occ);
        assert!(
            a & (1 << 43) != 0 && a & (1 << 51) == 0,
            "north stops at d6"
        );
        assert!(a & (1 << 25) != 0 && a & (1 << 24) == 0, "west stops at b4");
        assert_eq!((a & RAYS[S][27]).count_ones(), 3, "south runs to d1");
    }

    #[test]
    fn knight_on_the_rim_has_fewer_targets() {
        assert_eq!(KNIGHT[0].count_ones(), 2);
        assert_eq!(KNIGHT[27].count_ones(), 8);
        assert_eq!(KING[0].count_ones(), 3);
        assert_eq!(WHITE_PAWN[8], (1 << 17), "a2 pawn attacks b3 only");
    }
}
