//! The graph runtime. A personality's whole evaluation, described as data. Carried over from RUSE
//! (ruse-blocks/src/graph.rs): the position is the sample, the ply is the control rate.
//!
//! For every position, each argument is evaluated from the parameters, the modulators and the game state, every
//! block gets one call to `control`, and then the frame runs through the nodes. `parallel` gives every branch the
//! same input and adds each one's contribution at its gain; `mix` scales a node's contribution; `bypass` skips it;
//! `only` keeps its contribution to the listed sides.

use crate::block::{ArgInfo, Block, BlockInfo, CHANNELS, Frame};
use crate::blocks;
use crate::board::Board;
use crate::expr::{Ctx, Env, Expr};
use crate::json;
use crate::spec::{self, ArgValue, GraphSpec, NodeKind, NodeSpec};

struct Arg {
    expr: Option<Expr>,
    value: f32,
}

impl Arg {
    fn eval(&self, info: &ArgInfo, env: &Env) -> f32 {
        info.clean(match &self.expr {
            Some(e) => e.eval(env),
            None => self.value,
        })
    }

    fn raw(&self, env: &Env) -> f32 {
        match &self.expr {
            Some(e) => e.eval(env),
            None => self.value,
        }
    }
}

/// Compiles one argument. A number is checked against the range. A string is a choice's name or an expression.
fn compile_arg(v: &ArgValue, info: &ArgInfo, c: &Compiler, what: &str) -> Result<Arg, String> {
    let out_of_range = |x: f32| {
        format!(
            "{} is {}, outside the range {} to {}{}",
            what,
            x,
            info.min,
            info.max,
            if info.unit.is_empty() {
                String::new()
            } else {
                format!(" {}", info.unit)
            }
        )
    };
    match v {
        ArgValue::Num(n) => {
            let x = *n as f32;
            if !x.is_finite() || x < info.min || x > info.max {
                return Err(out_of_range(x));
            }
            Ok(Arg {
                expr: None,
                value: x,
            })
        }
        ArgValue::Text(s) => {
            if info.is_choice()
                && let Some(i) = info
                    .options
                    .iter()
                    .position(|o| o.eq_ignore_ascii_case(s.trim()))
            {
                return Ok(Arg {
                    expr: None,
                    value: i as f32,
                });
            }
            let expr = c.parse(s).map_err(|e| {
                if info.is_choice() {
                    format!(
                        "{}: '{}' is not one of {} and not a valid expression ({})",
                        what,
                        s,
                        info.options.join(", "),
                        e
                    )
                } else {
                    format!("{}: {}", what, e)
                }
            })?;
            if let Some(k) = expr.constant() {
                if !k.is_finite() || k < info.min || k > info.max {
                    return Err(out_of_range(k));
                }
                return Ok(Arg {
                    expr: None,
                    value: k,
                });
            }
            if info.fixed {
                return Err(format!(
                    "{} is fixed when the graph is built, so it must be a number and cannot follow a parameter",
                    what
                ));
            }
            Ok(Arg {
                expr: Some(expr),
                value: info.default,
            })
        }
    }
}

/// A plain number or expression with no block range: gains, mix and bypass.
fn compile_free(v: &ArgValue, c: &Compiler, what: &str) -> Result<Arg, String> {
    match v {
        ArgValue::Num(n) => Ok(Arg {
            expr: None,
            value: *n as f32,
        }),
        ArgValue::Text(s) => {
            let expr = c.parse(s).map_err(|e| format!("{}: {}", what, e))?;
            Ok(match expr.constant() {
                Some(k) => Arg {
                    expr: None,
                    value: k,
                },
                None => Arg {
                    expr: Some(expr),
                    value: 0.0,
                },
            })
        }
    }
}

struct Compiler<'a> {
    ids: &'a [String],
    mods: &'a [String],
}

