// SPDX-License-Identifier: GPL-3.0-or-later
//! Incremental all-pairs enrollment. Retains independent templates; no fusion.
use super::{Enrollment, FingerprintTemplate, RecognitionEngine};
use anyhow::{Context, Result, ensure};
use std::time::{Duration, Instant};

/// Explicit sample-count and similarity policy. Scores are SourceAFIS scores.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnrollmentConfig {
    pub min_samples: usize,
    pub max_samples: usize,
    pub consistency_threshold: f64,
}

/// A snapshot of committed enrollment state, including a measured pair minimum.
/// One accepted sample has no pair score; readiness requires the configured minimum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnrollmentProgress {
    pub accepted_samples: usize,
    pub required_samples: usize,
    pub maximum_samples: usize,
    pub ready: bool,
    pub at_capacity: bool,
    pub minimum_pair_score: Option<f64>,
}

/// A retryable mismatch; process and validation failures remain separate errors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnrollmentRejection {
    pub score: f64,
    pub threshold: f64,
}
impl std::fmt::Display for EnrollmentRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "inconsistent enrollment samples: score {} is below {}",
            self.score, self.threshold
        )
    }
}
impl std::error::Error for EnrollmentRejection {}

/// Owns accepted template copies only. No persistent process, device, or worker
/// is retained. Each add finishes its helper processes before returning.
#[derive(Debug)]
pub struct EnrollmentSession {
    config: EnrollmentConfig,
    samples: Vec<FingerprintTemplate>,
    minimum_pair_score: Option<f64>,
}

impl EnrollmentSession {
    pub fn new(config: EnrollmentConfig) -> Result<Self> {
        ensure!(
            (2..=10).contains(&config.min_samples)
                && (config.min_samples..=10).contains(&config.max_samples),
            "enrollment sample bounds require 2 <= minimum <= maximum <= 10"
        );
        ensure!(
            config.consistency_threshold.is_finite() && config.consistency_threshold > 0.0,
            "explicit positive finite threshold required"
        );
        Ok(Self {
            config,
            samples: Vec::with_capacity(config.max_samples),
            minimum_pair_score: None,
        })
    }

    pub fn progress(&self) -> EnrollmentProgress {
        EnrollmentProgress {
            accepted_samples: self.samples.len(),
            required_samples: self.config.min_samples,
            maximum_samples: self.config.max_samples,
            ready: self.samples.len() >= self.config.min_samples,
            at_capacity: self.samples.len() >= self.config.max_samples,
            minimum_pair_score: self.minimum_pair_score,
        }
    }

    /// Validate and compare the proposed sample against every accepted sample,
    /// under one engine timeout shared across this add. Existing samples remain
    /// the probes, matching batch enroll's insertion-order pair orientation.
    /// No session state changes until every validation/comparison succeeds.
    /// The first sample is format/provenance-validated without launching a helper.
    pub fn add(
        &mut self,
        engine: &RecognitionEngine,
        sample: &FingerprintTemplate,
    ) -> Result<EnrollmentProgress> {
        let started = Instant::now();
        ensure!(
            self.samples.len() < self.config.max_samples,
            "enrollment is full"
        );
        ensure!(
            engine.timeout > Duration::ZERO && engine.timeout <= Duration::from_secs(300),
            "invalid engine deadline"
        );
        engine.validate(sample)?;
        let mut minimum = self.minimum_pair_score;
        for accepted in &self.samples {
            let remaining = engine.timeout.saturating_sub(started.elapsed());
            ensure!(!remaining.is_zero(), "enrollment timed out");
            let bounded = RecognitionEngine {
                executable: engine.executable.clone(),
                timeout: remaining,
            };
            let scores = bounded.comparison_scores(
                accepted,
                std::slice::from_ref(sample),
                self.config.consistency_threshold,
            )?;
            let score = *scores
                .first()
                .context("missing enrollment comparison score")?;
            if score < self.config.consistency_threshold {
                return Err(EnrollmentRejection {
                    score,
                    threshold: self.config.consistency_threshold,
                }
                .into());
            }
            minimum = Some(minimum.map_or(score, |previous| previous.min(score)));
        }
        self.samples.push(sample.clone());
        self.minimum_pair_score = minimum;
        Ok(self.progress())
    }

    /// Clear all accepted samples and measured scores while retaining the policy.
    pub fn reset(&mut self) {
        self.samples.clear();
        self.minimum_pair_score = None;
    }

    /// Consume a ready session, retaining the existing multi-template policy.
    /// Each sample remains separate; no fused/vendor FMR is synthesized.
    pub fn finish(self) -> Result<Enrollment> {
        ensure!(
            self.samples.len() >= self.config.min_samples,
            "enrollment is not ready"
        );
        Ok(Enrollment {
            schema_version: 1,
            policy: "all_pairs_consistent_multi_template_not_vendor_fusion".into(),
            samples: self.samples,
            consistency_threshold: self.config.consistency_threshold,
            minimum_pair_score: self
                .minimum_pair_score
                .context("enrollment lacks pair measurements")?,
        })
    }
}
