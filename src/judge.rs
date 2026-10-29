//! The judge: how good was the student's move, by Symbia's own search (B13).
//!
//! Two jobs on two clocks. `ponder` runs while the student thinks: it searches the position he must answer and
//! keeps the best move and score for every depth that finished. `judge` runs when his move arrives: if it is the
//! best move there is nothing more to do; otherwise it searches the position after his move one ply shallower and
//! compares the two scores. The difference, in centipawns, is what the move cost.
//!
//! The verdict is a function of the position, the move, the depth and the node limit, and of nothing else. Both
//! searches stop on nodes and depth, never on the clock, and a ponder that was cut short is discarded, so the words
//! do not depend on how long the student thought or how busy the machine was. The ponder only saves time.

use crate::board::{Board, Move, MoveKind, Piece, square_name};
use crate::graph::{DEFAULT_GRAPH, Graph};
use crate::movegen::legal_moves;
use crate::narrate::{move_words, name as piece_name};
use crate::search::{Limits, MATE, MATE_BOUND, Searcher};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// How deep and how many nodes each of the judge's searches may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub depth: u32,
    pub nodes: u64,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            depth: 6,
            nodes: 500_000,
        }
    }
}

/// Scores are clamped to this before they are subtracted, so winning a little more or less in a won position
/// costs nothing.
const CLAMP: i32 = 1000;

/// A move this close to the best one is as good as it.
const SAME_AS_BEST: i32 = 10;

/// Where each class starts, in centipawns of cost, largest first. Under the last the move is Good.
const FLOORS: [(i32, Class); 3] = [
    (300, Class::Blunder),
    (120, Class::Mistake),
    (50, Class::Inaccuracy),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    Best,
    Good,
    Inaccuracy,
    Mistake,
    Blunder,
}

impl Class {
    pub fn word(self) -> &'static str {
        match self {
            Class::Best => "Best",
            Class::Good => "Good",
            Class::Inaccuracy => "Inaccuracy",
            Class::Mistake => "Mistake",
            Class::Blunder => "Blunder",
        }
    }

    /// The class as a noun phrase, for "You made a mistake".
    pub fn noun(self) -> &'static str {
        match self {
            Class::Best => "the best move",
            Class::Good => "a good move",
            Class::Inaccuracy => "an inaccuracy",
            Class::Mistake => "a mistake",
            Class::Blunder => "a blunder",
        }
    }
}