impl Compiler<'_> {
    fn lookup(&self, name: &str) -> Option<usize> {
        self.ids.iter().position(|x| x == name).or_else(|| {
            self.mods
                .iter()
                .position(|m| m == name)
                .map(|i| self.ids.len() + i)
        })
    }

    fn parse(&self, src: &str) -> Result<Expr, String> {
        Expr::parse(src, &|n| self.lookup(n)).map_err(|e| {
            let mut names: Vec<String> = self.ids.to_vec();
            names.extend(self.mods.iter().cloned());
            names.extend(
                ["ply", "move", "phase", "stm"]
                    .iter()
                    .map(|s| s.to_string()),
            );
            format!("{}. The names available are: {}", e, names.join(", "))
        })
    }
}

struct BlockNode {
    label: String,
    block: Box<dyn Block>,
    info: &'static BlockInfo,
    args: Vec<Arg>,
    resolved: Vec<f32>,
}

struct MixNode {
    inner: Node,
    bypass: Option<Arg>,
    mix: Option<Arg>,
    only: Option<Vec<usize>>,
    skip: bool,
    amount: f32,
}

struct Branch {
    node: Node,
    gain: Option<Arg>,
    g: f32,
}

enum Node {
    Block(BlockNode),
    Chain(Vec<Node>),
    Parallel(Vec<Branch>),
    Mix(Box<MixNode>),
}

impl Node {
    fn control(&mut self, env: &Env) {
        match self {
            Node::Block(b) => {
                for (i, a) in b.args.iter().enumerate() {
                    b.resolved[i] = a.eval(&b.info.args[i], env);
                }
                b.block.control(&b.resolved);
            }
            Node::Chain(nodes) => nodes.iter_mut().for_each(|n| n.control(env)),
            Node::Parallel(branches) => {
                for b in branches.iter_mut() {
                    b.node.control(env);
                    let g = b.gain.as_ref().map_or(1.0, |a| a.raw(env));
                    b.g = if g.is_finite() { g } else { 1.0 };
                }
            }
            Node::Mix(m) => {
                m.skip = m.bypass.as_ref().is_some_and(|a| a.raw(env) >= 0.5);
                let amount = m.mix.as_ref().map_or(1.0, |a| a.raw(env));
                m.amount = if amount.is_finite() {
                    amount.clamp(0.0, 1.0)
                } else {
                    1.0
                };
                m.inner.control(env);
            }
        }
    }

    fn process(&mut self, board: &Board, ctx: &Ctx, io: &mut Frame) {
        match self {
            Node::Block(b) => b.block.process(board, ctx, io),
            Node::Chain(nodes) => nodes.iter_mut().for_each(|n| n.process(board, ctx, io)),
            Node::Parallel(branches) => {
                let input = *io;
                let mut acc = input;
                for b in branches.iter_mut() {
                    let mut out = input;
                    b.node.process(board, ctx, &mut out);
                    for c in 0..CHANNELS {
                        acc.ch[c] += b.g * (out.ch[c] - input.ch[c]);
                    }
                }
                *io = acc;
            }
            Node::Mix(m) => {
                if m.skip {
                    return;
                }
                let dry = *io;
                let mut wet = dry;
                m.inner.process(board, ctx, &mut wet);
                for c in 0..CHANNELS {
                    let applies = m.only.as_ref().is_none_or(|o| o.contains(&c));
                    io.ch[c] = if applies {
                        dry.ch[c] + m.amount * (wet.ch[c] - dry.ch[c])
                    } else {
                        dry.ch[c]
                    };
                }
            }
        }
    }

    fn latency(&self) -> u32 {
        match self {
            Node::Block(b) => b.block.latency(),
            Node::Chain(nodes) => nodes.iter().map(|n| n.latency()).sum(),
            Node::Parallel(branches) => {
                branches.iter().map(|b| b.node.latency()).max().unwrap_or(0)
            }
            Node::Mix(m) => m.inner.latency(),
        }
    }

