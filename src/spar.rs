//! Sparring (B15): playing near the opponent's level, in casual games against people.
//!
//! The search runs as it always does and finds the best move and its score. This module looks at that result and, only
//! when she stands more than a pawn up, replaces the best move with a slightly worse one that leaves her closer to
//! level. She never gives back more in one move than a limit that depends on the opponent's rating, and she never
//! chooses a move that leaves her below level. Everything here is a function of the position, the settings and the
//! seed, and every search it runs stops on depth and nodes, so a game replays the same way on any machine.
//!
//! STANDING choices (mine, named so they can be moved): the band is level to +100 centipawns by Stockfish's count,
//! which is her own score less `EVAL_BIAS` (130); the target when she gives back is between +30 and +70 by seed; the
//! chooser is off for the first ten half-moves and when she has less
//! than twenty seconds on the clock (the caller checks the clock); five alternatives are compared with the best move
//! at one ply less than the search, at most four, with 30,000 nodes each.

use crate::board::{Board, Move};
use crate::movegen::legal_moves;
use crate::search::{Limits, MATE, MATE_BOUND, Report, Searcher};

/// How much more optimistic her evaluation reads than Stockfish's near level, in centipawns. Measured in B15 over
/// 5,300 of her moves from half-move 20 against Stockfish 19 at UCI_Elo 1320 to 1700: a position she scores 100 to 200
/// is level to Stockfish, 0 to 100 is about -100, 200 to 300 about +100. The band and the target below are written in
/// Stockfish's terms and shifted by this, so that "level" means level and not slightly behind. It belongs to her
/// evaluation: measure it again after any change to the graph.
pub const EVAL_BIAS: i32 = 130;
/// Above this score, in centipawns from her side, she starts giving some back (a pawn up by Stockfish's count).
pub const BAND_HI: i32 = 100 + EVAL_BIAS;
/// She never chooses a move that leaves her below this (level by Stockfish's count).
pub const BAND_LO: i32 = EVAL_BIAS;
/// The target when she gives back: this plus a seeded amount up to `TARGET_SPREAD` (+30 to +70 by Stockfish's count).
const TARGET_LO: i32 = 30 + EVAL_BIAS;
const TARGET_SPREAD: i32 = 40;
/// No giving back before this half-move.
pub const OPENING_PLIES: u32 = 10;
/// With less time than this on her clock, in milliseconds, she plays her best.
pub const MIN_CLOCK_MS: u64 = 20_000;
/// The shallow search that sorts the alternatives, and its node limit.
const SCREEN_DEPTH: u32 = 2;
const SCREEN_NODES: u64 = 4_000;
/// The search that compares the best move with the alternatives: at most this deep, with this many nodes each.
const REFINE_DEPTH: u32 = 4;
const REFINE_NODES: u64 = 30_000;
/// How many alternatives get the comparison.
const KEEP: usize = 5;
/// An alternative that screens worse than the best move by more than the limit plus this is not looked at again.
const SCREEN_SLACK: i32 = 60;
/// Scores are clamped to this before a difference is taken, so a mate is a large number and not an overflow.
const CLAMP: i32 = 3000;

/// How much she may give back in one move, in centipawns, to an opponent of this rating. Zero means full strength.
pub fn give_back(rating: i32) -> i32 {
    match rating {
        r if r <= 0 => 100,
        r if r < 1000 => 250,
        r if r < 1200 => 200,
        r if r < 1400 => 160,
        r if r < 1600 => 120,
        r if r < 1800 => 80,
        r if r < 2000 => 50,
        _ => 0,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// The most she gives back in one move, in centipawns.
    pub limit: i32,
    /// Makes the target, and so the choice, differ from game to game and repeat within one.
    pub seed: u64,
}

impl Settings {
    /// The settings for an opponent of this rating, or `None` when she should play her best (2000 and over).
    pub fn for_rating(rating: i32, seed: u64) -> Option<Settings> {
        let limit = give_back(rating);
        (limit > 0).then_some(Settings { limit, seed })
    }
}

/// A move and how much worse it is than the best one, measured by searches of the same depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub mv: Move,
    pub loss: i32,
}

