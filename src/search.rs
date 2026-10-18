//! The transport: iterative deepening alpha-beta with a quiescence search over captures, driving the board up
//! and down the tree with make and unmake, and asking the graph for a score at the leaves. Moves are ordered by
//! the previous iteration's best line, captures by most valuable victim first, then killer moves.

use crate::block::Frame;
use crate::board::{Board, Color, Move, Piece};
use crate::graph::Graph;
use crate::movegen::legal_moves;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Wall-clock time where the platform has one. On wasm32-unknown-unknown there is no clock, so elapsed time
/// reads as zero and only depth and node limits apply.
#[cfg(not(target_arch = "wasm32"))]
mod clock {
    pub struct Clock(std::time::Instant);
    impl Clock {
        pub fn start() -> Clock {
            Clock(std::time::Instant::now())
        }
        pub fn elapsed(&self) -> std::time::Duration {
            self.0.elapsed()
        }
    }
}
#[cfg(target_arch = "wasm32")]
mod clock {
    pub struct Clock;
    impl Clock {
        pub fn start() -> Clock {
            Clock
        }
        pub fn elapsed(&self) -> std::time::Duration {
            std::time::Duration::ZERO
        }
    }
}
use clock::Clock;

pub const MATE: i32 = 32000;
/// Scores at or beyond this are mates; the distance is MATE minus the score.
pub const MATE_BOUND: i32 = MATE - 1000;
const MAX_PLY: usize = 128;

#[derive(Clone, Debug, Default)]
pub struct Limits {
    pub depth: Option<u32>,
    pub nodes: Option<u64>,
    pub movetime: Option<Duration>,
    pub wtime: Option<Duration>,
    pub btime: Option<Duration>,
    pub winc: Option<Duration>,
    pub binc: Option<Duration>,
    pub infinite: bool,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub depth: u32,
    pub score: i32,
    pub nodes: u64,
    pub elapsed: Duration,
    pub pv: Vec<Move>,
}

impl Report {
    pub fn best(&self) -> Option<Move> {
        self.pv.first().copied()
    }

    /// `cp 12` or `mate 3` as UCI writes it, from the side to move.
    pub fn uci_score(&self) -> String {
        if self.score >= MATE_BOUND {
            format!("mate {}", (MATE - self.score + 1) / 2)
        } else if self.score <= -MATE_BOUND {
            format!("mate -{}", (MATE + self.score + 1) / 2)
        } else {
            format!("cp {}", self.score)
        }
    }
}

pub struct Searcher {
    pub graph: Graph,
    pub stop: Arc<AtomicBool>,
    /// Hashes of the positions played in the game so far, newest last, for repetition.
    pub history: Vec<u64>,
    /// The game ply at the root, which the graph's expressions read.
    pub root_ply: u32,
    nodes: u64,
    start: Clock,
    deadline: Option<Duration>,
    node_limit: Option<u64>,
    killers: Vec<[Option<Move>; 2]>,
    path: Vec<u64>,
    pv: Vec<Vec<Move>>,
    aborted: bool,
}

const VAL: [i32; 6] = [100, 320, 330, 500, 900, 10000];

impl Searcher {
    pub fn new(graph: Graph) -> Searcher {
        Searcher {
            graph,
            stop: Arc::new(AtomicBool::new(false)),
            history: Vec::new(),
            root_ply: 0,
            nodes: 0,
            start: Clock::start(),
            deadline: None,
            node_limit: None,
            killers: vec![[None, None]; MAX_PLY],
            path: Vec::with_capacity(MAX_PLY),
            pv: vec![Vec::new(); MAX_PLY + 1],
            aborted: false,
        }
    }

    /// The graph's score from the side to move, in centipawns.
    pub fn evaluate(&mut self, b: &Board, ply: usize) -> i32 {
        let f: Frame = self.graph.eval(b, self.root_ply + ply as u32);
        let white = f.white().round() as i32;
        if b.turn == Color::White {
            white
        } else {
            -white
        }
    }

