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

use crate::board::Board;
use crate::graph::{DEFAULT_GRAPH, Graph};
use crate::movegen::legal_moves;
use crate::search::{Limits, Searcher};

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
