//! The coach: three lanes that talk about a game, each on its own thread, and one outbox that speaks for all of them.
//!
//! A hook in the bot sends one event per move. The coach copies what each lane needs, puts it on that lane's
//! queue and returns, so the hook never waits on a lane and a lane never touches a live board. The lanes are
//!
//! * `Game`: what the student's move did. Fed by `student` events.
//! * `You`: how good the student's move was, by the engine's own search (`judge`). Fed by `student` events, and by
//!   `tutor` events, which it uses to search the student's position while he thinks. A `student` event stops that
//!   search, so it never runs beside the tutor's own.
//! * `Me`: what the tutor's own move did and where the game stands. Fed by `tutor` events.
//!
//! Every lane hands its finished line to the outbox, the only writer, so two lines are never mixed. The outbox
//! drops a line that has fallen more than one ply behind the newest event, and a line for a ply that a takeback
//! removed, and says so with a `drop` line instead.
//!
//! Protocol, one line each way. In (the position is the one before the move, as a FEN):
//!
//! ```text
//! student <ply> <uci-move> <fen>
//! tutor <ply> <uci-move> <depth> <cp|mate> <value> <fen>
//! takeback <ply>          the first ply that no longer stands
//! quit                    finish the lines still waiting, then end
//! ```
//!
//! Out:
//!
//! ```text
//! say <ply> <game|you|me> <text>     the text fits the chat limit; it has no tag (B14), the words say who it is about
//! drop <ply> <game|you|me> <why>
//! note <text>
//! ```

use crate::board::{Board, Move};
use crate::judge::{Judge, Settings, sentence, sentence_quiet};
use crate::movegen::legal_moves;
use crate::narrate::{MAX_CHARS, say_theirs_quiet_within, say_theirs_within, say_within};
use crate::search::{Report, score_from_uci};
use crate::why::{Standing, Terms, cooling, me_quiet, you_reason};
use std::io::{BufRead, Write};
use std::str::SplitWhitespace;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    Game,
    You,
    Me,
}

impl Lane {
    pub const ALL: [Lane; 3] = [Lane::Game, Lane::You, Lane::Me];

    /// The word the protocol uses.
    pub fn word(self) -> &'static str {
        match self {
            Lane::Game => "game",
            Lane::You => "you",
            Lane::Me => "me",
        }
    }

    /// How many characters a line of this lane may have. The lines carry no label: each says in its first words who
    /// it is about ("You moved", "I took").
    pub fn room(self) -> usize {
        MAX_CHARS
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The student played `mv` from `fen`, as half-move `ply`.
    Student { ply: u32, mv: String, fen: String },
    /// The tutor chose `mv` from `fen`, as half-move `ply`, after a search that reached `depth` with `score`.
    Tutor {
        ply: u32,
        mv: String,
        fen: String,
        depth: u32,
        score: i32,
    },
    /// The game went back: `ply` and every ply after it no longer stand.
    Takeback { ply: u32 },
}

impl Event {
    pub fn ply(&self) -> u32 {
        match self {
            Event::Student { ply, .. } | Event::Tutor { ply, .. } | Event::Takeback { ply } => *ply,
        }
    }
}

fn next_number(words: &mut SplitWhitespace, what: &str) -> Result<i32, String> {
    let word = words.next().ok_or_else(|| format!("missing {}", what))?;
    word.parse::<i32>()
        .map_err(|_| format!("{} is not a number: {}", what, word))
}

fn next_ply(words: &mut SplitWhitespace) -> Result<u32, String> {
    let ply = next_number(words, "ply")?;
    if ply < 1 {
        return Err(format!("ply must be 1 or more: {}", ply));
    }
    Ok(ply as u32)
}

fn rest_as_fen(words: SplitWhitespace) -> Result<String, String> {
    let fen = words.collect::<Vec<_>>().join(" ");
    if fen.is_empty() {
        Err("missing fen".to_string())
    } else {
        Ok(fen)
    }
}

/// One input line as an event.
pub fn parse(line: &str) -> Result<Event, String> {
    let mut words = line.split_whitespace();
    match words.next() {
        None => Err("empty line".to_string()),
        Some("student") => {
            let ply = next_ply(&mut words)?;
            let mv = words.next().ok_or("missing move")?.to_string();
            Ok(Event::Student {
                ply,
                mv,
                fen: rest_as_fen(words)?,
            })
        }
        Some("tutor") => {
            let ply = next_ply(&mut words)?;
            let mv = words.next().ok_or("missing move")?.to_string();
            let depth = next_number(&mut words, "depth")?;
            let kind = words.next().ok_or("missing score kind")?;
            let value = next_number(&mut words, "score")?;
            let score = score_from_uci(kind, value)
                .ok_or_else(|| format!("not a score: {} {}", kind, value))?;
            Ok(Event::Tutor {
                ply,
                mv,
                fen: rest_as_fen(words)?,
                depth: depth.max(0) as u32,
                score,
            })
        }
        Some("takeback") => Ok(Event::Takeback {
            ply: next_ply(&mut words)?,
        }),
        Some(other) => Err(format!("unknown event: {}", other)),
    }
}

