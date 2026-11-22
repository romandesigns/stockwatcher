//! The two frozen b-good boosters (XGBoost 3.4.1, `binary:logistic`, `gbtree`, 300 trees each;
//! b-good-1 depth 4 on 56 features, b-good-2 depth 6 on 46), evaluated in pure Rust exactly as
//! XGBoost's CPU predictor does. std only, so the parity harness compiles this file unchanged
//! (`#[path]`, `C:/wavystack/outputs/stockwatcher-research-v2/50-ship/bgood/`).
//!
//! # Why a native evaluator
//!
//! The workspace has no tree-model crate, and the repository already evaluates two frozen
//! LightGBM models natively and bit-exactly (`rocket/burst/model.rs`, `rocket/r2.rs`). The
//! alternatives were weaker (PORT-GUIDE §4): XGBoost FFI crates add the C++ library to the image
//! and are thinly maintained; ONNX through `ort` adds a large native runtime whose TreeEnsemble op
//! does not sum in XGBoost's order; `gbdt` reads XGBoost only through a conversion.
//!
//! # The walk, as XGBoost's `RegTree::GetNext` / CPU predictor
//!
//! - Every feature is a float32 (the research cast its rows with `astype(np.float32)`; the server
//!   rounds its f64 values to nearest with `as f32`, the same rounding).
//! - At a split: a NaN takes the node's `default_left` child; otherwise `x < split_condition` goes
//!   LEFT (XGBoost's rule is strict `<`, unlike LightGBM's `<=`).
//! - The margin is `base_margin`, then `+= leaf` for each tree in tree order, in float32.
//!   `bgood_export.py` proved that this order reproduces `predict(output_margin=True)` bit for bit
//!   on every parity row (it refuses to write the model otherwise), and the tests below prove it
//!   again on the committed vectors.
//!
//! # Deciding in margin space
//!
//! The research flags `predict() >= θ`, a float32 probability. The server never computes a
//! probability for the decision: it compares the float32 margin with `theta_margin`, the smallest
//! float32 margin whose float32 sigmoid is ≥ θ (`bgood_export.py theta_margin`, checked against
//! XGBoost's own predictions). So the platform's `expf` (MSVC here, glibc on the VPS) cannot flip
//! a flag. [`XgbModel::prob`] exists only for display and the ledger.
//!
//! # Files
//!
//! `b-good-N.xgb.txt` beside this file (format in `bgood_export.py`), embedded at compile time,
//! parsed once on first use; `b-good-N.parity.csv` are XGBoost's own margins and probabilities on
//! research feature rows of Train days 2024-06-27/28.

/// One node. `left == -1` marks a leaf whose value is `cond`.
#[derive(Clone, Copy, Debug)]
struct Node {
    left: i32,
    right: i32,
    feat: u16,
    cond: f32,
    default_left: bool,
}

#[derive(Debug)]
pub struct XgbModel {
    pub name: String,
    pub features: Vec<String>,
    pub base_margin: f32,
    /// The research's θ (a float32 probability held in an f64).
    pub theta: f64,
    /// The smallest float32 margin whose probability is ≥ θ: the decision threshold.
    pub theta_margin: f32,
    pub model_sha256: String,
    pub finalist_sha256: String,
    trees: Vec<Vec<Node>>,
}

