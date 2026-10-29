//! The engine in a browser: a handful of C-style exports, no bindings library. Strings go in through `alloc`
//! and come back through one output buffer read with `out_ptr` and `out_len`.
//!
//! - `legal(fen)` → the legal moves, space separated, led by `check` when the side to move is in check; or
//!   `mate`, `stalemate`.
//! - `play(fen, move)` → the FEN after the move, or an error starting with `!`.
//! - `think(fen, depth, graph, settings)` → `move score depth nodes`, score in centipawns from the side to move.
//! - `meters(fen, graph, settings, ply)` → `score` then `name=value` pairs, space separated.
//!
//! `graph` is a personality JSON text or `default`; `settings` is `id=value` pairs separated by spaces.
//!
//! The coach, the bot's quiet voice (B17) for a chat beside the board. Each answer is `lane<TAB>text` lines, a lane
//! that has nothing to say giving no line:
//!
//! - `coach_new()` → nothing; a new game, so the coach forgets the last one.
//! - `coach_student(ply, fen, move)` → the `game` and `you` lines for the student's move from `fen`, half-move `ply`.
//! - `coach_tutor(ply, fen, move, depth, score)` → the `me` line for the engine's move and its search.
//! - `coach_ponder(ply, fen)` → nothing; searches the position the student must answer (`ply` half-moves played),
//!   so his verdict comes quickly. The verdict is the same with or without it.

use crate::board::{Board, Move};
use crate::graph::{DEFAULT_GRAPH, Graph};
use crate::judge::{Class, Judge, Settings, sentence_quiet};
use crate::movegen::legal_moves;
use crate::narrate::{MAX_CHARS, say_theirs_quiet_within};
use crate::search::{Limits, Report, Searcher};
use crate::why::{Standing, Terms, cooling, me_quiet, you_reason};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

static mut OUT: Vec<u8> = Vec::new();

fn set_out(s: String) -> i32 {
    // Single-threaded: wasm has one thread, and every export finishes before the next call.
    unsafe {
        let out = &mut *std::ptr::addr_of_mut!(OUT);
        *out = s.into_bytes();
        out.len() as i32
    }
}

