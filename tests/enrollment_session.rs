// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::OnceLock,
    time::{Duration, Instant},
};
use uareu::{
    metadata::{Region, Resolution},
    recognition::{
        EnrollmentConfig, EnrollmentSession, FingerprintTemplate, RecognitionEngine,
        RecognitionIdentity,
    },
};

// Standalone process-protocol fixture. Marker scores deliberately preserve
// compare orientation; they have no biometric meaning. No shared stub is edited.
const ENROLLMENT_STUB: &str = r#"
use std::{env,fs,io::{Read,Write},time::Duration};
fn marker(input:&str,key:&str)->u8 {
    input.split(&format!("\"{key}\":[")).nth(1).unwrap().split([',',']']).next().unwrap().parse().unwrap()
}
fn main() {
    let root=env::current_exe().unwrap().parent().unwrap().to_owned();
    let mode=fs::read_to_string(root.join("mode.txt")).unwrap();
    let mut launches=fs::OpenOptions::new().append(true).create(true).open(root.join("launches.txt")).unwrap();
    writeln!(launches,"launch").unwrap();
    let mut input=String::new(); std::io::stdin().read_to_string(&mut input).unwrap();
    let probe=marker(&input,"probe"); let candidate=marker(&input,"candidate");
    if mode=="failure" || (mode=="late_failure" && probe==30 && candidate==40) { std::process::exit(7); }
    if mode=="oldslow" { std::thread::sleep(Duration::from_millis(150)); }
    let score=if mode=="late_inconsistent" && probe==30 && candidate==40 {1} else {candidate};
    let build=if mode=="build" {"wrong"} else {"fixture"};
    println!("{{\"schema_version\":1,\"engine\":\"SourceAFIS\",\"version\":\"3.14.0\",\"engine_sha256\":\"{build}\",\"host_sha256\":\"fixture\",\"result\":{{\"score\":{score}}}}}");
}
"#;