impl XgbModel {
    /// Parses `bgood_export.py`'s text model. Every structural assumption is checked (children in
    /// range and after their parent, feature index in range), so the walk cannot index out of
    /// bounds or loop.
    pub fn parse(txt: &str) -> Result<XgbModel, String> {
        let mut lines = txt.lines().enumerate();
        let mut next = |want: &str| -> Result<(usize, Vec<String>), String> {
            let (n, l) = lines
                .next()
                .ok_or_else(|| format!("unexpected end, wanted {want}"))?;
            let parts: Vec<String> = l.split_ascii_whitespace().map(str::to_string).collect();
            if parts.first().map(String::as_str) != Some(want) && !want.is_empty() {
                return Err(format!("line {}: wanted {want}, got {l:?}", n + 1));
            }
            Ok((n, parts))
        };
        let f32_of = |s: &str| s.parse::<f32>().map_err(|_| format!("bad float {s:?}"));
        let (_, h) = next("bgood-xgb")?;
        if h.get(1).map(String::as_str) != Some("1") {
            return Err(format!("unknown format version {h:?}"));
        }
        let name = next("name")?.1.get(1).cloned().unwrap_or_default();
        let finalist_sha256 = next("finalist_sha256")?
            .1
            .get(1)
            .cloned()
            .unwrap_or_default();
        let model_sha256 = next("model_sha256")?.1.get(1).cloned().unwrap_or_default();
        let (_, fl) = next("features")?;
        let nf: usize = fl
            .get(1)
            .and_then(|s| s.parse().ok())
            .ok_or("bad feature count")?;
        let features: Vec<String> = fl[2..].to_vec();
        if features.len() != nf || nf == 0 || nf > u16::MAX as usize {
            return Err(format!(
                "feature list holds {} names, header says {nf}",
                features.len()
            ));
        }
        let base_margin = f32_of(next("base_margin")?.1.get(1).ok_or("base_margin")?)?;
        let theta: f64 = next("theta")?
            .1
            .get(1)
            .and_then(|s| s.parse().ok())
            .ok_or("bad theta")?;
        let theta_margin = f32_of(next("theta_margin")?.1.get(1).ok_or("theta_margin")?)?;
        let nt: usize = next("trees")?
            .1
            .get(1)
            .and_then(|s| s.parse().ok())
            .ok_or("bad tree count")?;
        let mut trees = Vec::with_capacity(nt);
        for _ in 0..nt {
            let (ln, th) = next("tree")?;
            let nn: usize = th
                .get(1)
                .and_then(|s| s.parse().ok())
                .ok_or(format!("line {}: bad node count", ln + 1))?;
            let mut nodes = Vec::with_capacity(nn);
            for i in 0..nn {
                let (ln, p) = next("")?;
                let at = |e: String| format!("line {}: {e}", ln + 1);
                if p.len() != 5 {
                    return Err(at(format!("node wants 5 fields, has {}", p.len())));
                }
                let left: i32 = p[0].parse().map_err(|_| at("left".into()))?;
                let right: i32 = p[1].parse().map_err(|_| at("right".into()))?;
                let feat: usize = p[2].parse().map_err(|_| at("feature".into()))?;
                let cond = f32_of(&p[3]).map_err(at)?;
                let default_left = match p[4].as_str() {
                    "0" => false,
                    "1" => true,
                    o => return Err(at(format!("default_left {o:?}"))),
                };
                if left == -1 {
                    if right != -1 {
                        return Err(at("leaf with a right child".into()));
                    }
                } else {
                    let ok = |c: i32| c as usize > i && (c as usize) < nn && c > 0;
                    if !ok(left) || !ok(right) || feat >= nf {
                        return Err(at(format!("bad split {left} {right} {feat}")));
                    }
                }
                nodes.push(Node {
                    left,
                    right,
                    feat: feat.min(u16::MAX as usize) as u16,
                    cond,
                    default_left,
                });
            }
            if nodes.is_empty() {
                return Err(format!("line {}: empty tree", ln + 1));
            }
            trees.push(nodes);
        }
        Ok(XgbModel {
            name,
            features,
            base_margin,
            theta,
            theta_margin,
            model_sha256,
            finalist_sha256,
            trees,
        })
    }

    pub fn n_features(&self) -> usize {
        self.features.len()
    }

    /// The float32 margin (log-odds) of one row in the model's feature order; `x.len()` must be
    /// [`Self::n_features`] (a shorter row reads missing values as NaN).
    pub fn margin(&self, x: &[f32]) -> f32 {
        let mut acc = self.base_margin;
        for t in &self.trees {
            let mut i = 0usize;
            loop {
                let n = t[i];
                if n.left == -1 {
                    acc += n.cond;
                    break;
                }
                let v = x.get(n.feat as usize).copied().unwrap_or(f32::NAN);
                let left = if v.is_nan() {
                    n.default_left
                } else {
                    v < n.cond
                };
                i = if left { n.left } else { n.right } as usize;
            }
        }
        acc
    }

