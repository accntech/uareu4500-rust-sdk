// SPDX-License-Identifier: GPL-3.0-or-later
use std::{fs, path::PathBuf, process::Command, sync::OnceLock, time::Duration};
use uareu::{
    metadata::{Region, Resolution},
    recognition::{Enrollment, FingerprintTemplate, RecognitionEngine, RecognitionIdentity},
};

struct Harness {
    root: PathBuf,
    engine: RecognitionEngine,
}
impl Harness {
    fn new(mode: &str) -> Self {
        static STUB: OnceLock<PathBuf> = OnceLock::new();
        let stub = STUB.get_or_init(|| {
            let path = std::env::temp_dir().join(format!(
                "uareu-recognition-stub-{}{}",
                std::process::id(),
                std::env::consts::EXE_SUFFIX
            ));
            let status = Command::new("rustc")
                .args(["--edition=2024", "tests/support/recognition_stub.rs", "-o"])
                .arg(&path)
                .status()
                .unwrap();
            assert!(status.success());
            path
        });
        let mut nonce = [0; 16];
        getrandom::fill(&mut nonce).unwrap();
        let root = std::env::temp_dir().join(format!(
            "uareu-batch-test-{:032x}",
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
#[test]
fn fitting_gallery_is_one_launch_with_existing_ranking_threshold_and_tie_break() {
    let h = Harness::new("new");
    let r = h
        .engine
        .identify(
            &template(0),
            &[template(20), template(50), template(50), template(10)],
            20.,
            2,
        )
        .unwrap();
    assert_eq!(
        r.iter().map(|c| (c.index, c.score)).collect::<Vec<_>>(),
        vec![(1, 50.), (2, 50.)]
    );
    assert_eq!(h.launches(), 1);
}
#[test]
fn old_helper_keeps_first_result_without_repeating_comparison() {
    let h = Harness::new("old");
    let r = h
        .engine
        .identify(
            &template(0),
            &[template(20), template(50), template(30)],
            20.,
            10,
        )
        .unwrap();
    assert_eq!(r.iter().map(|c| c.index).collect::<Vec<_>>(), vec![1, 2, 0]);
    assert_eq!(h.launches(), 3);
}
#[test]
fn empty_gallery_does_not_spawn_and_invalid_candidate_is_checked_before_spawn() {
    let h = Harness::new("new");
    assert!(
        h.engine
            .identify(&template(0), &[], 1., 1)
            .unwrap()
            .is_empty()
    );
    let mut bad = template(2);
    bad.identity.engine_sha256 = "other".into();
    assert!(
        h.engine
            .identify(&template(0), &[template(1), bad], 1., 2)
            .is_err()
    );
    assert_eq!(h.launches(), 0);
}
#[test]
fn malformed_batch_and_failed_helpers_never_fallback() {
    for mode in ["short", "null", "negative", "failure", "build", "flood"] {
        let h = Harness::new(mode);
        assert!(
            h.engine
                .identify(&template(0), &[template(20), template(50)], 1., 2)
                .is_err(),
            "mode {mode}"
        );
        assert_eq!(h.launches(), 1, "mode {mode}");
    }
}
#[test]
fn old_helper_fallback_keeps_the_overall_deadline() {
    let mut h = Harness::new("oldslow");
    h.engine.timeout = Duration::from_millis(250);
    assert!(
        h.engine
            .identify(&template(0), &vec![template(20); 4], 1., 4)
            .is_err()
    );
    assert!(h.launches() <= 2);
}
#[test]
fn aggregate_overflow_preserves_legacy_gallery_path() {
    let h = Harness::new("new");
    let mut big = template(255);
    big.data = vec![255; 1024 * 1024];
    let candidates = vec![big; 5];
    let r = h.engine.identify(&template(0), &candidates, 1., 5).unwrap();
    assert_eq!(r.len(), 5);
    assert_eq!(h.launches(), 5);
}
#[test]
fn enrollment_verification_uses_one_batch_and_retains_maximum_score() {
    let h = Harness::new("new");
    let enrollment = Enrollment {
        schema_version: 1,
        policy: "all_pairs_consistent_multi_template_not_vendor_fusion".into(),
        samples: vec![template(20), template(50)],
        consistency_threshold: 1.,
        minimum_pair_score: 1.,
    };
    let r = h
        .engine
        .verify_enrollment(&template(0), &enrollment, 40.)
        .unwrap();
    assert_eq!(r.score, 50.);
    assert!(r.matched);
    assert_eq!(h.launches(), 1);
}

#[test]
#[ignore = "requires UAREU_TEST_RECOGNITION_HELPER pointing to the built real helper"]
fn real_helper_batch_scores_equal_pairwise_for_synthetic_minutiae() {
    let engine = RecognitionEngine {
        executable: PathBuf::from(
            std::env::var_os("UAREU_TEST_RECOGNITION_HELPER").expect("set real helper path"),
        ),
        timeout: Duration::from_secs(300),
    };
    let templates = synthetic_templates(&engine);
    let reference: Vec<f64> = templates
        .iter()
        .map(|t| engine.compare(&templates[0], t, 1.).unwrap().score)
        .collect();
    for size in [1, 10, 100, 1000] {
        let candidates: Vec<_> = (0..size)
            .map(|i| templates[i % templates.len()].clone())
            .collect();
        let result = engine
            .identify(&templates[0], &candidates, f64::MIN_POSITIVE, size)
            .unwrap();
        let mut expected: Vec<_> = (0..size)
            .filter(|i| reference[i % reference.len()] > 0.)
            .map(|i| (i, reference[i % reference.len()]))
            .collect();
        expected.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        assert_eq!(
            result
                .iter()
                .map(|c| (c.index, c.score))
                .collect::<Vec<_>>(),
            expected
        );
    }
}

fn synthetic_templates(engine: &RecognitionEngine) -> Vec<FingerprintTemplate> {
    use uareu::template_io::{Minutia, MinutiaeRecord, TemplateFormat, encode};
    let mut templates = Vec::new();
    for variant in 0..4 {
        let record = MinutiaeRecord {
            width: 384,
            height: 290,
            pixels_per_cm_x: 197,
            pixels_per_cm_y: 197,
            finger_position: 0,
            impression: 0,
            quality: 0,
            vendor_id: 0x0103,
            subformat: 0,
            device_info: 0,
            minutiae: (0..40)
                .map(|i| Minutia {
                    x: 20 + (i % 8) * 40 + variant * (i % 3),
                    y: 20 + (i / 8) * 40 + variant * (i % 5),
                    kind: 1 + (i % 2) as u8,
                    direction_clockwise: ((i % 6) as f64 * 0.45 + variant as f64 * 0.1)
                        % std::f64::consts::TAU,
                    quality: 0,
                })
                .collect(),
        };
        let bytes = encode(&record, TemplateFormat::Iso19794_2_2005).unwrap();
        templates.push(
            engine
                .import_template(&bytes, TemplateFormat::Iso19794_2_2005)
                .unwrap(),
        );
    }
    templates
}

#[test]
#[ignore = "requires real helper paths and UAREU_RECOGNITION_BENCHMARK_OUTPUT"]
fn real_helper_benchmark_against_pairwise_baseline() {
    use sha2::{Digest, Sha256};
    use std::time::Instant;
    let engine = RecognitionEngine {
        executable: PathBuf::from(
            std::env::var_os("UAREU_TEST_RECOGNITION_HELPER").expect("set new helper"),
        ),
        timeout: Duration::from_secs(300),
    };
    let old = RecognitionEngine {
        executable: PathBuf::from(
            std::env::var_os("UAREU_TEST_LEGACY_HELPER").expect("set preserved old helper"),
        ),
        timeout: Duration::from_secs(300),
    };
    let templates = synthetic_templates(&engine);
    let mut rows = Vec::new();
    for size in [1, 10, 100] {
        let candidates: Vec<_> = (0..size)
            .map(|i| templates[i % templates.len()].clone())
            .collect();
        let pairwise = || {
            let started = Instant::now();
            let mut scores = Vec::new();
            for (index, candidate) in candidates.iter().enumerate() {
                let remaining = old.timeout.saturating_sub(started.elapsed());
                assert!(!remaining.is_zero(), "baseline operation timed out");
                let score = RecognitionEngine {
                    executable: old.executable.clone(),
                    timeout: remaining,
                }
                .compare(&templates[0], candidate, f64::MIN_POSITIVE)
                .unwrap()
                .score;
                if score > 0.0 {
                    scores.push((index, score));
                }
            }
            scores.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            scores
        };
        let expected = pairwise();
        let _ = engine
            .identify(&templates[0], &candidates, f64::MIN_POSITIVE, size)
            .unwrap();
        let mut baseline_ms = Vec::new();
        let mut batched_ms = Vec::new();
        for run in 0..10 {
            let started = Instant::now();
            assert_eq!(pairwise(), expected);
            baseline_ms.push(started.elapsed().as_secs_f64() * 1000.0);
            let started = Instant::now();
            let result = engine
                .identify(&templates[0], &candidates, f64::MIN_POSITIVE, size)
                .unwrap();
            batched_ms.push(started.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(
                result
                    .iter()
                    .map(|c| (c.index, c.score))
                    .collect::<Vec<_>>(),
                expected
            );
            eprintln!(
                "gallery={size} run={} legacy_ms={:.2} batch_ms={:.2}",
                run + 1,
                baseline_ms[run],
                batched_ms[run]
            );
        }
        fn median(values: &[f64]) -> f64 {
            let mut v = values.to_vec();
            v.sort_by(f64::total_cmp);
            (v[4] + v[5]) / 2.0
        }
        fn p95(values: &[f64]) -> f64 {
            values.iter().copied().max_by(f64::total_cmp).unwrap()
        }
        rows.push(serde_json::json!({"gallery_size":size,"repetitions":10,"baseline_median_ms":median(&baseline_ms),"batch_median_ms":median(&batched_ms),"baseline_p95_nearest_rank_ms":p95(&baseline_ms),"batch_p95_nearest_rank_ms":p95(&batched_ms),"baseline_samples_ms":baseline_ms,"batch_samples_ms":batched_ms,"baseline_expected_launches":size,"batch_expected_launches":1,"equal_scores_and_ranking":true}));
    }
    let output = PathBuf::from(
        std::env::var_os("UAREU_RECOGNITION_BENCHMARK_OUTPUT")
            .expect("set new benchmark output file"),
    );
    let record = serde_json::json!({"schema_version":1,"platform":std::env::consts::OS,"architecture":std::env::consts::ARCH,"engine":"SourceAFIS","version":"3.14.0","new_helper_sha256":format!("{:x}",Sha256::digest(fs::read(&engine.executable).unwrap())),"legacy_helper_sha256":format!("{:x}",Sha256::digest(fs::read(&old.executable).unwrap())),"new_helper_assembly_sha256":format!("{:x}",Sha256::digest(fs::read(engine.executable.parent().unwrap().join("UareuEngine.dll")).unwrap())),"legacy_helper_assembly_sha256":format!("{:x}",Sha256::digest(fs::read(old.executable.parent().unwrap().join("UareuEngine.dll")).unwrap())),"sourceafis_assembly_sha256":templates[0].identity.engine_sha256,"synthetic_cache_sha256":templates.iter().map(|t|format!("{:x}",Sha256::digest(&t.data))).collect::<Vec<_>>(),"source":"four artificial 40-minutia patterns; no physical fingerprints","filesystem_cache":"warmed; each call starts a new helper process","gallery_1000_latency":"not_run; score/ranking equivalence is covered separately by cached pairwise reference","rows":rows});
    use std::io::Write;
    fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(output)
        .unwrap()
        .write_all(serde_json::to_string_pretty(&record).unwrap().as_bytes())
        .unwrap();
    for row in rows
        .iter()
        .filter(|r| r["gallery_size"].as_u64().unwrap() >= 10)
    {
        assert!(
            row["batch_median_ms"].as_f64().unwrap() <= row["baseline_median_ms"].as_f64().unwrap(),
            "performance gate regressed; repeat once per plan before claiming acceptance"
        );
    }
}

#[test]
fn batch_hang_and_invalid_deadline_fail_without_fallback() {
    let mut h = Harness::new("hang");
    h.engine.timeout = Duration::from_millis(500);
    assert!(
        h.engine
            .identify(&template(0), &[template(1), template(2)], 1., 2)
            .is_err()
    );
    assert_eq!(h.launches(), 1);
    let mut invalid = Harness::new("new");
    invalid.engine.timeout = Duration::from_secs(301);
    assert!(
        invalid
            .engine
            .identify(&template(0), &[template(1)], 1., 1)
            .is_err()
    );
    assert_eq!(invalid.launches(), 0);
}

#[test]
#[ignore = "requires real helper and a new UAREU_RECOGNITION_1000_OUTPUT path"]
fn real_helper_1000_gallery_latency_and_score_equivalence() {
    use sha2::{Digest, Sha256};
    use std::{io::Write, time::Instant};
    let output = PathBuf::from(
        std::env::var_os("UAREU_RECOGNITION_1000_OUTPUT").expect("set new output path"),
    );
    // Reserve the output before executing a workload; never overwrite past results.
    let mut receipt = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(output)
        .unwrap();
    let engine = RecognitionEngine {
        executable: PathBuf::from(
            std::env::var_os("UAREU_TEST_RECOGNITION_HELPER").expect("set real helper"),
        ),
        timeout: Duration::from_secs(300),
    };
    let templates = synthetic_templates(&engine);
    let reference: Vec<_> = templates
        .iter()
        .map(|t| {
            engine
                .compare(&templates[0], t, f64::MIN_POSITIVE)
                .unwrap()
                .score
        })
        .collect();
    let candidates: Vec<_> = (0..1000).map(|i| templates[i % 4].clone()).collect();
    let mut expected: Vec<_> = (0..1000)
        .filter(|i| reference[i % 4] > 0.0)
        .map(|i| (i, reference[i % 4]))
        .collect();
    expected.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut elapsed_ms = Vec::new();
    for run in 0..11 {
        let started = Instant::now();
        let actual = engine
            .identify(&templates[0], &candidates, f64::MIN_POSITIVE, 1000)
            .unwrap();
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(
            actual
                .iter()
                .map(|v| (v.index, v.score))
                .collect::<Vec<_>>(),
            expected
        );
        elapsed_ms.push(elapsed);
        eprintln!("gallery=1000 run={} elapsed_ms={elapsed:.2}", run + 1);
    }
    let mut sorted = elapsed_ms[1..].to_vec();
    sorted.sort_by(f64::total_cmp);
    let assembly = engine.executable.parent().unwrap().join("UareuEngine.dll");
    let result = serde_json::json!({
        "schema_version": 1,
        "platform": std::env::consts::OS,
        "architecture": std::env::consts::ARCH,
        "gallery_size": 1000,
        "source": "four repeated artificial 40-minutia patterns; no physical fingerprints",
        "helper_assembly_sha256": format!("{:x}", Sha256::digest(fs::read(assembly).unwrap())),
        "sourceafis_assembly_sha256": templates[0].identity.engine_sha256,
        "synthetic_cache_sha256": templates.iter().map(|t| format!("{:x}", Sha256::digest(&t.data))).collect::<Vec<_>>(),
        "first_gallery_run_ms": elapsed_ms[0],
        "subsequent_samples_ms": &elapsed_ms[1..],
        "subsequent_median_ms": (sorted[4] + sorted[5]) / 2.0,
        "subsequent_p95_nearest_rank_ms": sorted[9],
        "equal_scores_and_ranking": true,
        "cache_conditions": "fresh helper process each call; filesystem cache uncontrolled and likely warm after setup; not cold-cache evidence",
        "limitations": "single host synthetic latency; no pairwise speedup, biometric accuracy, production gallery or cold-cache claim"
    });
    receipt
        .write_all(serde_json::to_string_pretty(&result).unwrap().as_bytes())
        .unwrap();
}
