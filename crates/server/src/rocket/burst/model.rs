//! The frozen burst model `V3-5`: a LightGBM 4.7.0 binary classifier (300 trees, ≤ 15 leaves,
//! depth ≤ 4) on the 34 burst features ([`super::FEATS`]), evaluated in pure Rust exactly as
//! LightGBM's `Booster::predict` does.
//!
//! # Why not the R2 evaluator's format
//!
//! `super::super::r2` reads a converted text format with no missing-value rule: the R2 model was
//! trained on median-imputed rows, so LightGBM recorded `missing_type=None` everywhere and NaN
//! never reaches a tree. The burst model was trained with **native missing values** (protocol §6,
//! "missing native"): 2,390 of its splits route NaN by a learned default direction. So this
//! parser reads LightGBM's own model text (`v3_5.lgb.txt`, the frozen file) including each
//! split's `decision_type`, and the walk below is LightGBM's `Tree::NumericalDecision`:
//!
//! - `missing_type = (decision_type >> 2) & 3` (0 None, 1 Zero, 2 NaN), `default_left = bit 1`;
//! - a NaN with `missing_type != NaN` becomes `0.0`;
//! - `Zero` and `|x| ≤ 1e-35`, or `NaN` and NaN → the default child;
//! - else `x <= threshold` → left.
//!
//! The raw score is `0.0 + tree₀ + … + tree₂₉₉` in tree order (`GBDT::PredictRaw`), the
//! probability `1 / (1 + exp(−raw))` (`objective=binary sigmoid:1`), which is what the research's
//! `bst.predict` returned and what the frozen thresholds (`frozen.json`) are in.
//!
//! Thresholds and leaf values are written by LightGBM with 17 significant digits and parsed here
//! with `str::parse::<f64>` (correctly rounded), so every number is the frozen double.
//!
//! # Provenance
//!
//! `v3_5.lgb.txt` is `docs/research/burst-onset/V3-5.lgb.txt` of branch `research/burst-onset`
//! (frozen at `d0d2156`). The frozen copy on the research disk has sha256 `d337f78a…`; the
//! repository copy differs only in two carriage returns on its last line (`pandas_categorical`,
//! normalised by `.gitattributes`), which the parser ignores.

use std::sync::OnceLock;

use super::NF;

const MODEL_TXT: &str = include_str!("v3_5.lgb.txt");

#[derive(Clone, Copy, Debug)]
struct Node {
    feat: u16,
    thr: f64,
    /// `missing_type`: 0 None, 1 Zero, 2 NaN.
    missing: u8,
    default_left: bool,
    left: i32,
    right: i32,
}

#[derive(Debug)]
struct Tree {
    nodes: Vec<Node>,
    leaves: Vec<f64>,
}

#[derive(Debug)]
pub struct BurstModel {
    trees: Vec<Tree>,
    pub max_feature_idx: usize,
}

fn nums<T: std::str::FromStr>(v: &str, what: &str) -> Result<Vec<T>, String> {
    v.split_ascii_whitespace()
        .map(|s| {
            s.parse::<T>()
                .map_err(|_| format!("{what}: bad number {s:?}"))
        })
        .collect()
}

/// LightGBM's `kZeroThreshold`.
const ZERO: f64 = 1e-35;

