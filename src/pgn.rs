//! Reads PGN as Lichess exports it: tag pairs, then move text with clock and eval comments, which are dropped.
//! Standard algebraic notation is resolved against the legal moves, so an illegal or ambiguous move is an error
//! that names the ply.

use crate::board::{Board, Move, MoveKind, Piece, parse_square};
use crate::movegen::legal_moves;

#[derive(Clone, Debug, Default)]
pub struct Game {
    pub tags: Vec<(String, String)>,
    /// The moves as written, in order.
    pub sans: Vec<String>,
}

impl Game {
    pub fn tag(&self, name: &str) -> Option<&str> {
        self.tags
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// The position before the first move: the FEN tag, or the start.
    pub fn start(&self) -> Result<Board, String> {
        match self.tag("FEN") {
            Some(f) => Board::from_fen(f),
            None => Ok(Board::start()),
        }
    }
}

/// Every game in a PGN text.
pub fn read(text: &str) -> Vec<Game> {
    let mut games = Vec::new();
    let mut game = Game::default();
    let mut in_moves = false;
    let mut depth = 0usize;
    let mut comment = false;
    let mut movetext = String::new();

    let flush = |game: &mut Game, movetext: &mut String, games: &mut Vec<Game>| {
        if !game.tags.is_empty() || !movetext.trim().is_empty() {
            game.sans = tokens(movetext);
            games.push(std::mem::take(game));
        }
        movetext.clear();
    };

    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') && !in_moves && !comment {
            if let Some((k, v)) = parse_tag(t) {
                game.tags.push((k, v));
            }
            continue;
        }
        if t.starts_with('[') && in_moves && !comment && depth == 0 {
            // A new game's tags begin.
            flush(&mut game, &mut movetext, &mut games);
            in_moves = false;
            if let Some((k, v)) = parse_tag(t) {
                game.tags.push((k, v));
            }
            continue;
        }
        if t.is_empty() {
            continue;
        }
        in_moves = true;
        // Strip comments and variations, which may span lines.
        for ch in t.chars() {
            match ch {
                '{' if depth == 0 => comment = true,
                '}' if comment => comment = false,
                '(' if !comment => depth += 1,
                ')' if !comment && depth > 0 => depth -= 1,
                c if !comment && depth == 0 => movetext.push(c),
                _ => {}
            }
        }
        movetext.push(' ');
    }
    flush(&mut game, &mut movetext, &mut games);
    games
}

fn parse_tag(t: &str) -> Option<(String, String)> {
    let inner = t.strip_prefix('[')?.strip_suffix(']')?;
    let (k, v) = inner.split_once(' ')?;
    let v = v.trim().strip_prefix('"')?.strip_suffix('"')?;
    Some((k.to_string(), v.replace("\\\"", "\"")))
}

/// The SAN tokens of move text, without move numbers, NAGs or the result.
fn tokens(movetext: &str) -> Vec<String> {
    movetext
        .split_whitespace()
        .filter_map(|tok| {
            let tok = tok.trim_end_matches(['!', '?']);
            if tok.is_empty() || tok.starts_with('$') {
                return None;
            }
            if ["1-0", "0-1", "1/2-1/2", "*"].contains(&tok) {
                return None;
            }
            // "12." or "12..." prefixes, possibly glued to the move.
            let rest = tok.trim_start_matches(|c: char| c.is_ascii_digit());
            let rest = rest.trim_start_matches('.');
            if rest.is_empty() {
                return None;
            }
            Some(rest.to_string())
        })
        .collect()
}

