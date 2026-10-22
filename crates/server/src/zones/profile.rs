//! The volume-at-price profile and its shelves: `find_zones` of the research's
//! `scripts/research/zones_volume_profile.py` (branch `research/zones-volume-profile`, frozen
//! `178c4ab`), line for line, in the same f64 arithmetic so the bins and edges are bit-identical.

/// Variant C's parameters (`VARIANTS["C"]`).
pub const K: f64 = 1.5;
pub const SHARE: f64 = 0.05;
pub const FRAC: f64 = 0.5;
/// Bin width as a fraction of the first run's start price, rounded UP to whole ticks.
pub const R: f64 = 0.01;
/// A profile never grows past this many bins (~160 KB). A run's bins are ~1 % of its start price,
/// so a stock that went 10x in a day needs ~1,000; past the cap a print outside is dropped.
pub const MAX_BINS: usize = 20_000;

/// `tick * max(1, ceil(r * p0 / tick - 1e-9))`, tick = $0.0001 below $1, $0.01 at and above:
/// linear, tick-aligned bins never alias (research comment).
pub fn bin_width(p0: f64) -> f64 {
    let tick = if p0 < 1.0 { 0.0001 } else { 0.01 };
    let n = (R * p0 / tick - 1e-9).ceil().max(1.0);
    tick * n
}

/// `smooth`: the [1,2,1]/4 kernel, the ends reflected as the research does.
fn smooth(x: &[f64]) -> Vec<f64> {
    let n = x.len();
    if n < 3 {
        return x.to_vec();
    }
    let mut y = vec![0.0; n];
    for i in 1..n - 1 {
        y[i] = (x[i - 1] + 2.0 * x[i] + x[i + 1]) / 4.0;
    }
    y[0] = (2.0 * x[0] + x[1]) / 4.0;
    y[n - 1] = (x[n - 2] + 2.0 * x[n - 1]) / 4.0;
    y
}

/// The volume profile from the run's anchor: raw volume per bin, `base` = the bin index of
/// `bins[0]`. Its occupied range is always the whole vector (it only grows to fit a print).
#[derive(Clone, Debug, Default)]
pub struct Profile {
    pub width: f64,
    base: i64,
    bins: Vec<f64>,
}

impl Profile {
    pub fn new(width: f64) -> Self {
        Profile {
            width,
            base: 0,
            bins: Vec::new(),
        }
    }

    /// `np.floor(p / width)`.
    pub fn bin_of(&self, price: f64) -> i64 {
        (price / self.width).floor() as i64
    }

    /// Adds `size` at `price`. Returns false when the print fell outside [`MAX_BINS`].
    pub fn add(&mut self, price: f64, size: f64) -> bool {
        let b = self.bin_of(price);
        if self.bins.is_empty() {
            self.base = b;
            self.bins.push(size);
            return true;
        }
        if b < self.base {
            let grow = (self.base - b) as usize;
            if self.bins.len() + grow > MAX_BINS {
                return false;
            }
            let mut v = vec![0.0; grow];
            v.extend_from_slice(&self.bins);
            self.bins = v;
            self.base = b;
        }
        let i = (b - self.base) as usize;
        if i >= self.bins.len() {
            if i + 1 > MAX_BINS {
                return false;
            }
            self.bins.resize(i + 1, 0.0);
        }
        self.bins[i] += size;
        true
    }

    pub fn bytes(&self) -> usize {
        self.bins.capacity() * 8
    }

    /// `find_zones(prof, lo_i, hi_i, prm, width)`: `(zone_lo, zone_hi, share)` per shelf.
    pub fn zones(&self) -> Vec<(f64, f64, f64)> {
        let seg = &self.bins;
        let lo_i = self.base;
        let tot: f64 = seg.iter().sum();
        if tot <= 0.0 || seg.len() < 3 {
            return Vec::new();
        }
        let s = smooth(seg);
        let (mut sum, mut n) = (0.0, 0usize);
        for (i, &v) in s.iter().enumerate() {
            if seg[i] > 0.0 {
                sum += v;
                n += 1;
            }
        }
        let mean = sum / n as f64;
        let len = s.len();
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for i in 0..len {
            let left = if i == 0 { -1.0 } else { s[i - 1] };
            let right = if i + 1 == len { -1.0 } else { s[i + 1] };
            // Local maxima, a plateau taking its first bin.
            if !(s[i] >= left && s[i] > right && s[i] >= K * mean) {
                continue;
            }
            let thr = FRAC * s[i];
            let mut a = i;
            while a > 0 && s[a - 1] >= thr {
                a -= 1;
            }
            let mut b = i;
            while b < len - 1 && s[b + 1] >= thr {
                b += 1;
            }
            spans.push((a, b));
        }
        spans.sort();
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for (a, b) in spans {
            match merged.last_mut() {
                // Overlapping or within one empty bin: one shelf.
                Some(m) if a <= m.1 + 2 => m.1 = m.1.max(b),
                _ => merged.push((a, b)),
            }
        }
        let mut out = Vec::new();
        for (a, b) in merged {
            let sh = seg[a..=b].iter().sum::<f64>() / tot;
            if sh >= SHARE {
                out.push((
                    (lo_i + a as i64) as f64 * self.width,
                    (lo_i + b as i64 + 1) as f64 * self.width,
                    sh,
                ));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_follow_the_frozen_rule() {
        // $1.5001 -> 0.015001/0.01 = 1.5 ticks -> 2 ticks.
        assert!((bin_width(1.5001) - 0.02).abs() < 1e-12);
        // Below $1 the tick is $0.0001: $0.358 -> 35.8 -> 36 ticks.
        assert!((bin_width(0.358) - 0.0036).abs() < 1e-12);
        // Never under one tick.
        assert!((bin_width(0.5) - 0.005).abs() < 1e-12);
        assert!((bin_width(0.20) - 0.002).abs() < 1e-12);
        assert!((bin_width(3.2016) - 0.04).abs() < 1e-12);
    }

    #[test]
    fn a_single_heavy_shelf_is_found_and_thin_noise_is_not() {
        let mut p = Profile::new(0.01);
        for (px, v) in [(1.00, 100.0), (1.01, 100.0), (1.02, 5000.0), (1.03, 4000.0)] {
            p.add(px, v);
        }
        for i in 4..30 {
            p.add(1.0 + i as f64 * 0.01, 100.0);
        }
        let z = p.zones();
        assert_eq!(z.len(), 1, "{z:?}");
        let (lo, hi, sh) = z[0];
        assert!(lo <= 1.02 + 1e-9 && hi >= 1.04 - 1e-9, "{z:?}");
        assert!(sh > 0.5);
    }

    #[test]
    fn the_profile_grows_both_ways_and_stops_at_its_cap() {
        let mut p = Profile::new(0.01);
        assert!(p.add(2.0, 1.0));
        assert!(p.add(1.5, 1.0));
        assert!(p.add(2.5, 1.0));
        assert_eq!(p.bins.len(), 101);
        assert!(!p.add(1000.0, 1.0), "past MAX_BINS");
    }
}
