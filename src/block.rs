//! What a block is, and how blocks describe themselves. Carried over from RUSE (ruse-blocks/src/block.rs).
//!
//! Every block declares its arguments, with units, ranges and defaults, in one static table. The graph reads
//! that table to check a graph, to clamp values while it runs, and to tell an agent what exists.
//!
//! The signal is a [`Frame`]: one score per side, in centipawns, positive when good for that side. A block is a
//! term: `process` reads the board and adds to each side's channel. `control` runs once per position with every
//! argument evaluated, the way RUSE's runs once per 32 samples.

use crate::board::Board;
use crate::expr::Ctx;

pub const CHANNELS: usize = 2;

/// One score per side: channel 0 is White, channel 1 is Black.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Frame {
    pub ch: [f32; CHANNELS],
}

impl Frame {
    /// The score from White's point of view.
    pub fn white(&self) -> f32 {
        self.ch[0] - self.ch[1]
    }
}

pub trait Block: Send {
    /// Called once when the graph is built, with every argument at its constant value or its default. Blocks
    /// read their `fixed` arguments here, because those cannot change while the graph runs.
    fn configure(&mut self, _args: &[f32]) {}

    /// Called once for every position, with the value of every argument in the order of `BlockInfo::args`,
    /// already clamped to its range.
    fn control(&mut self, args: &[f32]);

    /// Adds this block's term for each side to `io`.
    fn process(&mut self, board: &Board, ctx: &Ctx, io: &mut Frame);

    /// Plies of lookahead this block needs, which the graph sums and reports.
    fn latency(&self) -> u32 {
        0
    }

    /// The current value of meter number `index`, in the order of `BlockInfo::meters`.
    fn meter(&self, _index: usize) -> f32 {
        0.0
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ArgInfo {
    pub name: &'static str,
    pub unit: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    /// The names of a choice, in index order. Empty for a number.
    pub options: &'static [&'static str],
    /// A fixed argument is read when the graph is built and cannot follow a parameter.
    pub fixed: bool,
    pub doc: &'static str,
}

impl ArgInfo {
    pub const fn num(
        name: &'static str,
        unit: &'static str,
        min: f32,
        max: f32,
        default: f32,
        doc: &'static str,
    ) -> ArgInfo {
        ArgInfo {
            name,
            unit,
            min,
            max,
            default,
            options: &[],
            fixed: false,
            doc,
        }
    }

    pub const fn choice(
        name: &'static str,
        options: &'static [&'static str],
        default: usize,
        doc: &'static str,
    ) -> ArgInfo {
        ArgInfo {
            name,
            unit: "",
            min: 0.0,
            max: (options.len() - 1) as f32,
            default: default as f32,
            options,
            fixed: false,
            doc,
        }
    }

    pub const fn fixed(self) -> ArgInfo {
        ArgInfo {
            fixed: true,
            ..self
        }
    }

    pub fn is_choice(&self) -> bool {
        !self.options.is_empty()
    }

    /// The value a block gets: in range, and the default if the expression gave something that is not a number.
    pub fn clean(&self, v: f32) -> f32 {
        if v.is_nan() {
            self.default
        } else {
            let v = v.clamp(self.min, self.max);
            if self.is_choice() { v.round() } else { v }
        }
    }
}

pub struct BlockInfo {
    pub name: &'static str,
    pub doc: &'static str,
    pub args: &'static [ArgInfo],
    /// Values the block reports, which a render lists and a graph can bind to a meter parameter.
    pub meters: &'static [&'static str],
    pub make: fn() -> Box<dyn Block>,
}

impl BlockInfo {
    pub fn arg_index(&self, name: &str) -> Option<usize> {
        self.args.iter().position(|a| a.name == name)
    }

    pub fn meter_index(&self, name: &str) -> Option<usize> {
        self.meters.iter().position(|m| *m == name)
    }
}