/// Resolves a SAN move against the legal moves of the position.
pub fn san_to_move(board: &mut Board, san: &str) -> Result<Move, String> {
    let s = san.trim_end_matches(['+', '#', '!', '?']);
    let legal = legal_moves(board);
    if s == "O-O" || s == "0-0" {
        return legal
            .into_iter()
            .find(|m| m.kind == MoveKind::CastleKing)
            .ok_or_else(|| "castling king side is not legal here".to_string());
    }
    if s == "O-O-O" || s == "0-0-0" {
        return legal
            .into_iter()
            .find(|m| m.kind == MoveKind::CastleQueen)
            .ok_or_else(|| "castling queen side is not legal here".to_string());
    }
    let (body, promo) = match s.split_once('=') {
        Some((b, p)) => (b, Some(letter_to_piece(p.chars().next().unwrap_or('Q'))?)),
        None => (s, None),
    };
    let chars: Vec<char> = body.chars().collect();
    if chars.len() < 2 {
        return Err(format!("'{}' is not a move", san));
    }
    let piece = match chars[0] {
        'N' | 'B' | 'R' | 'Q' | 'K' => letter_to_piece(chars[0])?,
        _ => Piece::Pawn,
    };
    let start = if piece == Piece::Pawn { 0 } else { 1 };
    let dest = parse_square(&body[body.len() - 2..])
        .ok_or_else(|| format!("'{}' has no destination square", san))?;
    let middle: Vec<char> = chars[start..chars.len() - 2]
        .iter()
        .copied()
        .filter(|c| *c != 'x')
        .collect();
    let (mut from_file, mut from_rank) = (None, None);
    for c in middle {
        match c {
            'a'..='h' => from_file = Some(c as u8 - b'a'),
            '1'..='8' => from_rank = Some(c as u8 - b'1'),
            _ => return Err(format!("'{}' has an unexpected '{}'", san, c)),
        }
    }
    let matches: Vec<Move> = legal
        .into_iter()
        .filter(|m| {
            m.to == dest
                && board.piece_on(m.from).map(|(_, p)| p) == Some(piece)
                && m.promo == promo
                && from_file.is_none_or(|f| m.from % 8 == f)
                && from_rank.is_none_or(|r| m.from / 8 == r)
        })
        .collect();
    match matches.len() {
        1 => Ok(matches[0]),
        0 => Err(format!("'{}' is not a legal move here", san)),
        _ => Err(format!("'{}' is ambiguous here", san)),
    }
}

fn letter_to_piece(c: char) -> Result<Piece, String> {
    Ok(match c {
        'N' => Piece::Knight,
        'B' => Piece::Bishop,
        'R' => Piece::Rook,
        'Q' => Piece::Queen,
        'K' => Piece::King,
        _ => return Err(format!("'{}' is not a piece", c)),
    })
}

/// Plays a game's moves from its start, calling `f` with the ply number (1 for the first move), the SAN, the move
/// and the board after it.
pub fn replay(game: &Game, mut f: impl FnMut(u32, &str, Move, &Board)) -> Result<Board, String> {
    let mut b = game.start()?;
    for (i, san) in game.sans.iter().enumerate() {
        let mv = san_to_move(&mut b, san).map_err(|e| format!("ply {}: {}", i + 1, e))?;
        b.make(mv);
        f(i as u32 + 1, san, mv, &b);
    }
    Ok(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[Event "rated rapid game"]
[Site "https://lichess.org/abc"]
[White "someone"]
[Black "strikethree"]
[Result "0-1"]
[GameId "abc"]

1. e4 { [%clk 0:10:00] } 1... Nc6 { [%clk 0:10:00] } 2. d4 { [%clk 0:09:56] } 2... e5 3. d5 Nce7 4. Nf3 Ng6 5. Bc4 Bc5 6. O-O 0-1

[Event "second"]
[GameId "def"]

1. d4 d5 2. c4 dxc4 3. e4 e5 4. dxe5 Qxd1+ 5. Kxd1 *
"#;

    #[test]
    fn games_split_and_comments_drop() {
        let games = read(SAMPLE);
        assert_eq!(games.len(), 2);
        assert_eq!(games[0].tag("GameId"), Some("abc"));
        assert_eq!(
            games[0].sans,
            [
                "e4", "Nc6", "d4", "e5", "d5", "Nce7", "Nf3", "Ng6", "Bc4", "Bc5", "O-O"
            ]
        );
        assert_eq!(games[1].sans.len(), 9);
    }

    #[test]
    fn san_resolves_disambiguation_castling_captures_and_promotion() {
        let games = read(SAMPLE);
        let end = replay(&games[0], |_, _, _, _| {}).unwrap();
        assert_eq!(
            end.fen(),
            "r1bqk1nr/pppp1ppp/6n1/2bPp3/2B1P3/5N2/PPP2PPP/RNBQ1RK1 b kq - 6 6"
        );
        let end = replay(&games[1], |_, _, _, _| {}).unwrap();
        assert_eq!(
            end.fen(),
            "rnb1kbnr/ppp2ppp/8/4P3/2p1P3/8/PP3PPP/RNBK1BNR b kq - 0 5"
        );
        let mut b = Board::from_fen("4k3/1P6/8/8/8/8/8/4K3 w - - 0 1").unwrap();
        let m = san_to_move(&mut b, "b8=N+").unwrap();
        assert_eq!(m.promo, Some(Piece::Knight));
        assert!(
            san_to_move(&mut b, "Nf3")
                .unwrap_err()
                .contains("not a legal move")
        );
    }
}