/// What a move sentence and a chat line need: the move she chose, the score she expects after it and the line.
#[derive(Clone, Debug)]
pub struct Choice {
    pub mv: Move,
    /// Her estimated score after the move, in centipawns from her side.
    pub score: i32,
    /// How much worse than her best move it is.
    pub loss: i32,
    /// The move followed by the opponent's best answer line.
    pub pv: Vec<Move>,
    /// The nodes the choosing searched, on top of the search it looked at.
    pub nodes: u64,
}

fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// The score she aims for when she gives back: from `TARGET_LO` to `TARGET_LO + TARGET_SPREAD`.
pub fn target(seed: u64) -> i32 {
    TARGET_LO + (mix(seed) % (TARGET_SPREAD as u64 + 1)) as i32
}

/// The index of the candidate to play. `score` is her full-strength score and each loss is against the best move,
/// whose loss is the smallest. At or under the top of the band, or when no candidate is allowed, the best move.
/// Otherwise the allowed candidate that leaves her closest to the target, where allowed means a loss within the
/// limit and a score after it that is not below level. `salt` (the position) varies the target from move to move.
pub fn pick(score: i32, candidates: &[Candidate], settings: &Settings, salt: u64) -> Option<usize> {
    let best = (0..candidates.len()).min_by_key(|&i| candidates[i].loss)?;
    let here = score.clamp(-CLAMP, CLAMP);
    if here <= BAND_HI {
        return Some(best);
    }
    let aim = target(settings.seed ^ salt);
    let base = candidates[best].loss;
    let mut chosen = best;
    let mut gap = (here - aim).abs();
    for (i, c) in candidates.iter().enumerate() {
        let loss = c.loss - base;
        let after = here - loss;
        if loss > settings.limit || after < BAND_LO {
            continue;
        }
        let g = (after - aim).abs();
        if g < gap || (g == gap && loss < candidates[chosen].loss - base) {
            chosen = i;
            gap = g;
        }
    }
    Some(chosen)
}

/// The score of `mv` from the mover's side, with the line that follows it, by a search that stops on depth and
/// nodes. `None` when the search finished no depth. The nodes it searched are added to `spent`.
fn child_score(
    searcher: &mut Searcher,
    board: &Board,
    mv: Move,
    depth: u32,
    nodes: u64,
    root: u32,
    spent: &mut u64,
) -> Option<(i32, Vec<Move>)> {
    let mut after = board.clone();
    after.make(mv);
    if legal_moves(&mut after).is_empty() {
        // The move ended the game: mate for the mover, or a draw.
        return Some((if after.in_check() { MATE - 1 } else { 0 }, Vec::new()));
    }
    searcher.root_ply = root + 1;
    searcher.history.push(after.hash());
    let limits = Limits {
        depth: Some(depth),
        nodes: Some(nodes),
        ..Default::default()
    };
    let r = searcher.search(&mut after, &limits, |_| {});
    searcher.history.pop();
    *spent += r.nodes;
    (r.depth > 0).then(|| (-r.score, r.pv))
}

/// Looks at the search's result for `board` and returns a move to play instead of its best, or `None` to play the
/// best. `searcher` is the one that produced `report`; its root ply and history are left as they were.
pub fn choose(
    searcher: &mut Searcher,
    board: &Board,
    report: &Report,
    settings: &Settings,
) -> Option<Choice> {
    let best = report.best()?;
    let root = searcher.root_ply;
    if root < OPENING_PLIES
        || report.depth < 3
        || report.score >= MATE_BOUND
        || report.score.clamp(-CLAMP, CLAMP) <= BAND_HI
    {
        return None;
    }
    let history_len = searcher.history.len();
    let chosen = pick_from_search(searcher, board, report, best, settings, root);
    searcher.history.truncate(history_len);
    searcher.root_ply = root;
    chosen
}

