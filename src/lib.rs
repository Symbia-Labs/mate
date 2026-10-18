//! A chess engine built the way a plugin graph is built: the board is a stateful actor that takes make and
//! unmake as messages, evaluation is a graph of blocks that read it and report meters, and gates check every
//! build.
//!
//! B1: board, move generation, perft. B2: the block model, the expression language, the graph, PGN and render.

pub mod block;
pub mod blocks;
pub mod board;
pub mod expr;
pub mod graph;
pub mod json;
pub mod movegen;
pub mod perft;
pub mod pgn;
pub mod spec;
pub mod tables;

pub use block::{ArgInfo, Block, BlockInfo, Frame};
pub use board::{Board, Color, Move, MoveKind, Piece, Square};
pub use graph::Graph;
