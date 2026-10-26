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
    recognition::{
        Enrollment, EnrollmentConfig, EnrollmentSession, MatchResult, RecognitionEngine,
        enrollment::EnrollmentRejection,
    },
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
    pending_enrollment: Option<EnrollmentSession>,
    enrollment: Option<Enrollment>,
    sample_available: bool,
}
impl Session {
    pub fn new(config: Config) -> Self {
        Self {
            capture: None,
            config,
            pending_enrollment: None,
            enrollment: None,
            sample_available: false,
        }
    }
    pub fn reset(&mut self) {
        self.capture = None;
        self.pending_enrollment = None;
        self.enrollment = None;
        self.sample_available = false;
    }
    pub fn sample_count(&self) -> usize {
        self.enrollment.as_ref().map_or_else(
            || {
                self.pending_enrollment
                    .as_ref()
                    .map_or(0, |session| session.progress().accepted_samples)
            },
            |enrollment| enrollment.samples.len(),
        )
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
            !self.enrolled(),
            "Enrollment complete. Clear the session to enroll another finger."
        );
        let started = Instant::now();
        let engine = self.config.recognition_engine()?;
        let sample = engine.extract(
            self.capture
                .as_ref()
                .context("Capture a fingerprint first.")?,
        )?;
        let remaining = engine.timeout.saturating_sub(started.elapsed());
        ensure!(!remaining.is_zero(), "Enrollment timed out.");
        let bounded = RecognitionEngine {
            timeout: remaining,
            ..engine
        };
        if self.pending_enrollment.is_none() {
            self.pending_enrollment = Some(EnrollmentSession::new(EnrollmentConfig {
                min_samples: 3,
                max_samples: 3,
                consistency_threshold: self.config.threshold,
            })?);
        }
        let pending = self
            .pending_enrollment
            .as_mut()
            .expect("enrollment initialized");
        let progress = match pending.add(&bounded, &sample) {
            Ok(progress) => progress,
            Err(error) => {
                if let Some(rejection) = error.downcast_ref::<EnrollmentRejection>() {
                    self.sample_available = false;
                    return Ok(SampleAcceptance::Rejected {
                        score: rejection.score,
                        threshold: rejection.threshold,
                    });
                }
                return Err(error);
            }
        };
        if progress.ready {
            self.enrollment = Some(
                self.pending_enrollment
                    .take()
                    .expect("ready enrollment")
                    .finish()?,
            );
        }
        self.sample_available = false;
        Ok(SampleAcceptance::Accepted(progress.accepted_samples))
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