fn played(fen: &str, mv: &str) -> Result<(Board, Move), String> {
    let mut board = Board::from_fen(fen).map_err(|e| format!("bad fen: {}", e))?;
    let m = legal_moves(&mut board)
        .into_iter()
        .find(|m| m.to_string() == mv)
        .ok_or_else(|| format!("illegal move {}", mv))?;
    Ok((board, m))
}

/// What a lane says about an event: the words, `None` when it has nothing to say, or why it
/// could not.
pub type Work = Arc<dyn Fn(Lane, &Event) -> Result<Option<String>, String> + Send + Sync>;

/// The sentences `narrate` builds for the Game and Me lanes, held to the chat limit. The You lane is not
/// here: it needs a search and keeps state between events (see `Voice`).
pub fn narrate_lane(lane: Lane, event: &Event) -> Result<Option<String>, String> {
    match (lane, event) {
        (Lane::Game, Event::Student { mv, fen, .. }) => {
            let (board, m) = played(fen, mv)?;
            Ok(Some(say_theirs_within(&board, m, lane.room()).text))
        }
        (
            Lane::Me,
            Event::Tutor {
                mv,
                fen,
                depth,
                score,
                ..
            },
        ) => {
            let (board, m) = played(fen, mv)?;
            let report = Report {
                depth: *depth,
                score: *score,
                nodes: 0,
                elapsed: Duration::ZERO,
                pv: vec![m],
            };
            Ok(say_within(&board, &report, lane.room()).map(|s| s.text))
        }
        _ => Ok(None),
    }
}

/// How much the coach says (B16). `Every` is the B15 voice: a line for every move in every lane. `Quiet` speaks
/// when something happened, and gives a reason for what it says (see `why`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Every,
    Quiet,
}

impl Mode {
    pub fn parse(word: &str) -> Option<Mode> {
        match word {
            "every" => Some(Mode::Every),
            "quiet" => Some(Mode::Quiet),
            _ => None,
        }
    }
}

/// The shipped work: `narrate_lane` for the Game and Me lanes, and the judge for the You lane.
pub struct Voice {
    settings: Settings,
    interrupt: Arc<AtomicBool>,
    mode: Mode,
    /// Made at the first You event, so a coach that is never asked to judge costs nothing.
    judge: Mutex<Option<Judge>>,
    /// The evaluation blocks, one graph each, for the reasons. Made at the first quiet event.
    terms: Mutex<Option<Terms>>,
    /// `(ply, standing, spoke)` after each of her moves, so a standing is said only when it changes and an ordinary
    /// line waits for the cooldown (B17). A takeback removes the entries from its ply on when the move at that ply is
    /// made again.
    standing: Mutex<Vec<(u32, Standing, bool)>>,
}

impl Voice {
    /// `interrupt` is the flag the coach raises when the student has moved; the judge's ponder watches it. The voice
    /// is `Every`, the B15 lines; `with_mode` chooses.
    pub fn new(settings: Settings, interrupt: Arc<AtomicBool>) -> Arc<Voice> {
        Voice::with_mode(settings, interrupt, Mode::Every)
    }

    pub fn with_mode(settings: Settings, interrupt: Arc<AtomicBool>, mode: Mode) -> Arc<Voice> {
        Arc::new(Voice {
            settings,
            interrupt,
            mode,
            judge: Mutex::new(None),
            terms: Mutex::new(None),
            standing: Mutex::new(Vec::new()),
        })
    }

    fn with_terms<T>(&self, f: impl FnOnce(&mut Terms) -> T) -> Result<T, String> {
        let mut slot = lock(&self.terms);
        if slot.is_none() {
            *slot = Some(Terms::new()?);
        }
        match slot.as_mut() {
            Some(terms) => Ok(f(terms)),
            None => Err("no blocks".to_string()),
        }
    }

    pub fn work(self: &Arc<Voice>) -> Work {
        let voice = self.clone();
        Arc::new(move |lane, event| voice.speak(lane, event))
    }

    fn speak(&self, lane: Lane, event: &Event) -> Result<Option<String>, String> {
        match (lane, event) {
            (Lane::You, Event::Tutor { ply, mv, fen, .. }) => {
                let (mut board, m) = played(fen, mv)?;
                board.make(m);
                self.with_judge(|judge| {
                    judge.ponder(&board, *ply);
                    Ok(None)
                })
            }
            (Lane::You, Event::Student { ply, mv, fen }) => {
                let (board, m) = played(fen, mv)?;
                self.with_judge(|judge| {
                    let verdict = judge.judge(&board, m, *ply)?;
                    if self.mode == Mode::Every {
                        return Ok(Some(sentence(&board, &verdict, lane.room())));
                    }
                    let reason = if verdict.class > crate::judge::Class::Good {
                        self.with_terms(|t| you_reason(t, &board, &verdict))?
                    } else {
                        None
                    };
                    Ok(sentence_quiet(
                        &board,
                        &verdict,
                        reason.as_deref(),
                        lane.room(),
                    ))
                })
            }
            (Lane::Game, Event::Student { mv, fen, .. }) if self.mode == Mode::Quiet => {
                let (board, m) = played(fen, mv)?;
                Ok(say_theirs_quiet_within(&board, m, lane.room()).map(|s| s.text))
            }
            (
                Lane::Me,
                Event::Tutor {
                    ply,
                    mv,
                    fen,
                    depth,
                    score,
                },
            ) if self.mode == Mode::Quiet => {
                let (board, m) = played(fen, mv)?;
                let report = Report {
                    depth: *depth,
                    score: *score,
                    nodes: 0,
                    elapsed: Duration::ZERO,
                    pv: vec![m],
                };
                let (prev, wait) = {
                    let mut history = lock(&self.standing);
                    history.retain(|(p, _, _)| p < ply);
                    let spoke: Vec<bool> = history.iter().map(|(_, _, s)| *s).collect();
                    (
                        history.last().map_or(Standing::default(), |(_, s, _)| *s),
                        cooling(&spoke),
                    )
                };
                let (said, now) =
                    self.with_terms(|t| me_quiet(t, &board, &report, prev, wait, lane.room()))?;
                lock(&self.standing).push((*ply, now, said.is_some()));
                Ok(said.map(|s| s.text))
            }
            _ => narrate_lane(lane, event),
        }
    }

