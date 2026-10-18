//! A chess engine built the way a plugin graph is built: the board is a stateful actor that
//! takes make and unmake as messages, blocks read it and report meters, gates check every build.
//!
//! Increment 1: board, move generation, perft.

pub mod board;
pub mod movegen;
pub mod perft;
pub mod tables;

pub use board::{Board, Color, Move, MoveKind, Piece, Square};
