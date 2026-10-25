// SPDX-License-Identifier: GPL-3.0-or-later
use evaluator::{dataset, metrics};
use metrics::{Comparison, bootstrap, rates, threshold_grid};

#[test]
fn hand_computed_rates_keep_failure_denominators_separate() {
    let scores = vec![
        Comparison {
            subject_a: 0,
            subject_b: 0,
            genuine: true,
            score: 20.0,
        },
        Comparison {
            subject_a: 1,
            subject_b: 1,
            genuine: true,
            score: 5.0,
        },
        Comparison {
            subject_a: 0,
            subject_b: 1,
            genuine: false,
            score: 10.0,
        },
        Comparison {
            subject_a: 1,
            subject_b: 0,
            genuine: false,
            score: 2.0,
        },
    ];
    let result = rates(&scores, 10.0, &[1, 1]).unwrap();
    assert_eq!(result.fmr, Some(0.5));
    assert_eq!(result.fnmr, Some(0.5));
    assert_eq!((result.genuine_weight, result.impostor_weight), (2.0, 2.0));
    let weighted = rates(&scores, 10.0, &[2, 1]).unwrap();
    assert_eq!(weighted.fnmr, Some(1.0 / 3.0));
    assert_eq!(
        (weighted.genuine_weight, weighted.impostor_weight),
        (3.0, 4.0)
    );
    assert_eq!(rates(&[], 10.0, &[]).unwrap().fmr, None);
    assert!(rates(&scores, f64::NAN, &[1, 1]).is_err());
}

#[test]
fn development_grid_is_positive_sorted_and_has_an_above_max_endpoint() {
    let grid = threshold_grid(&[0.0, 10.0, 10.0, 20.0]).unwrap();
    assert_eq!(&grid[..2], &[10.0, 20.0]);
    assert!(grid[2] > 20.0 && grid[2].is_finite());
    assert!(threshold_grid(&[0.0]).is_err());
    assert!(threshold_grid(&[f64::NAN]).is_err());
}

#[test]
fn bootstrap_is_seeded_and_small_corpora_do_not_get_intervals() {
    let scores: Vec<_> = (0..10)
        .flat_map(|a| {
            (0..10).map(move |b| Comparison {
                subject_a: a,
                subject_b: b,
                genuine: a == b,
                score: if a == b { 20.0 } else { (a + b) as f64 },
            })
        })
        .collect();
    let first = bootstrap(&scores, 10.0, 10, 42).unwrap();
    let again = bootstrap(&scores, 10.0, 10, 42).unwrap();
    assert_eq!(first, again);
    assert!(first.fmr_interval.is_some());
    assert_eq!(first.replicates, 2000);
    let small = bootstrap(&[], 10.0, 2, 42).unwrap();
    assert_eq!(small.fmr_interval, None);
    assert!(small.reason.contains("10"));
}

#[test]
fn manifest_rejects_leakage_and_missing_authority_before_any_engine() {
    let source = serde_json::json!({"schema_version":1,"engine_path":"absent-helper", "timeout_ms":1000,"seed":42,
        "data_use_authority":"missing-authority", "samples":[
        {"subject_id":"subject1","finger_id":"finger1","session_id":"s1","split":"development","role":"enrollment","path":"a.png","sha256":"00".repeat(32),"scale_evidence":"evidence.json"},
        {"subject_id":"subject1","finger_id":"finger1","session_id":"s2","split":"evaluation","role":"probe","path":"b.png","sha256":"11".repeat(32),"scale_evidence":"evidence.json"}
    ]});
    let manifest: dataset::Manifest = serde_json::from_value(source.clone()).unwrap();
    assert!(
        dataset::validate_structure(&manifest)
            .unwrap_err()
            .to_string()
            .contains("subject")
    );
    let mut blank_authority = source.clone();
    blank_authority["data_use_authority"] = "".into();
    let manifest: dataset::Manifest = serde_json::from_value(blank_authority).unwrap();
    assert!(
        dataset::validate_structure(&manifest)
            .unwrap_err()
            .to_string()
            .contains("authority")
    );
    let mut same_role = source;
    same_role["samples"][1]["split"] = "development".into();
    same_role["samples"][1]["sha256"] = same_role["samples"][0]["sha256"].clone();
    let manifest: dataset::Manifest = serde_json::from_value(same_role).unwrap();
    assert!(
        dataset::validate_structure(&manifest)
            .unwrap_err()
            .to_string()
            .contains("hash")
    );
}

