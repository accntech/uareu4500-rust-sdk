// SPDX-License-Identifier: GPL-3.0-or-later
//! In-memory workflow for the Fingerprint Studio example.
use anyhow::{Context, Result, bail, ensure};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

pub mod layout;
pub mod theme;
pub mod workflow;
use uareu::{
    metadata::{CapturedImage, Resolution},
    recognition::{Enrollment, FingerprintTemplate, MatchResult, RecognitionEngine},
};

#[derive(Clone, Debug)]
pub struct Config {
    pub engine: Option<PathBuf>,
    pub ppi: Option<f64>,
    pub threshold: f64,
    pub experimental_ppi: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            engine: None,
            ppi: None,
            threshold: 40.0,
            experimental_ppi: false,
        }
    }
}
impl Config {
    pub fn parse_args(args: impl IntoIterator<Item = impl Into<String>>) -> Result<Self> {
        let mut config = Self::default();
        let mut args = args.into_iter().map(Into::into);
        while let Some(flag) = args.next() {
            let value = args
                .next()
                .with_context(|| format!("missing value for {flag}"))?;
            match flag.as_str() {
                "--engine" => {
                    ensure!(!value.is_empty(), "engine path is empty");
                    config.engine = Some(value.into());
                }
                "--ppi" | "--experimental-ppi" => {
                    ensure!(config.ppi.is_none(), "Choose only one PPI declaration.");
                    config.experimental_ppi = flag == "--experimental-ppi";
                    let ppi: f64 = value.parse()?;
                    ensure!(
                        ppi.is_finite() && (250.0..=2000.0).contains(&ppi),
                        "PPI must be between 250 and 2000"
                    );
                    config.ppi = Some(ppi);
                }
                "--threshold" => {
                    let score: f64 = value.parse()?;
                    ensure!(
                        score.is_finite() && score > 0.0,
                        "threshold must be positive and finite"
                    );
                    config.threshold = score;
                }
                _ => bail!("unknown option: {flag}"),
            }
        }
        Ok(config)
    }
    pub fn scale_label(&self) -> String {
        self.ppi.map_or("Scale not declared".into(), |ppi| {
            if self.experimental_ppi {
                format!("{ppi} PPI · nominal, unverified")
            } else {
                format!("{ppi} PPI · caller declared")
            }
        })
    }
    pub fn recognition_enabled(&self) -> bool {
        self.engine.is_some() && self.ppi.is_some()
    }
    fn recognition_engine(&self) -> Result<RecognitionEngine> {
        ensure!(
            self.ppi.is_some(),
            "Restart with --ppi using a confirmed reader pixel scale."
        );
        Ok(RecognitionEngine {
            executable: self
                .engine
                .clone()
                .context("Restart with --engine pointing to the local SourceAFIS helper.")?,
            timeout: Duration::from_secs(15),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SampleAcceptance {
    Accepted(usize),
    Rejected { score: f64, threshold: f64 },
}

pub struct Session {
    pub capture: Option<CapturedImage>,
    config: Config,
    samples: Vec<FingerprintTemplate>,
    enrollment: Option<Enrollment>,
    sample_available: bool,
    minimum_pair_score: Option<f64>,
}
impl Session {
    pub fn new(config: Config) -> Self {
        Self {
            capture: None,
            config,
            samples: Vec::new(),
            enrollment: None,
            sample_available: false,
            minimum_pair_score: None,
        }
    }
    pub fn reset(&mut self) {
        self.capture = None;
        self.samples.clear();
        self.minimum_pair_score = None;
        self.enrollment = None;
        self.sample_available = false;
    }
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }
    pub fn enrolled(&self) -> bool {
        self.enrollment.is_some()
    }
    pub fn begin_capture(&mut self) {
        self.capture = None;
        self.sample_available = false;
    }
    pub fn set_capture(&mut self, mut capture: CapturedImage) -> Result<()> {
        capture.metadata.validate(&capture.image)?;
        if let Some(ppi) = self.config.ppi {
            capture.metadata.resolution = Resolution::declared(ppi, ppi)?;
        }
        self.capture = Some(capture);
        self.sample_available = true;
        Ok(())
    }
    pub fn add_sample(&mut self) -> Result<usize> {
        match self.try_add_sample()? {
            SampleAcceptance::Accepted(count) => Ok(count),
            SampleAcceptance::Rejected { score, threshold } => bail!(
                "Inconsistent impression: score {score:.1} is below {threshold:.1}. Lift and try the same finger again."
            ),
        }
    }
    /// Add one independent acquisition. A mismatch is retryable; helper failures
    /// remain errors. Accepted samples are committed only after all comparisons.
    pub fn try_add_sample(&mut self) -> Result<SampleAcceptance> {
        ensure!(
            self.sample_available,
            "Capture a new impression before adding a sample."
        );
        ensure!(
            self.samples.len() < 3,
            "Enrollment complete. Clear the session to enroll another finger."
        );
        let started = Instant::now();
        let engine = self.config.recognition_engine()?;
        let sample = engine.extract(
            self.capture
                .as_ref()
                .context("Capture a fingerprint first.")?,
        )?;
        let mut minimum = self.minimum_pair_score;
        for accepted in &self.samples {
            let remaining = engine.timeout.saturating_sub(started.elapsed());
            ensure!(!remaining.is_zero(), "Enrollment timed out.");
            let bounded = RecognitionEngine {
                executable: engine.executable.clone(),
                timeout: remaining,
            };
            let result = bounded.compare(accepted, &sample, self.config.threshold)?;
            if !result.matched {
                self.sample_available = false;
                return Ok(SampleAcceptance::Rejected {
                    score: result.score,
                    threshold: result.threshold,
                });
            }
            minimum = Some(minimum.map_or(result.score, |previous| previous.min(result.score)));
        }
        self.samples.push(sample);
        self.minimum_pair_score = minimum;
        if self.samples.len() == 3 {
            self.enrollment = Some(Enrollment {
                schema_version: 1,
                policy: "all_pairs_consistent_multi_template_not_vendor_fusion".into(),
                samples: self.samples.clone(),
                consistency_threshold: self.config.threshold,
                minimum_pair_score: minimum.context("Enrollment lacks pair measurements")?,
            });
        }
        self.sample_available = false;
        Ok(SampleAcceptance::Accepted(self.samples.len()))
    }
    pub fn verify(&mut self) -> Result<MatchResult> {
        ensure!(
            self.sample_available,
            "Capture a fresh impression to verify."
        );
        let engine = self.config.recognition_engine()?;
        let enrollment = self
            .enrollment
            .as_ref()
            .context("Enroll three impressions first.")?;
        let probe = engine.extract(
            self.capture
                .as_ref()
                .context("Capture a fingerprint first.")?,
        )?;
        let result = engine.verify_enrollment(&probe, enrollment, self.config.threshold)?;
        self.sample_available = false;
        Ok(result)
    }
}