/// The class for a move that cost `cost` centipawns.
pub fn classify(cost: i32) -> Class {
    if cost <= SAME_AS_BEST {
        Class::Best
    } else {
        FLOORS
            .iter()
            .find(|(from, _)| cost >= *from)
            .map_or(Class::Good, |(_, class)| *class)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub class: Class,
    /// What the move cost against the best one, in centipawns, after clamping. Never negative.
    pub cost: i32,
    pub played: Move,
    pub best: Move,
    /// Both scores are from the student's side, as the search produced them.
    pub best_score: i32,
    pub played_score: i32,
    /// The depth of the search that found the best move.
    pub depth: u32,
    /// The best answer to the move played, when it was searched.
    pub reply: Option<Move>,
}

/// One finished depth of a search.
#[derive(Clone, Debug)]
struct Step {
    depth: u32,
    score: i32,
    best: Move,
}

/// The position and the number of half-moves played to reach it. The en passant square is left out: it cannot
/// change the verdict, and two sources write it differently.
type Key = (String, u32);

fn key_of(board: &Board, root: u32) -> Key {
    let fen = board.fen();
    let head: Vec<&str> = fen.split(' ').take(3).collect();
    (head.join(" "), root)
}

fn limits(depth: u32, nodes: u64) -> Limits {
    Limits {
        depth: Some(depth),
        nodes: Some(nodes),
        ..Default::default()
    }
}

pub struct Judge {
    settings: Settings,
    interrupt: Arc<AtomicBool>,
    /// Searches while the student thinks. Its stop flag is the interrupt.
    ponderer: Searcher,
    /// Searches when the move arrives. Nothing stops it.
    own: Searcher,
    ponder: Option<(Key, Vec<Step>)>,
}

impl Judge {
    /// `interrupt` is raised when the student has moved or the game went back; it stops the ponder.
    pub fn new(settings: Settings, interrupt: Arc<AtomicBool>) -> Result<Judge, String> {
        let mut ponderer = Searcher::new(Graph::from_json(DEFAULT_GRAPH)?);
        ponderer.stop = interrupt.clone();
        Ok(Judge {
            settings,
            interrupt,
            ponderer,
            own: Searcher::new(Graph::from_json(DEFAULT_GRAPH)?),
            ponder: None,
        })
    }

    /// Every finished depth, and whether the search ran to its limits rather than being cut short.
    fn steps(
        searcher: &mut Searcher,
        board: &Board,
        root: u32,
        settings: Settings,
    ) -> (Vec<Step>, bool) {
        searcher.root_ply = root;
        let mut b = board.clone();
        let mut steps = Vec::new();
        let report = searcher.search(&mut b, &limits(settings.depth, settings.nodes), |r| {
            if let Some(best) = r.best() {
                steps.push(Step {
                    depth: r.depth,
                    score: r.score,
                    best,
                });
            }
        });
        // A search ends on its depth, on its nodes, or on a mate, and all three are the same every time. Any
        // other end is the interrupt.
        let whole = report.depth >= settings.depth
            || report.nodes >= settings.nodes
            || report.score.abs() >= MATE_BOUND;
        (steps, whole)
    }

    /// Search the position the student has to answer. `root` is the number of half-moves played to reach it.
    pub fn ponder(&mut self, after: &Board, root: u32) {
        self.ponder = None;
        if self.interrupt.load(Ordering::Relaxed) {
            return;
        }
        let (steps, whole) = Self::steps(&mut self.ponderer, after, root, self.settings);
        if whole && !steps.is_empty() {
            self.ponder = Some((key_of(after, root), steps));
        }
    }

    /// Whether a finished ponder is waiting.
    pub fn has_ponder(&self) -> bool {
        self.ponder.is_some()
    }

    /// How good `played` was from `before`. `ply` is the student's half-move number.
    pub fn judge(&mut self, before: &Board, played: Move, ply: u32) -> Result<Verdict, String> {
        let root = ply.saturating_sub(1);
        let key = key_of(before, root);
        let steps = match &self.ponder {
            Some((k, steps)) if *k == key => steps.clone(),
            _ => Self::steps(&mut self.own, before, root, self.settings).0,
        };
        let last = steps.last().ok_or("the search finished no depth")?.clone();
        let (best, best_score, depth) = (last.best, last.score, last.depth);
        if played == best {
            return Ok(Verdict {
                class: Class::Best,
                cost: 0,
                played,
                best,
                best_score,
                played_score: best_score,
                depth,
                reply: None,
            });
        }

        let mut after = before.clone();
        after.make(played);
        let (played_score, reply, reference) = if legal_moves(&mut after).is_empty() {
            // The move ended the game: mate for the mover, or a draw.
            (
                if after.in_check() { MATE - 1 } else { 0 },
                None,
                best_score,
            )
        } else {
            // A mate is exact at any depth, so a mate found at depth 1 (B18: the search now sees a mate on its last
            // ply) is compared with a one-ply search after the move. Anything else needs two plies.
            if depth < 2 && best_score.abs() < MATE_BOUND {
                return Err("the search did not get past one ply".to_string());
            }
            self.own.root_ply = root + 1;
            let r = self.own.search(
                &mut after.clone(),
                &limits((depth - 1).max(1), self.settings.nodes),
                |_| {},
            );
            if r.depth == 0 {
                return Err("the second search finished no depth".to_string());
            }
            // Compare scores from searches of the same depth. A mate is exact whatever the depth it was found at.
            let reference = if r.depth + 1 >= depth || r.score.abs() >= MATE_BOUND {
                best_score
            } else {
                steps
                    .iter()
                    .find(|s| s.depth == r.depth + 1)
                    .map_or(best_score, |s| s.score)
            };
            (-r.score, r.best(), reference)
        };

        let cost = (reference.clamp(-CLAMP, CLAMP) - played_score.clamp(-CLAMP, CLAMP)).max(0);
        Ok(Verdict {
            class: classify(cost),
            cost,
            played,
            best,
            best_score,
            played_score,
            depth,
            reply,
        })
    }
}

/// What a cost means in pawns, rounded the way people say it.
fn cost_clause(cost: i32) -> String {
    if cost >= 1000 {
        "costs the game".to_string()
    } else if cost < 75 {
        "costs about half a pawn".to_string()
    } else if cost < 125 {
        "costs about a pawn".to_string()
    } else {
        match (cost + 50) / 100 {
            0 | 1 => "costs about a pawn".to_string(),
            n => format!("costs about {} pawns", n),
        }
    }
}

/// What Symbia would have played, said the way a person would: "I would play knight to f3." or "I would castle
/// kingside."
fn would_play(words: &str) -> String {
    match words.strip_prefix("castling ") {
        Some(side) => format!("I would castle {}.", side),
        None => format!("I would play {}.", words),
    }
}

/// "I can take your bishop on d5." when the best answer to the move takes one of the student's pieces.
fn take_clause(before: &Board, v: &Verdict) -> Option<String> {
    let reply = v.reply?;
    if reply.kind != MoveKind::Capture {
        return None;
    }
    let mut after = before.clone();
    after.make(v.played);
    let (color, piece) = after.piece_on(reply.to)?;
    if color != before.turn {
        return None;
    }
    Some(format!(
        "I can take your {} on {}.",
        piece_name(piece),
        square_name(reply.to)
    ))
}

/// The pieces a sentence says are on squares, for a test to check: (square, piece) for the student's pieces.
pub fn takes_named(before: &Board, v: &Verdict) -> Option<(String, Piece)> {
    let reply = v.reply?;
    take_clause(before, v)?;
    let mut after = before.clone();
    after.make(v.played);
    let (_, piece) = after.piece_on(reply.to)?;
    Some((square_name(reply.to), piece))
}

/// The sentence about the verdict, the longest of its forms that fits `max` characters. "I" is Symbia, "you" the
/// student, as in the Game lane.
pub fn sentence(before: &Board, v: &Verdict, max: usize) -> String {
    fit(&candidates(before, v, None), max)
}

/// The quiet form of `sentence`: nothing for a move that was best or good, and for the rest the same sentence with
/// `reason` (what the move did to the student's position, found by `why`) ahead of the move she would play.
pub fn sentence_quiet(
    before: &Board,
    v: &Verdict,
    reason: Option<&str>,
    max: usize,
) -> Option<String> {
    if v.class <= Class::Good {
        return None;
    }
    Some(fit(&candidates(before, v, reason), max))
}

/// The first of the forms that fits `max` characters, longest first; the last one cut if none does.
fn fit(candidates: &[String], max: usize) -> String {
    match candidates.iter().find(|c| c.chars().count() <= max) {
        Some(text) => text.clone(),
        None => candidates
            .last()
            .map(|c| c.chars().take(max).collect())
            .unwrap_or_default(),
    }
}

/// The forms of the sentence, longest first. With no reason these are the B15 forms exactly.
fn candidates(before: &Board, v: &Verdict, reason: Option<&str>) -> Vec<String> {
    let best = move_words(before, v.best);
    let better = would_play(&best);
    let kind = v.class.noun();
    match v.class {
        Class::Best if v.played == v.best => {
            vec![
                "You played the move I would play.".to_string(),
                "You played my move.".to_string(),
            ]
        }
        Class::Best => vec![
            format!("You played nearly as well as my choice, {}.", best),
            "You played nearly as well as my choice.".to_string(),
        ],
        Class::Good => vec![
            format!("You played a good move. I like {} a little more.", best),
            "You played a good move.".to_string(),
        ],
        _ if v.played_score <= -MATE_BOUND => {
            let k = (MATE + v.played_score + 1) / 2;
            vec![
                format!("You allowed mate in {}. {}", k, better),
                format!("You allowed mate in {}.", k),
            ]
        }
        _ if v.best_score >= MATE_BOUND && v.played_score < MATE_BOUND => {
            let k = (MATE - v.best_score + 1) / 2;
            vec![
                format!("You missed mate in {} with {}.", k, best),
                format!("You missed mate in {}.", k),
            ]
        }
        _ => {
            let made = format!("You made {} that {}", kind, cost_clause(v.cost));
            let mut forms = Vec::new();
            let take = if v.class >= Class::Mistake {
                take_clause(before, v)
            } else {
                None
            };
            if let (Some(take), Some(why)) = (&take, reason) {
                forms.push(format!("{}. {} {} {}", made, take, why, better));
            }
            if let Some(take) = &take {
                forms.push(format!("{}. {} {}", made, take, better));
            }
            if let Some(why) = reason {
                forms.push(format!("{}. {} {}", made, why, better));
            }
            forms.push(format!("{}. {}", made, better));
            forms.push(format!("You made {}. {}", kind, better));
            forms.push(format!("You made {}.", kind));
            forms
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board_and_move(fen: &str, uci: &str) -> (Board, Move) {
        let mut b = Board::from_fen(fen).unwrap();
        let m = legal_moves(&mut b)
            .into_iter()
            .find(|m| m.to_string() == uci)
            .unwrap_or_else(|| panic!("{} is not legal in {}", uci, fen));
        (b, m)
    }

    fn judge_move(fen: &str, uci: &str, ply: u32) -> (Board, Verdict) {
        let (b, m) = board_and_move(fen, uci);
        let mut j = Judge::new(Settings::default(), Arc::new(AtomicBool::new(false))).unwrap();
        let v = j.judge(&b, m, ply).unwrap();
        (b, v)
    }

    const START: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";
    const QUEENS: &str = "4k3/8/8/3q4/8/8/8/3QK3 w - - 0 1";
    const BACK_RANK: &str = "6k1/5ppp/8/8/8/8/8/R5K1 w - - 0 1";
    const SCHOLAR: &str = "r1bqkbnr/pppp1ppp/2n5/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR b KQkq - 3 3";

    #[test]
    fn the_classes_start_where_the_table_says() {
        assert_eq!(classify(0), Class::Best);
        assert_eq!(classify(10), Class::Best);
        assert_eq!(classify(11), Class::Good);
        assert_eq!(classify(49), Class::Good);
        assert_eq!(classify(50), Class::Inaccuracy);
        assert_eq!(classify(119), Class::Inaccuracy);
        assert_eq!(classify(120), Class::Mistake);
        assert_eq!(classify(299), Class::Mistake);
        assert_eq!(classify(300), Class::Blunder);
        assert_eq!(classify(2000), Class::Blunder);
    }

    #[test]
    fn a_cost_is_never_said_as_one_pawns() {
        for cost in 0..2500 {
            let clause = cost_clause(cost);
            assert!(!clause.contains(" 1 pawns"), "{} -> {}", cost, clause);
            assert!(!clause.contains(" 0 pawns"), "{} -> {}", cost, clause);
        }
        assert_eq!(cost_clause(140), "costs about a pawn");
        assert_eq!(cost_clause(260), "costs about 3 pawns");
        assert_eq!(cost_clause(1000), "costs the game");
    }

    #[test]
    fn what_i_would_play_reads_like_speech() {
        assert_eq!(would_play("knight to f3"), "I would play knight to f3.");
        assert_eq!(would_play("castling kingside"), "I would castle kingside.");
        assert_eq!(
            would_play("castling queenside"),
            "I would castle queenside."
        );
    }

    #[test]
    fn taking_a_free_queen_is_best() {
        let (b, v) = judge_move(QUEENS, "d1d5", 1);
        assert_eq!(v.class, Class::Best, "{:?}", v);
        assert_eq!(v.cost, 0);
        assert_eq!(v.played, v.best);
        assert_eq!(sentence(&b, &v, 140), "You played the move I would play.");
    }

    #[test]
    fn missing_a_mate_in_one_is_a_blunder_that_says_so() {
        let (b, v) = judge_move(BACK_RANK, "a1a2", 1);
        assert_eq!(v.class, Class::Blunder, "{:?}", v);
        assert_eq!(v.best.to_string(), "a1a8");
        let text = sentence(&b, &v, 140);
        assert!(
            text.contains("You missed mate in 1 with rook to a8"),
            "{}",
            text
        );
    }

    #[test]
    fn allowing_the_scholars_mate_is_a_blunder_that_names_it() {
        let (b, v) = judge_move(SCHOLAR, "g8f6", 6);
        assert_eq!(v.class, Class::Blunder, "{:?}", v);
        let text = sentence(&b, &v, 140);
        assert!(text.contains("You allowed mate in 1"), "{}", text);
        assert_eq!(v.reply.map(|m| m.to_string()).as_deref(), Some("h5f7"));
    }

    #[test]
    fn leaving_a_queen_to_be_taken_says_so() {
        let (b, v) = judge_move(QUEENS, "d1d4", 1);
        assert_eq!(v.class, Class::Blunder, "{:?}", v);
        let text = sentence(&b, &v, 140);
        assert!(text.contains("I can take your queen on d4."), "{}", text);
        assert_eq!(takes_named(&b, &v), Some(("d4".to_string(), Piece::Queen)));
    }

    #[test]
    fn the_engines_own_choice_is_best_and_costs_nothing() {
        let (b, first) = judge_move(START, "e2e4", 1);
        let (_, own) = judge_move(START, &first.best.to_string(), 1);
        assert_eq!(own.class, Class::Best);
        assert_eq!(own.cost, 0);
        assert_eq!(own.played_score, own.best_score);
        assert!(!sentence(&b, &own, 140).is_empty());
    }

    /// The five quiet moves of prediction 1, labeled before the code was written.
    #[test]
    fn five_quiet_moves_from_the_start_read_as_labeled() {
        let cost = |uci: &str| judge_move(START, uci, 1).1;
        let (e4, d4, a3, f3, g4) = (
            cost("e2e4"),
            cost("d2d4"),
            cost("a2a3"),
            cost("f2f3"),
            cost("g2g4"),
        );
        for v in [&e4, &d4, &a3, &f3, &g4] {
            println!("{} cost {} {:?}", v.played, v.cost, v.class);
        }
        assert!(e4.class <= Class::Good, "{:?}", e4);
        assert!(d4.class <= Class::Good, "{:?}", d4);
        assert!(a3.class <= Class::Inaccuracy, "{:?}", a3);
        assert!(f3.cost > e4.cost, "f3 {:?} e4 {:?}", f3, e4);
        assert!(g4.cost > a3.cost, "g4 {:?} a3 {:?}", g4, a3);
    }

    #[test]
    fn a_finished_ponder_is_used_and_gives_the_same_verdict() {
        let (b, m) = board_and_move(START, "a2a3");
        let mut cold = Judge::new(Settings::default(), Arc::new(AtomicBool::new(false))).unwrap();
        let cold_verdict = cold.judge(&b, m, 1).unwrap();

        let mut warm = Judge::new(Settings::default(), Arc::new(AtomicBool::new(false))).unwrap();
        warm.ponder(&b, 0);
        assert!(warm.has_ponder());
        let warm_verdict = warm.judge(&b, m, 1).unwrap();
        assert_eq!(cold_verdict, warm_verdict);
    }

    #[test]
    fn a_ponder_for_another_position_is_not_used() {
        let (b, m) = board_and_move(START, "e2e4");
        let mut j = Judge::new(Settings::default(), Arc::new(AtomicBool::new(false))).unwrap();
        j.ponder(&Board::from_fen(QUEENS).unwrap(), 0);
        assert!(j.has_ponder());
        let mut cold = Judge::new(Settings::default(), Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!(j.judge(&b, m, 1).unwrap(), cold.judge(&b, m, 1).unwrap());
    }

    #[test]
    fn the_ponder_stops_when_the_student_moves_and_is_thrown_away() {
        let mid = Board::from_fen(
            "r1bq1rk1/ppp2ppp/2np1n2/2b1p3/2B1P3/2NP1N2/PPP2PPP/R1BQ1RK1 w - - 0 7",
        )
        .unwrap();
        let flag = Arc::new(AtomicBool::new(false));
        let mut j = Judge::new(
            Settings {
                depth: 40,
                nodes: 50_000_000,
            },
            flag.clone(),
        )
        .unwrap();
        let mut slowest = std::time::Duration::ZERO;
        for _ in 0..20 {
            flag.store(false, Ordering::Relaxed);
            let raiser = {
                let flag = flag.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(30));
                    flag.store(true, Ordering::Relaxed);
                    std::time::Instant::now()
                })
            };
            j.ponder(&mid, 12);
            let back = std::time::Instant::now();
            let raised = raiser.join().unwrap();
            let lag = back.saturating_duration_since(raised);
            slowest = slowest.max(lag);
            assert!(!j.has_ponder(), "a cut-off ponder was kept");
        }
        println!("slowest stop of 20: {:?}", slowest);
        // The release build stops in about a millisecond (B13, prediction 3). This test runs in the debug build beside
        // the rest of the suite, where one stop took 9 ms alone and 33 ms under load; a ponder that ignored the flag
        // would run for seconds, so 100 ms still tells the two apart.
        assert!(
            slowest < std::time::Duration::from_millis(100),
            "{:?}",
            slowest
        );
    }

    #[test]
    fn a_ponder_that_starts_after_the_student_moved_does_nothing() {
        let flag = Arc::new(AtomicBool::new(true));
        let mut j = Judge::new(Settings::default(), flag).unwrap();
        j.ponder(&Board::start(), 0);
        assert!(!j.has_ponder());
    }

    #[test]
    fn a_move_that_ends_the_game_is_judged_without_a_second_search() {
        // Mate in one against the side that cannot stop it: the winning move is the best move.
        let (b, v) = judge_move(BACK_RANK, "a1a8", 1);
        assert_eq!(v.class, Class::Best);
        // A stalemate that throws away a win costs everything.
        let (b2, v2) = judge_move("7k/8/6K1/5Q2/8/8/8/8 w - - 0 1", "f5f7", 1);
        assert!(v2.class >= Class::Mistake, "{:?}", v2);
        assert!(!sentence(&b2, &v2, 140).is_empty());
        assert!(!sentence(&b, &v, 140).is_empty());
    }

    #[test]
    fn every_sentence_fits_every_room_and_names_real_pieces() {
        for (fen, uci, ply) in [
            (QUEENS, "d1d4", 1),
            (QUEENS, "d1d5", 1),
            (BACK_RANK, "a1a2", 1),
            (SCHOLAR, "g8f6", 6),
            (START, "g2g4", 1),
            (START, "e2e4", 1),
        ] {
            let (b, v) = judge_move(fen, uci, ply);
            for room in [20, 40, 80, 140] {
                let text = sentence(&b, &v, room);
                assert!(text.chars().count() <= room, "{} in {}", text, room);
                assert!(!text.is_empty());
            }
            if let Some((square, piece)) = takes_named(&b, &v) {
                let mut after = b.clone();
                after.make(v.played);
                let at = crate::board::parse_square(&square).unwrap();
                assert_eq!(after.piece_on(at).map(|(_, p)| p), Some(piece));
            }
        }
    }
}