fn pick_from_search(
    searcher: &mut Searcher,
    board: &Board,
    report: &Report,
    best: Move,
    settings: &Settings,
    root: u32,
) -> Option<Choice> {
    let mut work = board.clone();
    let moves = legal_moves(&mut work);
    let mut spent = 0u64;

    // Sort the other moves with a shallow search, and keep the few that could be close enough to the best.
    let mut screened: Vec<(Move, i32)> = Vec::new();
    let mut reference = None;
    for &m in &moves {
        let (score, _) = child_score(
            searcher,
            board,
            m,
            SCREEN_DEPTH,
            SCREEN_NODES,
            root,
            &mut spent,
        )?;
        if m == best {
            reference = Some(score);
        } else {
            screened.push((m, score));
        }
    }
    let reference = reference?;
    let cutoff = reference.clamp(-CLAMP, CLAMP) - settings.limit - SCREEN_SLACK;
    screened.retain(|&(_, s)| s.clamp(-CLAMP, CLAMP) >= cutoff);
    // Highest first; the move's own text breaks a tie, so the order does not depend on how the moves were generated.
    screened.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.to_string().cmp(&b.0.to_string()))
    });
    screened.truncate(KEEP);
    if screened.is_empty() {
        return None;
    }

    // Compare the best move with each of them by searches of the same depth.
    let depth = report.depth.saturating_sub(1).clamp(1, REFINE_DEPTH);
    let (ref_score, _) = child_score(searcher, board, best, depth, REFINE_NODES, root, &mut spent)?;
    let ref_score = ref_score.clamp(-CLAMP, CLAMP);
    let mut candidates = vec![Candidate { mv: best, loss: 0 }];
    let mut lines: Vec<Vec<Move>> = vec![Vec::new()];
    for &(m, _) in &screened {
        let (score, line) = child_score(searcher, board, m, depth, REFINE_NODES, root, &mut spent)?;
        let loss = (ref_score - score.clamp(-CLAMP, CLAMP)).max(0);
        candidates.push(Candidate { mv: m, loss });
        lines.push(line);
    }

    let index = pick(report.score, &candidates, settings, board.hash())?;
    if candidates[index].mv == best {
        return None;
    }
    let c = candidates[index];
    let mut pv = vec![c.mv];
    pv.extend(lines[index].iter().copied());
    Some(Choice {
        mv: c.mv,
        score: report.score.clamp(-CLAMP, CLAMP) - c.loss,
        loss: c.loss,
        pv,
        nodes: spent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{DEFAULT_GRAPH, Graph};

    fn settings(limit: i32) -> Settings {
        Settings { limit, seed: 7 }
    }

    /// Candidates with these losses, on moves that are only there to be told apart.
    fn losses(list: &[i32]) -> Vec<Candidate> {
        let mut b = Board::start();
        let moves = legal_moves(&mut b);
        list.iter()
            .enumerate()
            .map(|(i, &loss)| Candidate { mv: moves[i], loss })
            .collect()
    }

    #[test]
    fn the_limit_follows_the_rating() {
        let table = [
            (0, 100),
            (-1, 100),
            (600, 250),
            (999, 250),
            (1000, 200),
            (1199, 200),
            (1200, 160),
            (1399, 160),
            (1400, 120),
            (1599, 120),
            (1600, 80),
            (1799, 80),
            (1800, 50),
            (1999, 50),
            (2000, 0),
            (2800, 0),
        ];
        for (rating, limit) in table {
            assert_eq!(give_back(rating), limit, "rating {}", rating);
        }
        assert_eq!(Settings::for_rating(2000, 1), None);
        assert_eq!(
            Settings::for_rating(1500, 1),
            Some(Settings {
                limit: 120,
                seed: 1
            })
        );
    }

    #[test]
    fn the_target_stays_in_its_range_and_varies() {
        let mut seen = std::collections::HashSet::new();
        for seed in 0..2000u64 {
            let t = target(seed);
            assert!(
                (TARGET_LO..=TARGET_LO + TARGET_SPREAD).contains(&t),
                "{}",
                t
            );
            seen.insert(t);
        }
        assert!(seen.len() >= 30, "only {} different targets", seen.len());
        assert_eq!(target(5), target(5));
    }

    #[test]
    fn in_the_band_or_behind_she_plays_her_best() {
        let c = losses(&[0, 40, 90]);
        for score in [-900, -50, 0, 60, 100, BAND_LO, BAND_HI] {
            assert_eq!(pick(score, &c, &settings(250), 1), Some(0), "{}", score);
        }
        // The best move is the one with the smallest loss, wherever it is in the list.
        let c = losses(&[90, 0, 40]);
        assert_eq!(pick(50, &c, &settings(250), 1), Some(1));
        assert_eq!(pick(50, &[], &settings(250), 1), None);
    }

    #[test]
    fn a_large_lead_is_given_back_up_to_the_limit_and_no_more() {
        let c = losses(&[0, 60, 119, 120, 121, 200]);
        // 400 is far above any target, so the move that gives back the most within the limit wins.
        assert_eq!(pick(400, &c, &settings(120), 1), Some(3));
        assert_eq!(pick(400, &c, &settings(119), 1), Some(2));
        assert_eq!(pick(400, &c, &settings(59), 1), Some(0));
    }

    #[test]
    fn she_does_not_choose_to_fall_behind() {
        // 50 over the band. A loss of 160 would leave her 20 below the bottom of the band, so it is not allowed even
        // with the limit.
        let top = BAND_HI + 50;
        let c = losses(&[0, 100, 160]);
        for salt in 0..200u64 {
            let i = pick(top, &c, &settings(250), salt).unwrap();
            assert_ne!(i, 2, "salt {}", salt);
        }
        // A loss of 100 leaves her 20 above the bottom of the band, which is nearer every target than the start.
        assert_eq!(
            pick(top, &c, &settings(250), 3).map(|i| c[i].loss),
            Some(100)
        );
    }

    #[test]
    fn the_choice_is_the_same_every_time_and_always_allowed() {
        let c = losses(&[0, 15, 35, 70, 95, 140, 230, 400]);
        for limit in [50, 80, 120, 160, 200, 250] {
            for score in [BAND_HI + 1, BAND_HI + 30, 400, 800, 2900] {
                for salt in 0..64u64 {
                    let s = settings(limit);
                    let a = pick(score, &c, &s, salt).unwrap();
                    assert_eq!(Some(a), pick(score, &c, &s, salt));
                    assert!(c[a].loss <= limit, "limit {} loss {}", limit, c[a].loss);
                    assert!(score.clamp(-CLAMP, CLAMP) - c[a].loss >= BAND_LO);
                }
            }
        }
    }

    fn searcher_at(root: u32, board: &Board) -> Searcher {
        let mut s = Searcher::new(Graph::from_json(DEFAULT_GRAPH).unwrap());
        s.root_ply = root;
        s.history = vec![board.hash()];
        s
    }

    fn full_strength(searcher: &mut Searcher, board: &Board, depth: u32) -> Report {
        let limits = Limits {
            depth: Some(depth),
            ..Default::default()
        };
        searcher.search(&mut board.clone(), &limits, |_| {})
    }

    #[test]
    fn she_gives_back_when_far_ahead_and_only_then() {
        // White is a queen up.
        let ahead = Board::from_fen("4k3/pppp1ppp/8/8/8/8/PPPP1PPP/3QK3 w - - 0 1").unwrap();
        let level = Board::start();
        let s = settings(250);

        let mut searcher = searcher_at(20, &ahead);
        let report = full_strength(&mut searcher, &ahead, 4);
        assert!(report.score > BAND_HI, "score {}", report.score);
        let best = report.best().unwrap();
        let choice =
            choose(&mut searcher, &ahead, &report, &s).expect("far ahead, so she gives back");
        assert_ne!(choice.mv, best);
        assert!(
            choice.loss > 0 && choice.loss <= s.limit,
            "loss {}",
            choice.loss
        );
        assert!(choice.score >= BAND_LO);
        assert_eq!(choice.pv[0], choice.mv);
        assert!(legal_moves(&mut ahead.clone()).contains(&choice.mv));
        // The search is as it was left.
        assert_eq!(searcher.root_ply, 20);
        assert_eq!(searcher.history, vec![ahead.hash()]);
        // The same call gives the same answer.
        let again = choose(&mut searcher, &ahead, &report, &s).unwrap();
        assert_eq!((again.mv, again.loss), (choice.mv, choice.loss));

        // Before half-move 10 she plays her best.
        let mut early = searcher_at(4, &ahead);
        let r = full_strength(&mut early, &ahead, 4);
        assert!(choose(&mut early, &ahead, &r, &s).is_none());

        // Level, she plays her best.
        let mut even = searcher_at(20, &level);
        let r = full_strength(&mut even, &level, 4);
        assert!(choose(&mut even, &level, &r, &s).is_none());
    }

    #[test]
    fn she_takes_a_mate_she_sees() {
        // White mates in one with the queen.
        let board = Board::from_fen("6k1/5ppp/8/8/8/8/5PPP/3Q2K1 w - - 0 1").unwrap();
        let mut searcher = searcher_at(30, &board);
        let report = full_strength(&mut searcher, &board, 4);
        assert!(report.score >= MATE_BOUND, "score {}", report.score);
        assert!(choose(&mut searcher, &board, &report, &settings(250)).is_none());
    }
}