#[test]
fn image_preflight_checks_hash_and_scale_before_extraction() {
    use sha2::{Digest, Sha256};
    use uareu::{
        frame::Image,
        metadata::{CapturedImage, Resolution},
    };
    let directory = std::env::temp_dir().join(format!(
        "uareu-evaluation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let mut captured = CapturedImage::new(Image {
        width: 16,
        height: 16,
        pixels: vec![128; 256],
    });
    captured.metadata.resolution = Resolution::declared(500.0, 500.0).unwrap();
    captured.save_png(directory.join("image.png")).unwrap();
    let hash = format!(
        "{:x}",
        Sha256::digest(std::fs::read(directory.join("image.png")).unwrap())
    );
    let mut sample = dataset::Sample {
        subject_id: "s1".into(),
        finger_id: "f1".into(),
        session_id: "a".into(),
        split: "development".into(),
        role: "enrollment".into(),
        path: "image.png".into(),
        sha256: hash.clone(),
        scale_evidence: "scale.json".into(),
    };
    let evidence = |ppi| {
        serde_json::to_vec(&serde_json::json!({"schema_version":1,"image_sha256":hash,"x_ppi":ppi,"y_ppi":ppi,"justification":"synthetic fixture scale property; not reader calibration"})).unwrap()
    };
    std::fs::write(directory.join("scale.json"), evidence(500.0)).unwrap();
    assert!(dataset::image(&directory, &sample).is_ok());
    std::fs::write(directory.join("scale.json"), evidence(450.0)).unwrap();
    assert!(
        dataset::image(&directory, &sample)
            .unwrap_err()
            .to_string()
            .contains("conflicts")
    );
    sample.sha256 = "00".repeat(32);
    assert!(
        dataset::image(&directory, &sample)
            .unwrap_err()
            .to_string()
            .contains("hash")
    );
    captured.metadata.resolution = Resolution::Unknown;
    captured.save_png(directory.join("unknown.png")).unwrap();
    sample.path = "unknown.png".into();
    sample.sha256 = format!(
        "{:x}",
        Sha256::digest(std::fs::read(directory.join("unknown.png")).unwrap())
    );
    std::fs::write(directory.join("scale.json"),serde_json::to_vec(&serde_json::json!({"schema_version":1,"image_sha256":sample.sha256,"x_ppi":500,"y_ppi":500,"justification":"cannot override unknown PNG scale"})).unwrap()).unwrap();
    assert!(
        dataset::image(&directory, &sample)
            .unwrap_err()
            .to_string()
            .contains("unknown")
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn absent_splits_cannot_produce_a_completed_evaluation() {
    let mut samples = Vec::new();
    for subject in ["a", "b"] {
        for (session, role, hash) in [("s1", "enrollment", "01"), ("s2", "probe", "02")] {
            let digest = if subject == "a" {
                hash.repeat(32)
            } else {
                format!("{}{}", hash, "03".repeat(31))
            };
            samples.push(serde_json::json!({"subject_id":subject,"finger_id":"f1","session_id":session,"split":"development","role":role,"path":"image.png","sha256":digest,"scale_evidence":"scale.json"}));
        }
    }
    let manifest:dataset::Manifest=serde_json::from_value(serde_json::json!({"schema_version":1,"engine_path":"helper","timeout_ms":1000,"seed":1,"data_use_authority":"authority.txt","samples":samples})).unwrap();
    assert!(
        dataset::validate_structure(&manifest)
            .unwrap_err()
            .to_string()
            .contains("split")
    );
}

#[allow(dead_code)]
#[path = "../examples/evaluate_recognition.rs"]
mod evaluator;
#[test]
fn full_pipeline_freezes_grid_before_held_out_and_reports_only_aggregates() {
    use sha2::{Digest, Sha256};
    use uareu::{
        frame::Image,
        metadata::{CapturedImage, Resolution},
    };
    let root = std::env::temp_dir().join(format!(
        "uareu-evaluation-pipeline-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let stub = root.join("helper.rs");
    std::fs::write(&stub,r#"
use std::{fs,io::{self,Read}};
fn main(){
 let root=std::env::current_exe().unwrap().parent().unwrap().to_owned();
 let mut input=String::new();io::stdin().read_to_string(&mut input).unwrap();
 let extract=input.contains("\"operation\":\"extract\"");
 let result=if extract {
  let n=fs::read_to_string(root.join("extract-count")).unwrap_or_default().parse::<usize>().unwrap_or(0);
  if n>=6 {assert!(root.join("output/frozen-configuration.json").is_file(),"held-out before frozen grid");}
  fs::write(root.join("extract-count"),(n+1).to_string()).unwrap();
  "{\"data\":[1,2,3]}"
 }else {"{\"score\":20.0}"};
 println!("{{\"schema_version\":1,\"engine\":\"SourceAFIS\",\"version\":\"3.14.0\",\"engine_sha256\":\"synthetic-process-fixture\",\"host_sha256\":\"synthetic-process-fixture\",\"result\":{result}}}");
}
"#).unwrap();
    let helper = root.join(if cfg!(windows) {
        "helper.exe"
    } else {
        "helper"
    });
    assert!(
        std::process::Command::new("rustc")
            .arg(&stub)
            .arg("-o")
            .arg(&helper)
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(
        root.join("authority.txt"),
        "Synthetic process-contract fixtures only; no biometric subjects or accuracy evidence.",
    )
    .unwrap();
    let mut samples = Vec::new();
    let mut pixels = 0u8;
    for (split, subjects) in [("development", ["d1", "d2"]), ("evaluation", ["e1", "e2"])] {
        for subject in subjects {
            for (image, session, role) in [
                ("a", "s1", "enrollment"),
                ("b", "s1", "enrollment"),
                ("c", "s2", "probe"),
            ] {
                pixels += 1;
                let filename = format!("{subject}-{image}.png");
                let scale = format!("{subject}-{image}-scale.json");
                let mut captured = CapturedImage::new(Image {
                    width: 16,
                    height: 16,
                    pixels: vec![pixels; 256],
                });
                captured.metadata.resolution = Resolution::declared(500.0, 500.0).unwrap();
                captured.save_png(root.join(&filename)).unwrap();
                let hash = format!(
                    "{:x}",
                    Sha256::digest(std::fs::read(root.join(&filename)).unwrap())
                );
                std::fs::write(root.join(&scale),serde_json::to_vec(&serde_json::json!({"schema_version":1,"image_sha256":hash,"x_ppi":500,"y_ppi":500,"justification":"synthetic fixture property only"})).unwrap()).unwrap();
                samples.push(serde_json::json!({"subject_id":subject,"finger_id":"f1","session_id":session,"split":split,"role":role,"path":filename,"sha256":hash,"scale_evidence":scale}));
            }
        }
    }
    let manifest = root.join("manifest.json");
    let output = root.join("output");
    std::fs::write(&manifest,serde_json::to_vec(&serde_json::json!({"schema_version":1,"engine_path":helper,"timeout_ms":5000,"seed":42,"data_use_authority":"authority.txt","samples":samples})).unwrap()).unwrap();
    evaluator::run(&manifest, &output).unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("extract-count")).unwrap(),
        "12"
    );
    let report_bytes = std::fs::read(output.join("report.json")).unwrap();
    let report: serde_json::Value = serde_json::from_slice(&report_bytes).unwrap();
    assert_eq!(report["evaluation"]["counts"]["extraction_failures"], 0);
    assert_eq!(
        report["evaluation"]["threshold_results"][0]["rates"]["fmr"],
        1.0
    );
    assert_eq!(
        report["evaluation"]["decisions"]["enrollment"][0]["cross_subject_fmr"],
        1.0
    );
    assert_eq!(
        report["evaluation"]["decisions"]["enrollment"][0]["fnmr"],
        0.0
    );
    let text = String::from_utf8(report_bytes).unwrap();
    assert!(!text.contains("pixels") && !text.contains("template") && !text.contains("e1-a.png"));
    assert!(
        evaluator::run(&manifest, &output)
            .unwrap_err()
            .to_string()
            .contains("already exists")
    );
    std::fs::remove_dir_all(root).unwrap();
}