    fn with_judge(
        &self,
        f: impl FnOnce(&mut Judge) -> Result<Option<String>, String>,
    ) -> Result<Option<String>, String> {
        let mut slot = lock(&self.judge);
        if slot.is_none() {
            *slot = Some(Judge::new(self.settings, self.interrupt.clone())?);
        }
        match slot.as_mut() {
            Some(judge) => f(judge),
            None => Err("no judge".to_string()),
        }
    }
}

/// What the outbox needs to know about the game as the events arrive.
struct Shared {
    /// Counts takebacks. An event carries the epoch it was sent in.
    epoch: u32,
    /// The highest ply any event has named since the last takeback.
    newest: u32,
    /// `(epoch, ply)` for each takeback: lines from that epoch or earlier, for that ply or later, are void.
    cuts: Vec<(u32, u32)>,
}

impl Shared {
    fn why_dead(&self, epoch: u32, ply: u32) -> Option<&'static str> {
        if self.cuts.iter().any(|&(e, k)| e >= epoch && ply >= k) {
            Some("takeback")
        } else if self.newest.saturating_sub(ply) > 1 {
            Some("stale")
        } else {
            None
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct Job {
    epoch: u32,
    event: Arc<Event>,
}

enum Msg {
    Line {
        lane: Lane,
        ply: u32,
        epoch: u32,
        text: String,
    },
    Drop {
        lane: Lane,
        ply: u32,
        why: String,
    },
    /// The lane had nothing to say for this ply (quiet voice only).
    Skip {
        lane: Lane,
        ply: u32,
        epoch: u32,
    },
    Note(String),
}

/// One line the coach said or dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub lane: Lane,
    pub ply: u32,
    /// The newest ply the coach had been told of when it decided.
    pub newest: u32,
    /// Why it was dropped; empty for a line that was said.
    pub why: String,
}

#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub said: Vec<Entry>,
    pub dropped: Vec<Entry>,
    /// Plies a lane chose to say nothing for. Filled only when the coach writes `skip` lines (quiet voice).
    pub skipped: Vec<Entry>,
}

fn put(out: &mut dyn Write, line: &str) {
    let mut s: String = line
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    s.push('\n');
    let _ = out.write_all(s.as_bytes());
    let _ = out.flush();
}

fn run_outbox(
    rx: Receiver<Msg>,
    shared: Arc<Mutex<Shared>>,
    mut out: Box<dyn Write + Send>,
    skips: bool,
) -> Summary {
    let mut summary = Summary::default();
    for msg in rx {
        match msg {
            Msg::Line {
                lane,
                ply,
                epoch,
                text,
            } => {
                let (newest, dead) = {
                    let s = lock(&shared);
                    (s.newest, s.why_dead(epoch, ply))
                };
                match dead {
                    None => {
                        put(&mut *out, &format!("say {} {} {}", ply, lane.word(), text));
                        summary.said.push(Entry {
                            lane,
                            ply,
                            newest,
                            why: String::new(),
                        });
                    }
                    Some(why) => {
                        put(&mut *out, &format!("drop {} {} {}", ply, lane.word(), why));
                        summary.dropped.push(Entry {
                            lane,
                            ply,
                            newest,
                            why: why.to_string(),
                        });
                    }
                }
            }
            Msg::Drop { lane, ply, why } => {
                let newest = lock(&shared).newest;
                put(&mut *out, &format!("drop {} {} {}", ply, lane.word(), why));
                summary.dropped.push(Entry {
                    lane,
                    ply,
                    newest,
                    why,
                });
            }
            Msg::Skip { lane, ply, epoch } => {
                let (newest, dead) = {
                    let s = lock(&shared);
                    (s.newest, s.why_dead(epoch, ply))
                };
                // A silence for a ply that has gone is no news.
                if dead.is_none() {
                    put(&mut *out, &format!("skip {} {}", ply, lane.word()));
                    summary.skipped.push(Entry {
                        lane,
                        ply,
                        newest,
                        why: String::new(),
                    });
                }
            }
            Msg::Note(text) => put(&mut *out, &format!("note {}", text)),
        }
    }
    let closing = if skips {
        format!(
            "note said {} dropped {} skipped {}",
            summary.said.len(),
            summary.dropped.len(),
            summary.skipped.len()
        )
    } else {
        format!(
            "note said {} dropped {}",
            summary.said.len(),
            summary.dropped.len()
        )
    };
    put(&mut *out, &closing);
    summary
}