fn text(ptr: *const u8, len: usize) -> String {
    if ptr.is_null() || len == 0 {
        return String::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    String::from_utf8_lossy(bytes).into_owned()
}

#[unsafe(no_mangle)]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len.max(1));
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

#[unsafe(no_mangle)]
pub extern "C" fn out_ptr() -> *const u8 {
    unsafe { (*std::ptr::addr_of!(OUT)).as_ptr() }
}

#[unsafe(no_mangle)]
pub extern "C" fn out_len() -> usize {
    unsafe { (*std::ptr::addr_of!(OUT)).len() }
}

fn graph_from(text: &str, settings: &str) -> Result<Graph, String> {
    let mut g = Graph::from_json(if text.is_empty() || text == "default" {
        DEFAULT_GRAPH
    } else {
        text
    })?;
    for kv in settings.split_whitespace() {
        if let Some((k, v)) = kv.split_once('=')
            && let Ok(x) = v.parse::<f32>()
        {
            g.set(k, x)?;
        }
    }
    Ok(g)
}

#[unsafe(no_mangle)]
pub extern "C" fn legal(fen: *const u8, fen_len: usize) -> i32 {
    let mut b = match Board::from_fen(&text(fen, fen_len)) {
        Ok(b) => b,
        Err(e) => return set_out(format!("!{}", e)),
    };
    let moves = legal_moves(&mut b);
    if moves.is_empty() {
        return set_out(if b.in_check() {
            "mate".into()
        } else {
            "stalemate".into()
        });
    }
    let list = moves
        .iter()
        .map(|m| m.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    set_out(if b.in_check() {
        format!("check {}", list)
    } else {
        list
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn play(fen: *const u8, fen_len: usize, mv: *const u8, mv_len: usize) -> i32 {
    let mut b = match Board::from_fen(&text(fen, fen_len)) {
        Ok(b) => b,
        Err(e) => return set_out(format!("!{}", e)),
    };
    let want = text(mv, mv_len);
    match legal_moves(&mut b)
        .into_iter()
        .find(|m| m.to_string() == want)
    {
        Some(m) => {
            b.make(m);
            set_out(b.fen())
        }
        None => set_out(format!("!{} is not legal", want)),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn think(
    fen: *const u8,
    fen_len: usize,
    depth: u32,
    graph: *const u8,
    graph_len: usize,
    settings: *const u8,
    settings_len: usize,
) -> i32 {
    let mut b = match Board::from_fen(&text(fen, fen_len)) {
        Ok(b) => b,
        Err(e) => return set_out(format!("!{}", e)),
    };
    let g = match graph_from(&text(graph, graph_len), &text(settings, settings_len)) {
        Ok(g) => g,
        Err(e) => return set_out(format!("!{}", e)),
    };
    let mut s = Searcher::new(g);
    s.root_ply = (b.fullmove as u32).saturating_sub(1) * 2 + (b.turn as u32);
    let r = s.search(
        &mut b,
        &Limits {
            depth: Some(depth.clamp(1, 8)),
            ..Default::default()
        },
        |_| {},
    );
    match r.best() {
        Some(m) => set_out(format!("{} {} {} {}", m, r.score, r.depth, r.nodes)),
        None => set_out("!no move".into()),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn meters(
    fen: *const u8,
    fen_len: usize,
    graph: *const u8,
    graph_len: usize,
    settings: *const u8,
    settings_len: usize,
    ply: u32,
) -> i32 {
    let b = match Board::from_fen(&text(fen, fen_len)) {
        Ok(b) => b,
        Err(e) => return set_out(format!("!{}", e)),
    };
    let mut g = match graph_from(&text(graph, graph_len), &text(settings, settings_len)) {
        Ok(g) => g,
        Err(e) => return set_out(format!("!{}", e)),
    };
    let f = g.eval(&b, ply);
    let mut s = format!("{:.0}", f.white());
    for (k, v) in g.meters() {
        s.push_str(&format!(" {}={:.1}", k, v + 0.0));
    }
    set_out(s)
}

/// The coach's memory of one game: the judge with its ponder, the blocks for the reasons, and after each of her moves
/// what she remembers of the standing and whether she spoke (for the cooldown). The same pieces `coach::Voice` holds.
struct Coach {
    judge: Judge,
    terms: Terms,
    history: Vec<(u32, Standing, bool)>,
}

static mut COACH: Option<Coach> = None;

fn with_coach<T>(f: impl FnOnce(&mut Coach) -> Result<T, String>) -> Result<T, String> {
    // Single-threaded, as `set_out`.
    let slot = unsafe { &mut *std::ptr::addr_of_mut!(COACH) };
    if slot.is_none() {
        *slot = Some(Coach {
            judge: Judge::new(Settings::default(), Arc::new(AtomicBool::new(false)))?,
            terms: Terms::new()?,
            history: Vec::new(),
        });
    }
    match slot.as_mut() {
        Some(coach) => f(coach),
        None => Err("no coach".to_string()),
    }
}

/// The board before the move, and the move, when it is legal there.
fn played(fen: &str, mv: &str) -> Result<(Board, Move), String> {
    let mut b = Board::from_fen(fen).map_err(|e| e.to_string())?;
    let m = legal_moves(&mut b)
        .into_iter()
        .find(|m| m.to_string() == mv)
        .ok_or(format!("{} is not legal", mv))?;
    Ok((b, m))
}

fn answer(r: Result<String, String>) -> i32 {
    set_out(r.unwrap_or_else(|e| format!("!{}", e)))
}

#[unsafe(no_mangle)]
pub extern "C" fn coach_new() -> i32 {
    unsafe {
        *std::ptr::addr_of_mut!(COACH) = None;
    }
    set_out(String::new())
}

#[unsafe(no_mangle)]
pub extern "C" fn coach_student(
    ply: u32,
    fen: *const u8,
    fen_len: usize,
    mv: *const u8,
    mv_len: usize,
) -> i32 {
    answer((|| {
        let (board, m) = played(&text(fen, fen_len), &text(mv, mv_len))?;
        let mut lines = Vec::new();
        if let Some(s) = say_theirs_quiet_within(&board, m, MAX_CHARS) {
            lines.push(format!("game\t{}", s.text));
        }
        let you = with_coach(|c| {
            let v = c.judge.judge(&board, m, ply)?;
            let reason = if v.class > Class::Good {
                you_reason(&mut c.terms, &board, &v)
            } else {
                None
            };
            Ok(sentence_quiet(&board, &v, reason.as_deref(), MAX_CHARS))
        })?;
        if let Some(t) = you {
            lines.push(format!("you\t{}", t));
        }
        Ok(lines.join("\n"))
    })())
}

#[unsafe(no_mangle)]
pub extern "C" fn coach_tutor(
    ply: u32,
    fen: *const u8,
    fen_len: usize,
    mv: *const u8,
    mv_len: usize,
    depth: u32,
    score: i32,
) -> i32 {
    answer((|| {
        let (board, m) = played(&text(fen, fen_len), &text(mv, mv_len))?;
        let report = Report {
            depth,
            score,
            nodes: 0,
            elapsed: Duration::ZERO,
            pv: vec![m],
        };
        with_coach(|c| {
            c.history.retain(|(p, _, _)| *p < ply);
            let spoke: Vec<bool> = c.history.iter().map(|(_, _, s)| *s).collect();
            let prev = c.history.last().map_or(Standing::default(), |(_, s, _)| *s);
            let (said, now) = me_quiet(
                &mut c.terms,
                &board,
                &report,
                prev,
                cooling(&spoke),
                MAX_CHARS,
            );
            c.history.push((ply, now, said.is_some()));
            Ok(said.map_or(String::new(), |s| format!("me\t{}", s.text)))
        })
    })())
}

#[unsafe(no_mangle)]
pub extern "C" fn coach_ponder(ply: u32, fen: *const u8, fen_len: usize) -> i32 {
    answer(
        Board::from_fen(&text(fen, fen_len))
            .map_err(|e| e.to_string())
            .and_then(|b| {
                with_coach(|c| {
                    c.judge.ponder(&b, ply);
                    Ok(String::new())
                })
            }),
    )
}