    fn meters(&self, out: &mut Vec<(String, f32)>) {
        match self {
            Node::Block(b) => {
                for (i, m) in b.info.meters.iter().enumerate() {
                    out.push((format!("{}.{}", b.label, m), b.block.meter(i)));
                }
            }
            Node::Chain(nodes) => nodes.iter().for_each(|n| n.meters(out)),
            Node::Parallel(branches) => branches.iter().for_each(|b| b.node.meters(out)),
            Node::Mix(m) => m.inner.meters(out),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Param {
    pub id: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub value: f32,
    pub doc: String,
}

struct Modulator {
    shape: String,
    rate: Arg,
    seed: u64,
}

pub struct Graph {
    params: Vec<Param>,
    mods: Vec<Modulator>,
    vars: Vec<f32>,
    root: Node,
    labels: std::collections::HashMap<&'static str, usize>,
}

fn compile_node(
    spec: &NodeSpec,
    c: &Compiler,
    path: &str,
    labels: &mut std::collections::HashMap<&'static str, usize>,
) -> Result<Node, String> {
    let inner = match &spec.kind {
        NodeKind::Block { name, args } => {
            let info = blocks::find(name).ok_or_else(|| {
                format!(
                    "{} names a block '{}' that does not exist. The blocks are: {}",
                    path,
                    name,
                    blocks::CATALOGUE
                        .iter()
                        .map(|b| b.name)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
            let mut compiled: Vec<Arg> = info
                .args
                .iter()
                .map(|a| Arg {
                    expr: None,
                    value: a.default,
                })
                .collect();
            for (k, v) in args {
                let i = info.arg_index(k).ok_or_else(|| {
                    format!(
                        "{} ({}) has no argument '{}'. It has: {}",
                        path,
                        name,
                        k,
                        info.args
                            .iter()
                            .map(|a| a.name)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })?;
                compiled[i] =
                    compile_arg(v, &info.args[i], c, &format!("{} ({}) {}", path, name, k))?;
            }
            let n = labels.entry(info.name).or_insert(0);
            *n += 1;
            let label = if *n == 1 {
                info.name.to_string()
            } else {
                format!("{}{}", info.name, n)
            };
            let mut block = (info.make)();
            block.configure(&compiled.iter().map(|a| a.value).collect::<Vec<_>>());
            Node::Block(BlockNode {
                label,
                block,
                info,
                resolved: vec![0.0; compiled.len()],
                args: compiled,
            })
        }
        NodeKind::Chain(nodes) => {
            let mut out = Vec::new();
            for (i, n) in nodes.iter().enumerate() {
                out.push(compile_node(
                    n,
                    c,
                    &format!("{}.chain[{}]", path, i),
                    labels,
                )?);
            }
            Node::Chain(out)
        }
        NodeKind::Parallel(branches) => {
            let mut out = Vec::new();
            for (i, b) in branches.iter().enumerate() {
                let bp = format!("{}.parallel[{}]", path, i);
                let gain = b
                    .gain
                    .as_ref()
                    .map(|g| compile_free(g, c, &format!("{} gain", bp)))
                    .transpose()?;
                out.push(Branch {
                    node: compile_node(&b.node, c, &format!("{}.node", bp), labels)?,
                    gain,
                    g: 1.0,
                });
            }
            Node::Parallel(out)
        }
    };
    if spec.bypass.is_none() && spec.mix.is_none() && spec.only.is_none() {
        return Ok(inner);
    }
    let bypass = spec
        .bypass
        .as_ref()
        .map(|v| compile_free(v, c, &format!("{} bypass", path)))
        .transpose()?;
    let mix = spec
        .mix
        .as_ref()
        .map(|v| compile_free(v, c, &format!("{} mix", path)))
        .transpose()?;
    Ok(Node::Mix(Box::new(MixNode {
        inner,
        bypass,
        mix,
        only: spec.only.clone(),
        skip: false,
        amount: 1.0,
    })))
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn modulate(shape: &str, t: f32, seed: u64) -> f32 {
    let frac = t - t.floor();
    match shape {
        "sine" => (2.0 * std::f32::consts::PI * t).sin(),
        "triangle" => 4.0 * ((frac + 0.25) - (frac + 0.25).floor() - 0.5).abs() - 1.0,
        "saw" => 2.0 * frac - 1.0,
        "square" => {
            if frac < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        _ => {
            let h = splitmix(seed.wrapping_mul(0x1000_0000_01B3) ^ (t.floor() as i64 as u64));
            (h >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0
        }
    }
}

impl Graph {
    pub fn from_json(text: &str) -> Result<Graph, String> {
        let j = json::parse(text).map_err(|e| format!("the graph is not JSON: {}", e))?;
        Graph::compile(&spec::graph_from_json(&j)?)
    }

    pub fn compile(spec: &GraphSpec) -> Result<Graph, String> {
        let ids: Vec<String> = spec.params.iter().map(|p| p.id.clone()).collect();
        for (i, id) in ids.iter().enumerate() {
            if ids[..i].contains(id) {
                return Err(format!("params has '{}' twice", id));
            }
            if ["ply", "move", "phase", "stm", "pi"].contains(&id.as_str()) {
                return Err(format!("params id '{}' is a built-in name", id));
            }
        }
        let mod_ids: Vec<String> = spec.modulators.iter().map(|m| m.id.clone()).collect();
        let c = Compiler {
            ids: &ids,
            mods: &mod_ids,
        };
        let only_params = Compiler {
            ids: &ids,
            mods: &[],
        };
        let mut mods = Vec::new();
        for (i, m) in spec.modulators.iter().enumerate() {
            if ids.contains(&m.id) {
                return Err(format!(
                    "modulators[{}] id '{}' is also a parameter",
                    i, m.id
                ));
            }
            mods.push(Modulator {
                shape: m.shape.clone(),
                rate: compile_free(&m.rate, &only_params, &format!("modulators[{}] rate", i))?,
                seed: m.seed,
            });
        }
        let mut labels = std::collections::HashMap::new();
        let root = compile_node(&spec.root, &c, "the graph", &mut labels)?;
        let params = spec
            .params
            .iter()
            .map(|p| Param {
                id: p.id.clone(),
                min: p.min,
                max: p.max,
                default: p.default,
                value: p.default,
                doc: p.doc.clone(),
            })
            .collect::<Vec<_>>();
        let vars = vec![0.0; params.len() + mods.len()];
        Ok(Graph {
            params,
            mods,
            vars,
            root,
            labels,
        })
    }

    pub fn params(&self) -> &[Param] {
        &self.params
    }

    /// Sets a parameter, clamped to its range.
    pub fn set(&mut self, id: &str, value: f32) -> Result<(), String> {
        let p = self
            .params
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("there is no parameter '{}'", id))?;
        p.value = value.clamp(p.min, p.max);
        Ok(())
    }

    pub fn latency(&self) -> u32 {
        self.root.latency()
    }

    /// How many blocks the graph holds, by name.
    pub fn block_counts(&self) -> Vec<(&'static str, usize)> {
        let mut v: Vec<_> = self.labels.iter().map(|(k, n)| (*k, *n)).collect();
        v.sort();
        v
    }

    /// The game state the expressions read for `board` at `ply`.
    pub fn ctx(board: &Board, ply: u32) -> Ctx {
        Ctx {
            ply,
            move_no: board.fullmove as u32,
            phase: board.phase(),
            stm: board.turn as u8 as f32,
        }
    }

    /// Evaluates the position: every argument read, every block run. The frame holds each side's score.
    pub fn eval(&mut self, board: &Board, ply: u32) -> Frame {
        let ctx = Graph::ctx(board, ply);
        let np = self.params.len();
        for (i, p) in self.params.iter().enumerate() {
            self.vars[i] = p.value;
        }
        for i in 0..self.mods.len() {
            let rate = {
                let env = Env {
                    vars: &self.vars[..np],
                    ctx,
                };
                self.mods[i].rate.raw(&env)
            };
            self.vars[np + i] = modulate(&self.mods[i].shape, rate * ply as f32, self.mods[i].seed);
        }
        let env = Env {
            vars: &self.vars,
            ctx,
        };
        self.root.control(&env);
        let mut io = Frame::default();
        self.root.process(board, &ctx, &mut io);
        io
    }

    /// Every block's meters after the last `eval`, labelled `block.meter` (a repeated block is numbered).
    pub fn meters(&self) -> Vec<(String, f32)> {
        let mut out = Vec::new();
        self.root.meters(&mut out);
        out
    }
}

/// A plain evaluation: every block at its defaults, summed.
pub const DEFAULT_GRAPH: &str = r#"{
  "params": [
    {"id": "sharp", "min": 0, "max": 1, "default": 1, "doc": "How much loose pieces count."},
    {"id": "safety", "min": 0, "max": 1, "default": 1, "doc": "How much king exposure counts."}
  ],
  "parallel": [
    {"node": {"block": "material"}},
    {"node": {"block": "loose"}, "gain": "sharp"},
    {"node": {"block": "king"}, "gain": "safety"},
    {"node": {"block": "mobility"}},
    {"node": {"block": "pawns"}}
  ]
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_graph_scores_the_start_position_level() {
        let mut g = Graph::from_json(DEFAULT_GRAPH).unwrap();
        let f = g.eval(&Board::start(), 0);
        assert_eq!(f.white(), 0.0);
        let m = g.meters();
        assert!(m.iter().any(|(k, v)| k == "material.white"
            && *v == 100.0 * 8.0 + 320.0 * 2.0 + 330.0 * 2.0 + 500.0 * 2.0 + 900.0));
    }

    #[test]
    fn gains_only_and_bypass_shape_the_contribution() {
        let b = Board::from_fen("4k3/8/8/8/8/8/8/R3K3 w - - 0 1").unwrap();
        let mut full = Graph::from_json(r#"{"block":"material"}"#).unwrap();
        assert_eq!(full.eval(&b, 0).white(), 500.0);
        let mut half =
            Graph::from_json(r#"{"parallel":[{"node":{"block":"material"},"gain":0.5}]}"#).unwrap();
        assert_eq!(half.eval(&b, 0).white(), 250.0);
        let mut black_only = Graph::from_json(r#"{"block":"material","only":[1]}"#).unwrap();
        assert_eq!(
            black_only.eval(&b, 0).white(),
            0.0,
            "White's rook is not counted"
        );
        let mut off = Graph::from_json(r#"{"params":[{"id":"x","min":0,"max":1,"default":1}],"block":"material","bypass":"x"}"#).unwrap();
        assert_eq!(off.eval(&b, 0).white(), 0.0);
        off.set("x", 0.0).unwrap();
        assert_eq!(off.eval(&b, 0).white(), 500.0);
        let mut mixed = Graph::from_json(r#"{"block":"material","mix":0.25}"#).unwrap();
        assert_eq!(mixed.eval(&b, 0).white(), 125.0);
    }

    #[test]
    fn expressions_read_the_game_state_and_modulators_repeat() {
        let b = Board::from_fen("4k3/8/8/8/8/8/8/R3K3 w - - 0 1").unwrap();
        let mut g = Graph::from_json(
            r#"{"parallel":[{"node":{"block":"material"},"gain":"if(ply < 10, 0, 1)"}]}"#,
        )
        .unwrap();
        assert_eq!(g.eval(&b, 4).white(), 0.0);
        assert_eq!(g.eval(&b, 12).white(), 500.0);
        let mut r = Graph::from_json(r#"{"modulators":[{"id":"dice","shape":"random","rate":1,"seed":5}],"parallel":[{"node":{"block":"material"},"gain":"dice"}]}"#).unwrap();
        let a: Vec<f32> = (0..6).map(|p| r.eval(&b, p).white()).collect();
        let again: Vec<f32> = (0..6).map(|p| r.eval(&b, p).white()).collect();
        assert_eq!(a, again, "the same seed gives the same sequence");
        assert!(a.iter().any(|x| *x != a[0]), "the sequence varies");
    }

    #[test]
    fn mistakes_say_where_and_what_exists() {
        let err = |s: &str| Graph::from_json(s).err().expect("a mistake is refused");
        assert!(err(r#"{"block":"nope"}"#).contains("does not exist. The blocks are: material"));
        assert!(
            err(r#"{"block":"material","args":{"bishopp":1}}"#)
                .contains("has no argument 'bishopp'")
        );
        assert!(
            err(r#"{"block":"material","args":{"queen":5000}}"#)
                .contains("outside the range 0 to 3000 cp")
        );
        assert!(
            err(r#"{"block":"material","args":{"queen":"nope * 2"}}"#)
                .contains("names available are")
        );
        assert!(err(r#"{"params":[{"id":"ply"}],"block":"material"}"#).contains("built-in"));
    }
}