fn lane_loop(
    lane: Lane,
    jobs: Receiver<Job>,
    shared: Arc<Mutex<Shared>>,
    work: Work,
    out: Sender<Msg>,
    skips: bool,
) {
    for job in jobs {
        let ply = job.event.ply();
        // A job that is already out of date costs nothing.
        let dead = lock(&shared).why_dead(job.epoch, ply);
        if let Some(why) = dead {
            let _ = out.send(Msg::Drop {
                lane,
                ply,
                why: why.to_string(),
            });
            continue;
        }
        // A lane that fails must not stop the others, or the game.
        let done =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(lane, &job.event)))
                .unwrap_or_else(|_| Err("the lane stopped on this event".to_string()));
        match done {
            Ok(Some(text)) => {
                let text: String = text.chars().take(MAX_CHARS).collect();
                let _ = out.send(Msg::Line {
                    lane,
                    ply,
                    epoch: job.epoch,
                    text,
                });
            }
            Ok(None) => {
                // A lane that has nothing to say for a move it was asked about says so, so a reader that counts
                // lines can tell silence from a delay. The You lane's search while he thinks is not a move.
                if skips && speaks(lane, &job.event) {
                    let _ = out.send(Msg::Skip {
                        lane,
                        ply,
                        epoch: job.epoch,
                    });
                }
            }
            Err(why) => {
                let _ = out.send(Msg::Drop { lane, ply, why });
            }
        }
    }
}

/// The events a lane answers with a line (or, in the quiet voice, a `skip`).
fn speaks(lane: Lane, event: &Event) -> bool {
    matches!(
        (lane, event),
        (Lane::Game, Event::Student { .. })
            | (Lane::You, Event::Student { .. })
            | (Lane::Me, Event::Tutor { .. })
    )
}

fn feeds(lane: Lane, event: &Event) -> bool {
    matches!(
        (lane, event),
        (Lane::Game, Event::Student { .. })
            | (Lane::You, Event::Student { .. })
            | (Lane::You, Event::Tutor { .. })
            | (Lane::Me, Event::Tutor { .. })
    )
}

pub struct Coach {
    shared: Arc<Mutex<Shared>>,
    /// Raised when the student moves or the game goes back, and lowered when the tutor moves. A search that
    /// runs while the student thinks watches it and gives way.
    interrupt: Arc<AtomicBool>,
    lanes: Vec<(Lane, Sender<Job>)>,
    workers: Vec<JoinHandle<()>>,
    notes: Sender<Msg>,
    outbox: JoinHandle<Summary>,
}

impl Coach {
    /// Start the three lanes and the outbox, which writes to `out`.
    pub fn start(out: impl Write + Send + 'static, work: Work) -> Coach {
        Coach::start_with(out, work, Arc::new(AtomicBool::new(false)))
    }

    /// `start`, with the flag that the work watches (see `Voice`).
    pub fn start_with(
        out: impl Write + Send + 'static,
        work: Work,
        interrupt: Arc<AtomicBool>,
    ) -> Coach {
        Coach::start_voiced(out, work, interrupt, false)
    }

    /// `start_with`, and `skips`: whether a lane that has nothing to say writes a `skip` line (the quiet voice).
    pub fn start_voiced(
        out: impl Write + Send + 'static,
        work: Work,
        interrupt: Arc<AtomicBool>,
        skips: bool,
    ) -> Coach {
        let shared = Arc::new(Mutex::new(Shared {
            epoch: 0,
            newest: 0,
            cuts: Vec::new(),
        }));
        let (to_outbox, from_lanes) = channel::<Msg>();
        let outbox = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("coach-outbox".into())
                .spawn(move || run_outbox(from_lanes, shared, Box::new(out), skips))
                .expect("start the outbox thread")
        };
        let mut lanes = Vec::new();
        let mut workers = Vec::new();
        for lane in Lane::ALL {
            let (tx, rx) = channel::<Job>();
            let (shared, work, out) = (shared.clone(), work.clone(), to_outbox.clone());
            workers.push(
                std::thread::Builder::new()
                    .name(format!("coach-{}", lane.word()))
                    .spawn(move || lane_loop(lane, rx, shared, work, out, skips))
                    .expect("start a lane thread"),
            );
            lanes.push((lane, tx));
        }
        Coach {
            shared,
            interrupt,
            lanes,
            workers,
            notes: to_outbox,
            outbox,
        }
    }

    /// The hook for one event. It copies and returns; it never waits on a lane.
    pub fn send(&self, event: Event) {
        // The flag is set before the event is queued, so a search that is already running sees it first.
        match &event {
            Event::Tutor { .. } => self.interrupt.store(false, Ordering::Relaxed),
            Event::Student { .. } | Event::Takeback { .. } => {
                self.interrupt.store(true, Ordering::Relaxed)
            }
        }
        let epoch = {
            let mut s = lock(&self.shared);
            match &event {
                Event::Takeback { ply } => {
                    let cut = (s.epoch, *ply);
                    s.cuts.push(cut);
                    s.epoch += 1;
                    s.newest = ply.saturating_sub(1);
                    return;
                }
                other => {
                    s.newest = s.newest.max(other.ply());
                    s.epoch
                }
            }
        };
        let event = Arc::new(event);
        for (lane, tx) in &self.lanes {
            if feeds(*lane, &event) {
                let _ = tx.send(Job {
                    epoch,
                    event: event.clone(),
                });
            }
        }
    }

    /// A line for the reader that is about the coach itself, not a lane.
    pub fn note(&self, text: String) {
        let _ = self.notes.send(Msg::Note(text));
    }

    /// Let the lanes finish what is queued, then stop. The summary lists every line said and dropped.
    pub fn finish(self) -> Summary {
        let Coach {
            lanes,
            workers,
            notes,
            outbox,
            interrupt,
            ..
        } = self;
        // Nobody is thinking any more, so a search for him has no use.
        interrupt.store(true, Ordering::Relaxed);
        drop(lanes);
        for w in workers {
            let _ = w.join();
        }
        drop(notes);
        outbox.join().unwrap_or_default()
    }
}