struct Harness {
    root: PathBuf,
    engine: RecognitionEngine,
}
impl Harness {
    fn new(mode: &str) -> Self {
        static STUB: OnceLock<PathBuf> = OnceLock::new();
        let stub = STUB.get_or_init(|| {
            let path = std::env::temp_dir().join(format!(
                "uareu-enrollment-stub-{}{}",
                std::process::id(),
                std::env::consts::EXE_SUFFIX
            ));
            let source = path.with_extension("rs");
            fs::write(&source, ENROLLMENT_STUB).unwrap();
            assert!(
                Command::new("rustc")
                    .arg("--edition=2024")
                    .arg(&source)
                    .arg("-o")
                    .arg(&path)
                    .status()
                    .unwrap()
                    .success()
            );
            path
        });
        let mut nonce = [0; 16];
        getrandom::fill(&mut nonce).unwrap();
        let root = std::env::temp_dir().join(format!(
            "uareu-enrollment-test-{:032x}",
            u128::from_le_bytes(nonce)
        ));
        fs::create_dir(&root).unwrap();
        let executable = root.join(format!("helper{}", std::env::consts::EXE_SUFFIX));
        fs::copy(stub, &executable).unwrap();
        fs::write(root.join("mode.txt"), mode).unwrap();
        Self {
            root,
            engine: RecognitionEngine {
                executable,
                timeout: Duration::from_secs(30),
            },
        }
    }
    fn mode(&self, mode: &str) {
        fs::write(self.root.join("mode.txt"), mode).unwrap();
    }
    fn launches(&self) -> usize {
        fs::read_to_string(self.root.join("launches.txt"))
            .unwrap_or_default()
            .lines()
            .count()
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn template(marker: u8) -> FingerprintTemplate {
    FingerprintTemplate {
        schema_version: 1,
        identity: RecognitionIdentity {
            engine: "SourceAFIS".into(),
            version: "3.14.0".into(),
            engine_sha256: "fixture".into(),
            host_sha256: "fixture".into(),
        },
        format: "sourceafis_native_cache".into(),
        data: vec![marker],
        source_resolution: Resolution::declared(500., 500.).unwrap(),
        source_region: Region {
            x: 0,
            y: 0,
            width: 32,
            height: 32,
        },
        source_pixels_sha256: None,
        source_processing: vec![],
    }
}
fn config() -> EnrollmentConfig {
    EnrollmentConfig {
        min_samples: 2,
        max_samples: 3,
        consistency_threshold: 10.,
    }
}

#[test]
fn invalid_configuration_is_rejected() {
    for (min, max) in [(0, 3), (1, 3), (2, 1), (3, 2), (2, 11), (11, 11)] {
        assert!(
            EnrollmentSession::new(EnrollmentConfig {
                min_samples: min,
                max_samples: max,
                ..config()
            })
            .is_err()
        );
    }
    for threshold in [0., -1., f64::NAN, f64::INFINITY] {
        assert!(
            EnrollmentSession::new(EnrollmentConfig {
                consistency_threshold: threshold,
                ..config()
            })
            .is_err()
        );
    }
    assert!(
        EnrollmentSession::new(EnrollmentConfig {
            min_samples: 10,
            max_samples: 10,
            ..config()
        })
        .is_ok()
    );
}
#[test]
fn add_progress_ready_finish_and_full_session() {
    let h = Harness::new("new");
    assert!(EnrollmentSession::new(config()).unwrap().finish().is_err());
    let mut session = EnrollmentSession::new(config()).unwrap();
    let p = session.add(&h.engine, &template(20)).unwrap();
    assert_eq!(p.accepted_samples, 1);
    assert!(!p.ready);
    assert!(!p.at_capacity);
    assert_eq!(p.minimum_pair_score, None);
    let p = session.add(&h.engine, &template(30)).unwrap();
    assert_eq!(p.accepted_samples, 2);
    assert!(p.ready);
    assert!(!p.at_capacity);
    assert_eq!(p.minimum_pair_score, Some(30.));
    let p = session.add(&h.engine, &template(40)).unwrap();
    assert_eq!(p.accepted_samples, 3);
    assert!(p.ready);
    assert!(p.at_capacity);
    let launches = h.launches();
    assert!(session.add(&h.engine, &template(50)).is_err());
    assert_eq!(h.launches(), launches);
    let enrolled = session.finish().unwrap();
    assert_eq!(enrolled.samples.len(), 3);
    assert_eq!(enrolled.minimum_pair_score, 30.);
    assert_eq!(
        enrolled.policy,
        "all_pairs_consistent_multi_template_not_vendor_fusion"
    );
    assert_eq!(enrolled.samples[0].data, vec![20]);
}
#[test]
fn inconsistent_or_failed_add_is_transactional_and_reset_clears_progress() {
    let h = Harness::new("new");
    let mut session = EnrollmentSession::new(config()).unwrap();
    session.add(&h.engine, &template(20)).unwrap();
    let before = session.progress();
    assert!(session.add(&h.engine, &template(1)).is_err());
    assert_eq!(session.progress(), before);
    h.mode("failure");
    assert!(session.add(&h.engine, &template(30)).is_err());
    assert_eq!(session.progress(), before);
    h.mode("new");
    session.add(&h.engine, &template(30)).unwrap();
    session.reset();
    assert_eq!(session.progress().accepted_samples, 0);
    assert_eq!(session.progress().minimum_pair_score, None);
    assert!(!session.progress().ready);
    assert!(session.finish().is_err());
}
#[test]
fn build_mismatch_and_invalid_template_do_not_change_state() {
    let h = Harness::new("new");
    let mut session = EnrollmentSession::new(config()).unwrap();
    let mut invalid = template(20);
    invalid.data.clear();
    assert!(session.add(&h.engine, &invalid).is_err());
    assert_eq!(session.progress().accepted_samples, 0);
    session.add(&h.engine, &template(20)).unwrap();
    let before = session.progress();
    let mut other = template(30);
    other.identity.engine_sha256 = "other".into();
    assert!(session.add(&h.engine, &other).is_err());
    assert_eq!(h.launches(), 0);
    assert_eq!(session.progress(), before);
    h.mode("build");
    assert!(session.add(&h.engine, &template(30)).is_err());
    assert_eq!(session.progress(), before);
}
#[test]
fn incremental_result_matches_batch_policy_scores_and_sample_order() {
    let h = Harness::new("new");
    let samples = [template(20), template(40), template(30)];
    let mut session = EnrollmentSession::new(config()).unwrap();
    for sample in &samples {
        session.add(&h.engine, sample).unwrap();
    }
    let incremental = session.finish().unwrap();
    let batch = h.engine.enroll(&samples, 10.).unwrap();
    assert_eq!(
        serde_json::to_value(incremental).unwrap(),
        serde_json::to_value(batch).unwrap()
    );
}
#[test]
fn add_uses_one_deadline_for_all_pair_checks() {
    let mut h = Harness::new("new");
    let mut session = EnrollmentSession::new(config()).unwrap();
    session.add(&h.engine, &template(20)).unwrap();
    session.add(&h.engine, &template(30)).unwrap();
    let before = session.progress();
    let launches = h.launches();
    h.mode("oldslow");
    h.engine.timeout = Duration::from_millis(250);
    let start = Instant::now();
    assert!(session.add(&h.engine, &template(40)).is_err());
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(h.launches() - launches <= 2);
    assert_eq!(session.progress(), before);
    h.engine.timeout = Duration::ZERO;
    assert!(session.add(&h.engine, &template(40)).is_err());
    assert_eq!(session.progress(), before);
}

#[test]
fn a_late_pair_rejection_never_commits_an_earlier_success() {
    let h = Harness::new("new");
    let mut session = EnrollmentSession::new(config()).unwrap();
    session.add(&h.engine, &template(20)).unwrap();
    session.add(&h.engine, &template(30)).unwrap();
    let before = session.progress();
    for mode in ["late_failure", "late_inconsistent"] {
        h.mode(mode);
        let launches = h.launches();
        assert!(session.add(&h.engine, &template(40)).is_err());
        assert_eq!(h.launches() - launches, 2);
        assert_eq!(session.progress(), before);
    }
    h.mode("new");
    session.add(&h.engine, &template(40)).unwrap();
    assert_eq!(session.finish().unwrap().minimum_pair_score, 30.);
}
