//! The description of a graph, as it sits in a personality file. Carried over from RUSE (ruse-blocks/src/spec.rs)
//! with the parameters moved into the same document, since a personality is one file.
//!
//! ```json
//! {
//!   "params": [{ "id": "sharp", "min": 0, "max": 1, "default": 1, "doc": "How much the loose-piece term counts." }],
//!   "modulators": [{ "id": "dice", "shape": "random", "rate": 1, "seed": 7 }],
//!   "parallel": [
//!     { "node": { "block": "material" } },
//!     { "node": { "block": "loose", "args": { "undefended": "0.5 * sharp" } }, "gain": "sharp" },
//!     { "node": { "block": "king", "only": [1] } }
//!   ]
//! }
//! ```
//!
//! A node is a `block`, a `chain` that runs its nodes one after another, or a `parallel` that runs its branches
//! on the same input and adds what each one contributes. Any node can also carry `bypass` and `mix`, which take
//! a number or an expression, and `only`, a list of channels (0 White, 1 Black) it applies to.

use crate::json::Json;

#[derive(Clone, Debug, PartialEq)]
pub enum ArgValue {
    Num(f64),
    /// An expression, or the name of a choice.
    Text(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParamSpec {
    pub id: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub doc: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModSpec {
    pub id: String,
    pub shape: String,
    pub rate: ArgValue,
    pub seed: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NodeSpec {
    pub kind: NodeKind,
    pub bypass: Option<ArgValue>,
    pub mix: Option<ArgValue>,
    pub only: Option<Vec<usize>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NodeKind {
    Block {
        name: String,
        args: Vec<(String, ArgValue)>,
    },
    Chain(Vec<NodeSpec>),
    Parallel(Vec<BranchSpec>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct BranchSpec {
    pub node: NodeSpec,
    pub gain: Option<ArgValue>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GraphSpec {
    pub params: Vec<ParamSpec>,
    pub modulators: Vec<ModSpec>,
    pub root: NodeSpec,
}

fn value(j: &Json, what: &str) -> Result<ArgValue, String> {
    match j {
        Json::Num(n) => Ok(ArgValue::Num(*n)),
        Json::Str(s) => Ok(ArgValue::Text(s.clone())),
        Json::Bool(b) => Ok(ArgValue::Num(if *b { 1.0 } else { 0.0 })),
        _ => Err(format!("{} must be a number or a string", what)),
    }
}

fn only_keys(obj: &Json, allowed: &[&str], what: &str) -> Result<(), String> {
    if let Json::Obj(items) = obj {
        for (k, _) in items {
            if !allowed.contains(&k.as_str()) {
                return Err(format!(
                    "{} has an unknown key '{}'. It can have: {}",
                    what,
                    k,
                    allowed.join(", ")
                ));
            }
        }
        Ok(())
    } else {
        Err(format!("{} must be an object", what))
    }
}

const NODE_KEYS: &[&str] = &[
    "block",
    "args",
    "chain",
    "parallel",
    "bypass",
    "mix",
    "only",
    "params",
    "modulators",
];

pub fn node_from_json(j: &Json, path: &str) -> Result<NodeSpec, String> {
    only_keys(j, NODE_KEYS, path)?;
    let bypass = j
        .get("bypass")
        .map(|v| value(v, &format!("{} bypass", path)))
        .transpose()?;
    let mix = j
        .get("mix")
        .map(|v| value(v, &format!("{} mix", path)))
        .transpose()?;
    let only = match j.get("only") {
        None => None,
        Some(v) => {
            let a = v
                .as_array()
                .ok_or_else(|| format!("{} only must be a list of channel numbers", path))?;
            let mut out = Vec::new();
            for c in a {
                let n = c
                    .as_f64()
                    .filter(|n| *n >= 0.0 && n.fract() == 0.0 && *n < 2.0);
                out.push(n.ok_or_else(|| {
                    format!("{} only must list channels 0 (White) or 1 (Black)", path)
                })? as usize);
            }
            Some(out)
        }
    };
    let kinds = ["block", "chain", "parallel"]
        .iter()
        .filter(|k| j.get(k).is_some())
        .count();
    if kinds != 1 {
        return Err(format!(
            "{} must have exactly one of block, chain or parallel",
            path
        ));
    }
    let kind = if let Some(name) = j.get("block") {
        let name = name
            .as_str()
            .ok_or_else(|| format!("{} block must be a name", path))?
            .to_string();
        let mut args = Vec::new();
        if let Some(a) = j.get("args") {
            match a {
                Json::Obj(items) => {
                    for (k, v) in items {
                        args.push((k.clone(), value(v, &format!("{} args.{}", path, k))?));
                    }
                }
                _ => return Err(format!("{} args must be an object", path)),
            }
        }
        NodeKind::Block { name, args }
    } else if let Some(c) = j.get("chain") {
        let items = c
            .as_array()
            .ok_or_else(|| format!("{} chain must be a list of nodes", path))?;
        let mut nodes = Vec::new();
        for (i, n) in items.iter().enumerate() {
            nodes.push(node_from_json(n, &format!("{}.chain[{}]", path, i))?);
        }
        NodeKind::Chain(nodes)
    } else {
        let items = j
            .get("parallel")
            .and_then(|p| p.as_array())
            .ok_or_else(|| format!("{} parallel must be a list of branches", path))?;
        let mut branches = Vec::new();
        for (i, b) in items.iter().enumerate() {
            let bp = format!("{}.parallel[{}]", path, i);
            only_keys(b, &["node", "gain"], &bp)?;
            let node = b
                .get("node")
                .ok_or_else(|| format!("{} needs a node", bp))?;
            let gain = b
                .get("gain")
                .map(|v| value(v, &format!("{} gain", bp)))
                .transpose()?;
            branches.push(BranchSpec {
                node: node_from_json(node, &format!("{}.node", bp))?,
                gain,
            });
        }
        NodeKind::Parallel(branches)
    };
    Ok(NodeSpec {
        kind,
        bypass,
        mix,
        only,
    })
}

pub fn graph_from_json(j: &Json) -> Result<GraphSpec, String> {
    let mut params = Vec::new();
    if let Some(list) = j.get("params") {
        let items = list.as_array().ok_or("params must be a list")?;
        for (i, p) in items.iter().enumerate() {
            let what = format!("params[{}]", i);
            only_keys(p, &["id", "min", "max", "default", "doc"], &what)?;
            let id = p
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("{} needs an id", what))?;
            if id.is_empty()
                || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                || !id.chars().next().unwrap().is_ascii_alphabetic()
            {
                return Err(format!(
                    "{} id '{}' must be a name: letters, digits and _, starting with a letter",
                    what, id
                ));
            }
            let num = |k: &str, d: f32| {
                p.get(k)
                    .map(|v| {
                        v.as_f64()
                            .map(|n| n as f32)
                            .ok_or_else(|| format!("{} {} must be a number", what, k))
                    })
                    .unwrap_or(Ok(d))
            };
            let min = num("min", 0.0)?;
            let max = num("max", 1.0)?;
            let default = num("default", min)?;
            if min >= max {
                return Err(format!("{} min {} must be below max {}", what, min, max));
            }
            if default < min || default > max {
                return Err(format!(
                    "{} default {} is outside {} to {}",
                    what, default, min, max
                ));
            }
            params.push(ParamSpec {
                id: id.to_string(),
                min,
                max,
                default,
                doc: p
                    .get("doc")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            });
        }
    }
    let mut modulators = Vec::new();
    if let Some(list) = j.get("modulators") {
        let items = list.as_array().ok_or("modulators must be a list")?;
        for (i, m) in items.iter().enumerate() {
            let what = format!("modulators[{}]", i);
            only_keys(m, &["id", "shape", "rate", "seed"], &what)?;
            let id = m
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("{} needs an id", what))?;
            let shape = m
                .get("shape")
                .and_then(|v| v.as_str())
                .unwrap_or("sine")
                .to_string();
            if !["sine", "triangle", "saw", "square", "random"].contains(&shape.as_str()) {
                return Err(format!(
                    "{} shape '{}' is not sine, triangle, saw, square or random",
                    what, shape
                ));
            }
            let rate = m
                .get("rate")
                .map(|v| value(v, &format!("{} rate", what)))
                .unwrap_or(Ok(ArgValue::Num(1.0)))?;
            let seed = m.get("seed").and_then(|v| v.as_f64()).unwrap_or(0.0) as u64;
            modulators.push(ModSpec {
                id: id.to_string(),
                shape,
                rate,
                seed,
            });
        }
    }
    let root = node_from_json(j, "the graph")?;
    Ok(GraphSpec {
        params,
        modulators,
        root,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;

    #[test]
    fn a_graph_parses_with_its_params_and_modulators() {
        let g = graph_from_json(&parse(r#"{"params":[{"id":"sharp","min":0,"max":1,"default":1}],
            "modulators":[{"id":"dice","shape":"random","rate":1,"seed":3}],
            "parallel":[{"node":{"block":"material"}},{"node":{"block":"loose","args":{"undefended":"0.5*sharp"},"only":[1]},"gain":"sharp"}]}"#).unwrap()).unwrap();
        assert_eq!(g.params[0].id, "sharp");
        assert_eq!(g.modulators[0].seed, 3);
        match &g.root.kind {
            NodeKind::Parallel(b) => {
                assert_eq!(b.len(), 2);
                assert_eq!(b[1].node.only, Some(vec![1]));
            }
            _ => panic!("the root is a parallel"),
        }
    }

    #[test]
    fn mistakes_say_where() {
        let err = |s: &str| graph_from_json(&parse(s).unwrap()).unwrap_err();
        assert!(
            err(r#"{"chain":[{"block":"material","gain":1}]}"#)
                .contains("the graph.chain[0] has an unknown key 'gain'")
        );
        assert!(
            err(r#"{"chain":[{"block":"material","only":[2]}]}"#)
                .contains("channels 0 (White) or 1 (Black)")
        );
        assert!(err(r#"{"block":"material","chain":[]}"#).contains("exactly one of"));
        assert!(err(r#"{"params":[{"id":"1x"}],"block":"material"}"#).contains("must be a name"));
        assert!(
            err(r#"{"params":[{"id":"a","min":1,"max":0}],"block":"material"}"#)
                .contains("must be below")
        );
    }
}