fn shorten(line: &str) -> String {
    line.chars().take(60).collect()
}

/// The coach as a process: events in, lines out, until `quit` or the end of the input.
pub fn run<R: BufRead>(input: R, out: impl Write + Send + 'static) -> Summary {
    run_with(input, out, Settings::default())
}

/// `run`, with the depth and node limits of the judge's searches.
pub fn run_with<R: BufRead>(
    input: R,
    out: impl Write + Send + 'static,
    settings: Settings,
) -> Summary {
    run_voiced(input, out, settings, Mode::Every)
}

/// `run_with`, in the voice `mode`. `Quiet` also writes a `skip` line for each lane that has nothing to say.
pub fn run_voiced<R: BufRead>(
    input: R,
    out: impl Write + Send + 'static,
    settings: Settings,
    mode: Mode,
) -> Summary {
    let interrupt = Arc::new(AtomicBool::new(false));
    let voice = Voice::with_mode(settings, interrupt.clone(), mode);
    let coach = Coach::start_voiced(out, voice.work(), interrupt, mode == Mode::Quiet);
    for line in input.lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "quit" {
            break;
        }
        match parse(line) {
            Ok(event) => coach.send(event),
            Err(why) => coach.note(format!("bad line ({}): {}", why, shorten(line))),
        }
    }
    coach.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::narrate::{say, say_theirs};
    use crate::search::MATE;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    const START: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";
    const AFTER_E4: &str = "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1";

    /// A writer that keeps every write call whole, with the time it was made.
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<(Instant, String)>>>);

    impl Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            lock(&self.0).push((Instant::now(), String::from_utf8_lossy(buf).into_owned()));
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Capture {
        /// Every write, checked to be exactly one whole line.
        fn lines(&self) -> Vec<(Instant, String)> {
            lock(&self.0)
                .iter()
                .map(|(t, chunk)| {
                    assert!(chunk.ends_with('\n'), "a cut line: {:?}", chunk);
                    assert_eq!(
                        chunk.matches('\n').count(),
                        1,
                        "two lines in one write: {:?}",
                        chunk
                    );
                    (*t, chunk.trim_end().to_string())
                })
                .collect()
        }

        fn texts(&self) -> Vec<String> {
            self.lines().into_iter().map(|(_, l)| l).collect()
        }

        fn count(&self, prefix: &str) -> usize {
            lock(&self.0)
                .iter()
                .filter(|(_, c)| c.starts_with(prefix))
                .count()
        }
    }

    fn wait_for(cap: &Capture, prefix: &str) {
        wait_count(cap, prefix, 1);
    }

    fn wait_count(cap: &Capture, prefix: &str, n: usize) {
        let until = Instant::now() + Duration::from_secs(5);
        while cap.count(prefix) < n {
            assert!(
                Instant::now() < until,
                "fewer than {} lines starting {:?}",
                n,
                prefix
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn student(ply: u32) -> Event {
        Event::Student {
            ply,
            mv: "e2e4".into(),
            fen: START.into(),
        }
    }

    fn tutor(ply: u32) -> Event {
        Event::Tutor {
            ply,
            mv: "e7e5".into(),
            fen: AFTER_E4.into(),
            depth: 5,
            score: 20,
        }
    }

    /// Lanes that answer at once with a short text naming the ply.
    fn echo() -> Work {
        Arc::new(|lane, e| {
            Ok(match (lane, e) {
                (Lane::Game, Event::Student { ply, .. }) => Some(format!("g{}", ply)),
                (Lane::Me, Event::Tutor { ply, .. }) => Some(format!("m{}", ply)),
                _ => None,
            })
        })
    }

    /// Lanes like `echo`, except that the Me lane waits for a release on the given ply. Each call is numbered.
    fn gated(ply: u32) -> (Work, Sender<()>) {
        let (release, gate) = channel::<()>();
        let gate = Arc::new(Mutex::new(gate));
        let calls = Arc::new(AtomicUsize::new(0));
        let work: Work = Arc::new(move |lane, e| {
            Ok(match (lane, e) {
                (Lane::Game, Event::Student { ply, .. }) => Some(format!("g{}", ply)),
                (Lane::Me, Event::Tutor { ply: p, .. }) => {
                    let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
                    if *p == ply {
                        let _ = lock(&gate).recv();
                    }
                    Some(format!("m{}#{}", p, n))
                }
                _ => None,
            })
        });
        (work, release)
    }

    fn said(lines: &[String]) -> Vec<(u32, String, String)> {
        lines
            .iter()
            .filter_map(|l| {
                let mut parts = l.splitn(4, ' ');
                (parts.next()? == "say").then_some(())?;
                Some((
                    parts.next()?.parse().ok()?,
                    parts.next()?.to_string(),
                    parts.next()?.to_string(),
                ))
            })
            .collect()
    }

    #[test]
    fn events_parse_and_bad_lines_say_why() {
        assert_eq!(
            parse(&format!("student 1 e2e4 {}", START)),
            Ok(Event::Student {
                ply: 1,
                mv: "e2e4".into(),
                fen: START.into()
            })
        );
        assert_eq!(
            parse(&format!("tutor 2 e7e5 5 cp 35 {}", AFTER_E4)),
            Ok(Event::Tutor {
                ply: 2,
                mv: "e7e5".into(),
                fen: AFTER_E4.into(),
                depth: 5,
                score: 35
            })
        );
        let mate = |text: &str| match parse(&format!("tutor 2 e7e5 5 {} {}", text, AFTER_E4)) {
            Ok(Event::Tutor { score, .. }) => score,
            other => panic!("{:?}", other),
        };
        assert_eq!(mate("mate 3"), MATE - 5);
        assert_eq!(mate("mate -2"), -MATE + 4);
        assert_eq!(parse("takeback 5"), Ok(Event::Takeback { ply: 5 }));
        for bad in [
            "",
            "student",
            "student x e2e4 fen",
            "student 1 e2e4",
            "tutor 2 e7e5 5 cp x fen",
            "tutor 2 e7e5 5 mate 0 fen",
            "tutor 2 e7e5 5 cp 1",
            "takeback 0",
            "dance 1",
        ] {
            assert!(parse(bad).is_err(), "{:?} should not parse", bad);
        }
    }

    #[test]
    fn the_lanes_say_what_narrate_says() {
        let cap = Capture::default();
        let coach = Coach::start(cap.clone(), Arc::new(narrate_lane));
        coach.send(student(1));
        coach.send(tutor(2));
        coach.finish();

        let mut board = Board::from_fen(START).unwrap();
        let first = legal_moves(&mut board)
            .into_iter()
            .find(|m| m.to_string() == "e2e4")
            .unwrap();
        let theirs = say_theirs(&board, first).text;
        let mut board = Board::from_fen(AFTER_E4).unwrap();
        let reply = legal_moves(&mut board)
            .into_iter()
            .find(|m| m.to_string() == "e7e5")
            .unwrap();
        let report = Report {
            depth: 5,
            score: 20,
            nodes: 0,
            elapsed: Duration::ZERO,
            pv: vec![reply],
        };
        let mine = say(&board, &report).unwrap().text;

        let lines = said(&cap.texts());
        let game = lines.iter().find(|l| l.1 == "game").unwrap();
        let me = lines.iter().find(|l| l.1 == "me").unwrap();
        assert_eq!(game.2, theirs);
        assert_eq!(me.2, mine);
        assert!(
            lines.iter().all(|l| l.1 != "you"),
            "the You lane has nothing to say yet"
        );
    }

    #[test]
    fn a_line_never_passes_the_limit_whatever_the_room() {
        for room in [10, 20, 40, 80, 134] {
            let mut board = Board::from_fen(START).unwrap();
            let m = legal_moves(&mut board)
                .into_iter()
                .find(|m| m.to_string() == "e2e4")
                .unwrap();
            assert!(say_theirs_within(&board, m, room).text.chars().count() <= room);
            let mut board = Board::from_fen(AFTER_E4).unwrap();
            let m = legal_moves(&mut board)
                .into_iter()
                .find(|m| m.to_string() == "e7e5")
                .unwrap();
            let report = Report {
                depth: 5,
                score: 20,
                nodes: 0,
                elapsed: Duration::ZERO,
                pv: vec![m],
            };
            assert!(
                say_within(&board, &report, room)
                    .unwrap()
                    .text
                    .chars()
                    .count()
                    <= room
            );
        }
        // A lane that ignores its room is cut at the chat limit.
        let cap = Capture::default();
        let long: Work = Arc::new(|lane, e| {
            Ok(matches!((lane, e), (Lane::Game, Event::Student { .. })).then(|| "x".repeat(500)))
        });
        let coach = Coach::start(cap.clone(), long);
        coach.send(student(1));
        coach.finish();
        let lines = said(&cap.texts());
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].2.chars().count(), MAX_CHARS);
    }

    #[test]
    fn lines_come_out_whole_and_in_order_under_load() {
        let cap = Capture::default();
        let work: Work = Arc::new(|lane, e| {
            // A few hundred microseconds, different for each ply, and different for each lane.
            let wait = (e.ply() as u64 * 2654435761 % 1500) + 100;
            std::thread::sleep(Duration::from_micros(wait));
            echo()(lane, e)
        });
        let coach = Arc::new(Coach::start(cap.clone(), work));
        let producers: Vec<_> = [1u32, 2]
            .into_iter()
            .map(|first| {
                let coach = coach.clone();
                std::thread::spawn(move || {
                    for k in 0..1000 {
                        let ply = first + 2 * k;
                        coach.send(if first == 1 { student(ply) } else { tutor(ply) });
                        if k % 7 == 0 {
                            std::thread::sleep(Duration::from_micros(300));
                        }
                    }
                })
            })
            .collect();
        for p in producers {
            p.join().unwrap();
        }
        let summary = Arc::try_unwrap(coach)
            .ok()
            .expect("the producers are done")
            .finish();

        let lines = cap.texts();
        for word in ["game", "me"] {
            let plies: Vec<u32> = said(&lines)
                .iter()
                .filter(|l| l.1 == word)
                .map(|l| l.0)
                .collect();
            assert!(
                plies.windows(2).all(|w| w[0] < w[1]),
                "{} went backward",
                word
            );
        }
        for e in &summary.said {
            assert!(
                e.newest.saturating_sub(e.ply) <= 1,
                "{:?} was said too late",
                e
            );
        }
        for lane in [Lane::Game, Lane::Me] {
            let said = summary.said.iter().filter(|e| e.lane == lane).count();
            let dropped = summary.dropped.iter().filter(|e| e.lane == lane).count();
            assert_eq!(
                said + dropped,
                1000,
                "{:?}: every event has an outcome",
                lane
            );
        }
        assert!(summary.dropped.iter().all(|e| e.why == "stale"));
        assert!(
            lines.last().unwrap().starts_with("note said "),
            "the coach ends with its count"
        );
    }

    #[test]
    fn a_takeback_voids_the_waiting_line_and_lets_the_new_one_through() {
        let cap = Capture::default();
        let (work, release) = gated(10);
        let coach = Coach::start(cap.clone(), work);
        coach.send(student(9));
        coach.send(tutor(10)); // the Me lane is now held on this one
        wait_for(&cap, "say 9 game");
        coach.send(Event::Takeback { ply: 9 });
        coach.send(student(9));
        coach.send(tutor(10)); // the same ply again, from the new line of play
        wait_count(&cap, "say 9 game", 2);
        release.send(()).unwrap();
        release.send(()).unwrap();
        coach.finish();

        let lines = cap.texts();
        let me: Vec<&String> = lines.iter().filter(|l| l.contains(" me ")).collect();
        assert_eq!(me.len(), 2, "{:?}", lines);
        assert_eq!(me[0], "drop 10 me takeback");
        assert!(me[1].starts_with("say 10 me m10#2"), "{:?}", me);
        assert!(
            !lines.iter().any(|l| l.contains("m10#1")),
            "the old line was said: {:?}",
            lines
        );
        assert_eq!(
            said(&lines).iter().filter(|l| l.1 == "game").count(),
            2,
            "{:?}",
            lines
        );
    }

    #[test]
    fn a_line_one_ply_behind_is_said_and_two_behind_is_dropped() {
        // One behind.
        let cap = Capture::default();
        let (work, release) = gated(2);
        let coach = Coach::start(cap.clone(), work);
        coach.send(tutor(2));
        coach.send(student(3));
        wait_for(&cap, "say 3 game");
        release.send(()).unwrap();
        coach.finish();
        assert!(
            cap.texts().iter().any(|l| l.starts_with("say 2 me ")),
            "{:?}",
            cap.texts()
        );

        // Two behind.
        let cap = Capture::default();
        let (work, release) = gated(2);
        let coach = Coach::start(cap.clone(), work);
        coach.send(tutor(2));
        coach.send(student(3));
        coach.send(tutor(4));
        wait_for(&cap, "say 3 game");
        release.send(()).unwrap();
        coach.finish();
        let lines = cap.texts();
        assert!(lines.iter().any(|l| l == "drop 2 me stale"), "{:?}", lines);
        assert!(
            lines.iter().any(|l| l.starts_with("say 4 me ")),
            "{:?}",
            lines
        );
    }

    #[test]
    fn the_game_line_does_not_wait_for_the_tutor_lane() {
        let cap = Capture::default();
        let work: Work = Arc::new(|lane, e| {
            if lane == Lane::Me {
                std::thread::sleep(Duration::from_millis(200));
            }
            echo()(lane, e)
        });
        let coach = Coach::start(cap.clone(), work);
        let mut sent = Vec::new();
        for k in 0..100u32 {
            sent.push((2 * k + 1, Instant::now()));
            coach.send(student(2 * k + 1));
            std::thread::sleep(Duration::from_millis(10));
            coach.send(tutor(2 * k + 2));
            std::thread::sleep(Duration::from_millis(10));
        }
        coach.finish();
        let lines = cap.lines();
        let mut slowest = Duration::ZERO;
        for (ply, at) in sent {
            let want = format!("say {} game ", ply);
            let (t, _) = lines
                .iter()
                .find(|(_, l)| l.starts_with(&want))
                .unwrap_or_else(|| panic!("no Game line for ply {}", ply));
            slowest = slowest.max(t.duration_since(at));
        }
        println!("slowest Game line of 100: {:?}", slowest);
        assert!(
            slowest < Duration::from_millis(50),
            "slowest Game line took {:?}",
            slowest
        );
    }

    #[test]
    fn a_lane_that_panics_does_not_stop_the_others() {
        let cap = Capture::default();
        let work: Work = Arc::new(|lane, e| {
            if lane == Lane::Me && e.ply() == 2 {
                panic!("a lane bug");
            }
            echo()(lane, e)
        });
        let coach = Coach::start(cap.clone(), work);
        coach.send(tutor(2));
        wait_for(&cap, "drop 2 me");
        coach.send(student(3));
        coach.send(tutor(4));
        coach.finish();
        let lines = cap.texts();
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("drop 2 me the lane stopped")),
            "{:?}",
            lines
        );
        assert!(
            lines.iter().any(|l| l.starts_with("say 4 me ")),
            "{:?}",
            lines
        );
        assert!(
            lines.iter().any(|l| l.starts_with("say 3 game ")),
            "{:?}",
            lines
        );
    }

    #[test]
    fn the_process_loop_reads_events_and_ends_on_quit() {
        let cap = Capture::default();
        let input = format!(
            "student 1 e2e4 {}\ntutor 2 e7e5 5 cp 20 {}\nnonsense\nquit\nstudent 5 e2e4 {}\n",
            START, AFTER_E4, START
        );
        let summary = run(Cursor::new(input), cap.clone());
        let lines = cap.texts();
        // The Game line, the Me line and the You line.
        assert_eq!(said(&lines).len(), 3, "{:?}", lines);
        assert!(
            lines.iter().any(|l| l.starts_with("say 1 you You ")),
            "{:?}",
            lines
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("note bad line (unknown event: nonsense)")),
            "{:?}",
            lines
        );
        assert_eq!(summary.said.len(), 3);
        assert!(
            !lines
                .iter()
                .any(|l| l.starts_with("say 5") || l.starts_with("drop 5")),
            "nothing after quit: {:?}",
            lines
        );

        // A position or a move that does not check out is dropped with the reason, and the next event still goes.
        let cap = Capture::default();
        let input = format!("student 1 e2e4 not-a-fen\nstudent 2 e2e5 {}\n", START);
        run(Cursor::new(input), cap.clone());
        let lines = cap.texts();
        assert!(
            lines.iter().any(|l| l.starts_with("drop 1 game bad fen")),
            "{:?}",
            lines
        );
        assert!(
            lines.iter().any(|l| l == "drop 2 game illegal move e2e5"),
            "{:?}",
            lines
        );
        assert!(
            lines.iter().any(|l| l.starts_with("drop 1 you bad fen")),
            "the You lane says why too: {:?}",
            lines
        );
    }

    const AFTER_E4_E5: &str = "rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 2";

    /// The You line for the student's third half-move, g1f3, with the given events before it and a pause after
    /// each. Small limits keep the test quick; the property does not depend on them.
    fn you_line(before: &[(Event, u64)]) -> String {
        let cap = Capture::default();
        let interrupt = Arc::new(AtomicBool::new(false));
        let settings = Settings {
            depth: 5,
            nodes: 100_000,
        };
        let voice = Voice::new(settings, interrupt.clone());
        let coach = Coach::start_with(cap.clone(), voice.work(), interrupt);
        for (event, pause) in before {
            coach.send(event.clone());
            std::thread::sleep(Duration::from_millis(*pause));
        }
        coach.send(Event::Student {
            ply: 3,
            mv: "g1f3".into(),
            fen: AFTER_E4_E5.into(),
        });
        coach.finish();
        let lines = cap.texts();
        lines
            .iter()
            .find(|l| l.starts_with("say 3 you "))
            .unwrap_or_else(|| panic!("no You line: {:?}", lines))
            .clone()
    }

    #[test]
    fn the_you_line_is_the_same_however_long_the_student_thought() {
        let ponder = || Event::Tutor {
            ply: 2,
            mv: "e7e5".into(),
            fen: AFTER_E4.into(),
            depth: 5,
            score: 20,
        };
        // No ponder at all, a ponder cut off at once, and a ponder given time to finish.
        let cold = you_line(&[]);
        let cut = you_line(&[(ponder(), 0)]);
        let finished = you_line(&[(ponder(), 1500)]);
        assert!(cold.starts_with("say 3 you You "), "{}", cold);
        assert!(cold.chars().count() <= 140 + "say 3 you ".len());
        assert_eq!(cold, cut);
        assert_eq!(cold, finished);
    }

    #[test]
    fn the_flag_goes_up_when_the_student_moves_and_down_when_the_tutor_does() {
        let flag = Arc::new(AtomicBool::new(false));
        let coach = Coach::start_with(Capture::default(), echo(), flag.clone());
        coach.send(tutor(2));
        assert!(!flag.load(Ordering::Relaxed));
        coach.send(student(3));
        assert!(flag.load(Ordering::Relaxed));
        coach.send(tutor(4));
        assert!(!flag.load(Ordering::Relaxed));
        coach.send(Event::Takeback { ply: 3 });
        assert!(flag.load(Ordering::Relaxed));
        coach.finish();
        assert!(
            flag.load(Ordering::Relaxed),
            "finish stops any search for the student"
        );
    }
}