impl BurstModel {
    /// Parses LightGBM's model text (only what prediction needs; everything is checked so the
    /// walk cannot index out of bounds or loop).
    pub fn parse(txt: &str) -> Result<BurstModel, String> {
        let mut trees = Vec::new();
        let mut max_feature_idx = usize::MAX;
        let mut objective_ok = false;
        // The tree being read: key → value.
        let mut cur: Option<std::collections::HashMap<&str, &str>> = None;
        let finish = |m: std::collections::HashMap<&str, &str>| -> Result<Tree, String> {
            let get = |k: &str| m.get(k).copied().ok_or_else(|| format!("tree without {k}"));
            let nl: usize = get("num_leaves")?
                .parse()
                .map_err(|_| "num_leaves".to_string())?;
            if get("num_cat")? != "0" {
                return Err("categorical splits are not supported".into());
            }
            let leaves: Vec<f64> = nums(get("leaf_value")?, "leaf_value")?;
            if leaves.len() != nl || nl == 0 {
                return Err(format!("{} leaf values for {nl} leaves", leaves.len()));
            }
            if nl == 1 {
                return Ok(Tree {
                    nodes: Vec::new(),
                    leaves,
                });
            }
            let f: Vec<u16> = nums(get("split_feature")?, "split_feature")?;
            let t: Vec<f64> = nums(get("threshold")?, "threshold")?;
            let d: Vec<u8> = nums(get("decision_type")?, "decision_type")?;
            let l: Vec<i32> = nums(get("left_child")?, "left_child")?;
            let r: Vec<i32> = nums(get("right_child")?, "right_child")?;
            let k = nl - 1;
            if [f.len(), t.len(), d.len(), l.len(), r.len()]
                .iter()
                .any(|&n| n != k)
            {
                return Err("tree arrays of different lengths".into());
            }
            let ok = |c: i32, from: usize| {
                if c < 0 {
                    ((-c - 1) as usize) < nl
                } else {
                    // Children come after their parent, so a walk ends.
                    (c as usize) > from && (c as usize) < k
                }
            };
            let mut nodes = Vec::with_capacity(k);
            for i in 0..k {
                if !ok(l[i], i) || !ok(r[i], i) || !t[i].is_finite() || d[i] & 1 != 0 {
                    return Err(format!("tree node {i} is malformed"));
                }
                if usize::from(f[i]) >= NF {
                    return Err(format!("split on feature {} of {NF}", f[i]));
                }
                nodes.push(Node {
                    feat: f[i],
                    thr: t[i],
                    missing: (d[i] >> 2) & 3,
                    default_left: d[i] & 2 != 0,
                    left: l[i],
                    right: r[i],
                });
            }
            Ok(Tree { nodes, leaves })
        };
        for line in txt.lines() {
            let line = line.trim_end_matches('\r');
            if line == "end of trees" {
                break;
            }
            if line.starts_with("Tree=") {
                if let Some(m) = cur.take() {
                    trees.push(finish(m)?);
                }
                cur = Some(Default::default());
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            match cur.as_mut() {
                Some(m) => {
                    m.insert(k, v);
                }
                None => match k {
                    "max_feature_idx" => {
                        max_feature_idx = v.parse().map_err(|_| "max_feature_idx")?;
                    }
                    "objective" => objective_ok = v == "binary sigmoid:1",
                    "num_class" | "num_tree_per_iteration" if v != "1" => {
                        return Err(format!("{k}={v}: only one tree per iteration"));
                    }
                    _ => {}
                },
            }
        }
        if let Some(m) = cur.take() {
            trees.push(finish(m)?);
        }
        if !objective_ok {
            return Err("objective is not `binary sigmoid:1`".into());
        }
        if max_feature_idx + 1 != NF {
            return Err(format!(
                "model has {} features, not {NF}",
                max_feature_idx + 1
            ));
        }
        if trees.is_empty() {
            return Err("no trees".into());
        }
        Ok(BurstModel {
            trees,
            max_feature_idx,
        })
    }

    /// The frozen model (parsed once).
    pub fn frozen() -> &'static BurstModel {
        static M: OnceLock<BurstModel> = OnceLock::new();
        M.get_or_init(|| BurstModel::parse(MODEL_TXT).expect("frozen burst model parses"))
    }

    pub fn trees(&self) -> usize {
        self.trees.len()
    }

    /// The raw score (log-odds). `x` in [`super::FEATS`] order, `NaN` = missing.
    pub fn raw(&self, x: &[f64; NF]) -> f64 {
        let mut z = 0.0;
        for t in &self.trees {
            if t.nodes.is_empty() {
                z += t.leaves[0];
                continue;
            }
            let mut node = 0usize;
            loop {
                let s = t.nodes[node];
                let mut v = x[usize::from(s.feat)];
                if v.is_nan() && s.missing != 2 {
                    v = 0.0;
                }
                let next = if (s.missing == 1 && (-ZERO..=ZERO).contains(&v))
                    || (s.missing == 2 && v.is_nan())
                {
                    if s.default_left { s.left } else { s.right }
                } else if v <= s.thr {
                    s.left
                } else {
                    s.right
                };
                if next < 0 {
                    z += t.leaves[(-next - 1) as usize];
                    break;
                }
                node = next as usize;
            }
        }
        z
    }

    /// The probability the research thresholds are in.
    pub fn predict(&self, x: &[f64; NF]) -> f64 {
        1.0 / (1.0 + (-self.raw(x)).exp())
    }
}