    /// Searches `board` within `limits`, calling `report` after each completed depth.
    pub fn search(
        &mut self,
        board: &mut Board,
        limits: &Limits,
        mut report: impl FnMut(&Report),
    ) -> Report {
        self.nodes = 0;
        self.start = Clock::start();
        self.aborted = false;
        self.stop.store(false, Ordering::Relaxed);
        self.node_limit = limits.nodes;
        self.killers.iter_mut().for_each(|k| *k = [None, None]);
        self.deadline = time_budget(board.turn, limits);
        let max_depth = limits.depth.unwrap_or(64).min(MAX_PLY as u32 - 2);
        let mut best = Report {
            depth: 0,
            score: 0,
            nodes: 0,
            elapsed: Duration::ZERO,
            pv: Vec::new(),
        };
        let moves = legal_moves(board);
        if moves.is_empty() {
            return best;
        }
        if moves.len() == 1 && !limits.infinite && limits.depth.is_none() {
            best.pv = moves;
            best.depth = 1;
            best.score = self.evaluate(board, 0);
            report(&best);
            return best;
        }
        for depth in 1..=max_depth {
            self.path.clear();
            let score = self.negamax(board, depth as i32, -MATE - 1, MATE + 1, 0);
            if self.aborted {
                break;
            }
            best = Report {
                depth,
                score,
                nodes: self.nodes,
                elapsed: self.start.elapsed(),
                pv: self.pv[0].clone(),
            };
            report(&best);
            if score.abs() >= MATE_BOUND {
                break;
            }
            // Don't start a depth that cannot finish in the time left.
            if let Some(dl) = self.deadline
                && self.start.elapsed() * 3 > dl
            {
                break;
            }
        }
        best.nodes = self.nodes;
        best.elapsed = self.start.elapsed();
        best
    }

    fn out_of_time(&mut self) -> bool {
        if self.aborted {
            return true;
        }
        if self.nodes & 1023 == 0 {
            if self.stop.load(Ordering::Relaxed) {
                self.aborted = true;
            } else if let Some(dl) = self.deadline
                && self.start.elapsed() >= dl
            {
                self.aborted = true;
            }
        }
        if let Some(n) = self.node_limit
            && self.nodes >= n
        {
            self.aborted = true;
        }
        self.aborted
    }

    fn repeated(&self, hash: u64) -> bool {
        self.path.contains(&hash)
            || self
                .history
                .iter()
                .rev()
                .skip(1)
                .step_by(2)
                .any(|h| *h == hash)
    }

    fn negamax(&mut self, b: &mut Board, depth: i32, mut alpha: i32, beta: i32, ply: usize) -> i32 {
        self.pv[ply].clear();
        if depth <= 0 || ply >= MAX_PLY - 2 {
            return self.quiesce(b, alpha, beta, ply);
        }
        self.nodes += 1;
        if self.out_of_time() {
            return 0;
        }
        let hash = b.hash();
        if ply > 0 && (b.halfmove >= 100 || self.repeated(hash) || b.bare()) {
            return 0;
        }
        let mut moves = legal_moves(b);
        if moves.is_empty() {
            return if b.in_check() { -MATE + ply as i32 } else { 0 };
        }
        let in_check = b.in_check();
        let depth = if in_check { depth + 1 } else { depth };
        self.order(b, &mut moves, ply);
        let mut best = -MATE - 1;
        self.path.push(hash);
        for mv in moves {
            b.make(mv);
            let score = -self.negamax(b, depth - 1, -beta, -alpha, ply + 1);
            b.unmake();
            if self.aborted {
                self.path.pop();
                return 0;
            }
            if score > best {
                best = score;
                if score > alpha {
                    alpha = score;
                    let tail = std::mem::take(&mut self.pv[ply + 1]);
                    self.pv[ply].clear();
                    self.pv[ply].push(mv);
                    self.pv[ply].extend(tail.iter());
                    self.pv[ply + 1] = tail;
                }
            }
            if alpha >= beta {
                if !mv.is_capture() && mv.promo.is_none() {
                    let k = &mut self.killers[ply];
                    if k[0] != Some(mv) {
                        k[1] = k[0];
                        k[0] = Some(mv);
                    }
                }
                break;
            }
        }
        self.path.pop();
        best
    }