    /// The decision: margin ≥ `theta_margin` (module docs).
    pub fn passes(&self, margin: f32) -> bool {
        margin >= self.theta_margin
    }

    /// The float32 probability, `1 / (1 + exp(-m))` as XGBoost's `common::Sigmoid`. For display
    /// and the ledger only: the platform's `expf` may differ by an ulp, which is why decisions use
    /// [`Self::passes`].
    pub fn prob(margin: f32) -> f32 {
        1.0f32 / (1.0f32 + (-margin).exp())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(csv: &str, nf: usize) -> Vec<(Vec<f32>, f32, f32)> {
        let mut it = csv.lines();
        let head: Vec<&str> = it.next().unwrap().split(',').collect();
        assert_eq!(head.len(), nf + 2);
        assert_eq!(head[nf], "xgb_margin");
        it.map(|l| {
            let v: Vec<f32> = l
                .split(',')
                .map(|s| {
                    if s == "nan" {
                        f32::NAN
                    } else {
                        s.parse().unwrap()
                    }
                })
                .collect();
            (v[..nf].to_vec(), v[nf], v[nf + 1])
        })
        .collect()
    }

    fn check(model: &str, csv: &str, want_rows: usize) {
        let m = XgbModel::parse(model).unwrap();
        assert_eq!(m.trees.len(), 300);
        let r = rows(csv, m.n_features());
        assert_eq!(r.len(), want_rows);
        let theta = m.theta;
        let mut flagged = 0;
        for (i, (x, want_m, want_p)) in r.iter().enumerate() {
            let got = m.margin(x);
            assert_eq!(
                got.to_bits(),
                want_m.to_bits(),
                "{} row {i}: margin {got} vs XGBoost {want_m}",
                m.name
            );
            // The flag in margin space equals XGBoost's own `predict() >= θ`.
            assert_eq!(
                m.passes(got),
                (*want_p as f64) >= theta,
                "{} row {i}",
                m.name
            );
            flagged += m.passes(got) as usize;
        }
        eprintln!(
            "{}: {} rows bit-identical, {flagged} flagged",
            m.name,
            r.len()
        );
    }

    #[test]
    fn b_good_1_margins_bit_identical_to_xgboost() {
        check(
            include_str!("b-good-1.xgb.txt"),
            include_str!("b-good-1.parity.csv"),
            317,
        );
    }

    #[test]
    fn b_good_2_margins_bit_identical_to_xgboost() {
        check(
            include_str!("b-good-2.xgb.txt"),
            include_str!("b-good-2.parity.csv"),
            651,
        );
    }

    #[test]
    fn theta_margin_is_the_boundary() {
        for txt in [
            include_str!("b-good-1.xgb.txt"),
            include_str!("b-good-2.xgb.txt"),
        ] {
            let m = XgbModel::parse(txt).unwrap();
            let below = f32::from_bits(if m.theta_margin < 0.0 {
                m.theta_margin.to_bits() + 1
            } else {
                m.theta_margin.to_bits() - 1
            });
            assert!(below < m.theta_margin);
            assert!(m.passes(m.theta_margin) && !m.passes(below));
            // The sigmoid agrees here to well under an ulp of θ (display only).
            assert!((XgbModel::prob(m.theta_margin) as f64 - m.theta).abs() < 1e-6);
        }
    }

    #[test]
    fn rejects_broken_models() {
        let good = include_str!("b-good-1.xgb.txt");
        assert!(XgbModel::parse(&good.replacen("bgood-xgb 1", "bgood-xgb 2", 1)).is_err());
        assert!(XgbModel::parse(&good[..good.len() / 2]).is_err());
        let loop_ = good.replacen("\n1 2 52 ", "\n0 2 52 ", 1);
        assert!(XgbModel::parse(&loop_).is_err());
    }
}
