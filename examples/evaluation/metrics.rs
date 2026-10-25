// SPDX-License-Identifier: GPL-3.0-or-later
use anyhow::{Result, ensure};
use serde::Serialize;

#[derive(Clone, Debug)]
pub struct Comparison {
    pub subject_a: usize,
    pub subject_b: usize,
    pub genuine: bool,
    pub score: f64,
}
#[derive(Clone, Debug, Serialize)]
pub struct Rates {
    pub genuine_weight: f64,
    pub impostor_weight: f64,
    pub fmr: Option<f64>,
    pub fnmr: Option<f64>,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Bootstrap {
    pub replicates: usize,
    pub defined_fmr: usize,
    pub defined_fnmr: usize,
    pub fmr_interval: Option<[f64; 2]>,
    pub fnmr_interval: Option<[f64; 2]>,
    pub reason: String,
}
pub fn rates(scores: &[Comparison], threshold: f64, weights: &[u32]) -> Result<Rates> {
    ensure!(
        threshold.is_finite() && threshold > 0.0,
        "invalid threshold"
    );
    let (mut genuine, mut impostor, mut false_match, mut false_nonmatch) = (0.0, 0.0, 0.0, 0.0);
    for score in scores {
        ensure!(
            score.score.is_finite() && score.score >= 0.0,
            "invalid score"
        );
        ensure!(
            score.subject_a < weights.len() && score.subject_b < weights.len(),
            "subject index outside weights"
        );
        ensure!(
            !score.genuine || score.subject_a == score.subject_b,
            "genuine comparison must have one subject"
        );
        ensure!(
            score.genuine || score.subject_a != score.subject_b,
            "impostor comparison must cross subjects"
        );
        let a = f64::from(weights[score.subject_a]);
        if score.genuine {
            genuine += a;
            if score.score < threshold {
                false_nonmatch += a;
            }
        } else {
            let weight = a * f64::from(weights[score.subject_b]);
            impostor += weight;
            if score.score >= threshold {
                false_match += weight;
            }
        }
    }
    Ok(Rates {
        genuine_weight: genuine,
        impostor_weight: impostor,
        fmr: (impostor > 0.0).then(|| false_match / impostor),
        fnmr: (genuine > 0.0).then(|| false_nonmatch / genuine),
    })
}
pub fn threshold_grid(scores: &[f64]) -> Result<Vec<f64>> {
    ensure!(
        scores.iter().all(|s| s.is_finite() && *s >= 0.0),
        "invalid development score"
    );
    let mut positive: Vec<_> = scores.iter().copied().filter(|s| *s > 0.0).collect();
    ensure!(!positive.is_empty(), "no positive development scores");
    positive.sort_by(f64::total_cmp);
    let mut grid: Vec<_> = (0..=100)
        .map(|p| positive[p * (positive.len() - 1) / 100])
        .collect();
    grid.dedup();
    let maximum = *positive.last().unwrap();
    let above = f64::from_bits(maximum.to_bits() + 1);
    ensure!(
        above.is_finite(),
        "maximum score has no finite upper endpoint"
    );
    grid.push(above);
    Ok(grid)
}
fn interval(values: &mut [f64]) -> Option<[f64; 2]> {
    if values.len() < 1900 {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let last = values.len() - 1;
    Some([values[last * 25 / 1000], values[last * 975 / 1000]])
}
pub fn bootstrap(
    scores: &[Comparison],
    threshold: f64,
    subjects: usize,
    seed: u64,
) -> Result<Bootstrap> {
    // Validate observations even when there are too few subjects for intervals.
    let _ = rates(scores, threshold, &vec![1; subjects])?;
    let mut result = Bootstrap {
        replicates: 0,
        defined_fmr: 0,
        defined_fnmr: 0,
        fmr_interval: None,
        fnmr_interval: None,
        reason: "fewer than 10 held-out subjects; descriptive results only".into(),
    };
    if subjects < 10 {
        return Ok(result);
    }
    let (mut fmr, mut fnmr) = (Vec::new(), Vec::new());
    let mut state = seed;
    for _ in 0..2000 {
        let mut weights = vec![0u32; subjects];
        for _ in 0..subjects {
            // SplitMix64: deterministic resampling, not a security primitive.
            state = state.wrapping_add(0x9e3779b97f4a7c15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
            z ^= z >> 31;
            weights[((u128::from(z) * subjects as u128) >> 64) as usize] += 1;
        }
        let sample = rates(scores, threshold, &weights)?;
        if let Some(v) = sample.fmr {
            fmr.push(v);
        }
        if let Some(v) = sample.fnmr {
            fnmr.push(v);
        }
    }
    result.replicates = 2000;
    result.defined_fmr = fmr.len();
    result.defined_fnmr = fnmr.len();
    result.fmr_interval = interval(&mut fmr);
    result.fnmr_interval = interval(&mut fnmr);
    result.reason = "exploratory 95% subject-cluster bootstrap; zero errors/zero width do not prove zero population error; intervals withheld below 1900 defined replicates".into();
    Ok(result)
}