    fn quiesce(&mut self, b: &mut Board, mut alpha: i32, beta: i32, ply: usize) -> i32 {
        self.nodes += 1;
        if self.out_of_time() {
            return 0;
        }
        let stand = self.evaluate(b, ply);
        if stand >= beta {
            return stand;
        }
        if stand > alpha {
            alpha = stand;
        }
        if ply >= MAX_PLY - 2 {
            return stand;
        }
        let mut moves = legal_moves(b);
        moves.retain(|m| m.is_capture() || m.promo.is_some());
        self.order(b, &mut moves, ply);
        let mut best = stand;
        for mv in moves {
            b.make(mv);
            let score = -self.quiesce(b, -beta, -alpha, ply + 1);
            b.unmake();
            if self.aborted {
                return 0;
            }
            if score > best {
                best = score;
                if score > alpha {
                    alpha = score;
                }
            }
            if alpha >= beta {
                break;
            }
        }
        best
    }

    fn order(&self, b: &Board, moves: &mut [Move], ply: usize) {
        let first = self.pv[ply].first().copied();
        let killers = self.killers[ply];
        let key = |m: &Move| -> i32 {
            if Some(*m) == first {
                return 1_000_000;
            }
            let mut s = 0;
            if m.is_capture() {
                let victim = b
                    .piece_on(m.to)
                    .map_or(VAL[Piece::Pawn as usize], |(_, p)| VAL[p as usize]);
                let attacker = b.piece_on(m.from).map_or(0, |(_, p)| VAL[p as usize]);
                s += 100_000 + victim * 10 - attacker / 10;
            }
            if let Some(p) = m.promo {
                s += 90_000 + VAL[p as usize];
            }
            if !m.is_capture() {
                if killers[0] == Some(*m) {
                    s += 80_000;
                } else if killers[1] == Some(*m) {
                    s += 70_000;
                }
            }
            s
        };
        moves.sort_by_key(|m| -key(m));
    }

    pub fn nodes(&self) -> u64 {
        self.nodes
    }
}

/// How long to think, from the clocks or a fixed time.
fn time_budget(turn: Color, l: &Limits) -> Option<Duration> {
    if l.infinite {
        return None;
    }
    if let Some(t) = l.movetime {
        return Some(t);
    }
    let (time, inc) = match turn {
        Color::White => (l.wtime, l.winc),
        Color::Black => (l.btime, l.binc),
    };
    let time = time?;
    let inc = inc.unwrap_or(Duration::ZERO);
    let budget = time / 30 + inc / 2;
    Some(
        budget.min(
            time.saturating_sub(Duration::from_millis(50))
                .max(Duration::from_millis(10)),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::DEFAULT_GRAPH;

    fn best(fen: &str, depth: u32) -> (String, i32) {
        let mut s = Searcher::new(Graph::from_json(DEFAULT_GRAPH).unwrap());
        let mut b = Board::from_fen(fen).unwrap();
        let r = s.search(
            &mut b,
            &Limits {
                depth: Some(depth),
                ..Default::default()
            },
            |_| {},
        );
        (r.best().map(|m| m.to_string()).unwrap_or_default(), r.score)
    }

    #[test]
    fn finds_a_mate_in_one_and_scores_it_as_mate() {
        let (m, s) = best("6k1/5ppp/8/8/8/8/8/R5K1 w - - 0 1", 2);
        assert_eq!(m, "a1a8");
        assert_eq!(s, MATE - 1);
    }

    #[test]
    fn takes_a_free_queen() {
        let (m, _) = best(
            "rnb1kbnr/pppp1ppp/8/4p3/3qP3/8/PPP2PPP/RNBQKBNR w KQkq - 0 3",
            2,
        );
        assert_eq!(m, "d1d4");
    }

    #[test]
    fn stalemate_is_a_draw_not_a_loss() {
        let (_, s) = best("7k/5Q2/8/8/8/8/8/6K1 b - - 0 1", 3);
        assert_eq!(s, 0);
    }
}
