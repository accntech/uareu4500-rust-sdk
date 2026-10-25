// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline exploratory evaluation using the product's actual recognition APIs.
#[path = "evaluation/dataset.rs"]
pub(crate) mod dataset;
#[path = "evaluation/metrics.rs"]
pub(crate) mod metrics;
use anyhow::{Context, Result, ensure};
use dataset::{Manifest, Sample};
use metrics::Comparison;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::Path,
    time::Duration,
};
use uareu::recognition::{FingerprintTemplate, RecognitionEngine};

fn write_new(root: &Path, name: &str, value: &Value) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(root.join(name))?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    Ok(())
}
struct Prepared<'a> {
    sample: &'a Sample,
    template: Option<FingerprintTemplate>,
}
fn prepare<'a>(
    manifest: &'a Manifest,
    root: &Path,
    split: &str,
    engine: &RecognitionEngine,
) -> Result<Vec<Prepared<'a>>> {
    manifest
        .samples
        .iter()
        .filter(|s| s.split == split)
        .map(|sample| {
            let image = dataset::image(root, sample)?;
            Ok(Prepared {
                sample,
                template: engine.extract(&image).ok(),
            })
        })
        .collect()
}
fn subjects(samples: &[Prepared<'_>]) -> BTreeMap<String, usize> {
    samples
        .iter()
        .map(|s| s.sample.subject_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .enumerate()
        .map(|(index, name)| (name, index))
        .collect()
}
fn comparisons(
    samples: &[Prepared<'_>],
    engine: &RecognitionEngine,
) -> Result<(Vec<Comparison>, Value)> {
    let subject = subjects(samples);
    let mut completed = Vec::new();
    let (mut attempted, mut failed) = (0usize, 0usize);
    for probe in samples.iter().filter(|s| s.sample.role == "probe") {
        for candidate in samples.iter().filter(|s| s.sample.role == "enrollment") {
            let genuine = probe.sample.subject_id == candidate.sample.subject_id
                && probe.sample.finger_id == candidate.sample.finger_id;
            if !genuine && probe.sample.subject_id == candidate.sample.subject_id {
                continue;
            }
            attempted += 1;
            match (&probe.template, &candidate.template) {
                (Some(a), Some(b)) => match engine.compare(a, b, 1.0) {
                    Ok(result) => completed.push(Comparison {
                        subject_a: subject[&probe.sample.subject_id],
                        subject_b: subject[&candidate.sample.subject_id],
                        genuine,
                        score: result.score,
                    }),
                    Err(_) => failed += 1,
                },
                _ => failed += 1,
            }
        }
    }
    Ok((
        completed,
        json!({"samples_attempted":samples.len(),"subjects":subject.len(),"extraction_failures":samples.iter().filter(|s|s.template.is_none()).count(),"eligible_comparisons_attempted":attempted,"comparison_failures_including_extraction_failures":failed}),
    ))
}
fn summarize(
    scores: &[Comparison],
    grid: &[f64],
    count: usize,
    seed: u64,
    held_out: bool,
) -> Result<Vec<Value>> {
    grid.iter().map(|&threshold| {
        Ok(json!({"threshold":threshold,"rates":metrics::rates(scores,threshold,&vec![1;count])?,
            "uncertainty":if held_out { Some(metrics::bootstrap(scores,threshold,count,seed)?) } else { None }}))
    }).collect()
}
fn decisions(samples: &[Prepared<'_>], grid: &[f64], engine: &RecognitionEngine) -> Result<Value> {
    let mut result = Vec::new();
    let mut groups: BTreeMap<(&str, &str), Vec<&Prepared<'_>>> = BTreeMap::new();
    for sample in samples.iter().filter(|s| s.sample.role == "enrollment") {
        groups
            .entry((&sample.sample.subject_id, &sample.sample.finger_id))
            .or_default()
            .push(sample);
    }
    for &threshold in grid {
        for size in [2usize, 3, 5, 10] {
            let (
                mut attempted,
                mut unavailable,
                mut failed,
                mut verified,
                mut genuine_attempted,
                mut impostor_attempted,
                mut false_nonmatch,
                mut false_match,
            ) = (
                0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize,
            );
            let (mut genuine_completed, mut impostor_completed) = (0usize, 0usize);
            for ((subject, finger), group) in &groups {
                attempted += 1;
                if group.len() < size {
                    unavailable += 1;
                    continue;
                }
                let chosen: Option<Vec<_>> =
                    group[..size].iter().map(|s| s.template.clone()).collect();
                let Some(chosen) = chosen else {
                    failed += 1;
                    continue;
                };
                let Ok(enrollment) = engine.enroll(&chosen, threshold) else {
                    failed += 1;
                    continue;
                };
                for probe in samples.iter().filter(|s| s.sample.role == "probe") {
                    let genuine =
                        probe.sample.subject_id == *subject && probe.sample.finger_id == *finger;
                    if !genuine && probe.sample.subject_id == *subject {
                        continue;
                    }
                    if genuine {
                        genuine_attempted += 1;
                    } else {
                        impostor_attempted += 1;
                    }
                    if let Some(template) = &probe.template {
                        if let Ok(value) =
                            engine.verify_enrollment(template, &enrollment, threshold)
                        {
                            verified += 1;
                            if genuine {
                                genuine_completed += 1;
                            } else {
                                impostor_completed += 1;
                            }
                            if genuine && !value.matched {
                                false_nonmatch += 1;
                            }
                            if !genuine && value.matched {
                                false_match += 1;
                            }
                        }
                    }
                }
            }
            result.push(json!({"threshold":threshold,"enrollment_size":size,"fingers_attempted":attempted,"insufficient_samples":unavailable,"enrollment_failures_including_extraction_failures":failed,"genuine_verifications_attempted":genuine_attempted,"cross_subject_verifications_attempted":impostor_attempted,"verifications_completed":verified,"genuine_completed":genuine_completed,"cross_subject_completed":impostor_completed,"genuine_verification_failures":genuine_attempted-genuine_completed,"cross_subject_verification_failures":impostor_attempted-impostor_completed,"false_nonmatches":false_nonmatch,"false_matches":false_match,"cross_subject_fmr":(impostor_completed>0).then(||false_match as f64/impostor_completed as f64),"fnmr":(genuine_completed>0).then(||false_nonmatch as f64/genuine_completed as f64)}));
        }
    }
    let mut identification = Vec::new();
    let candidates: Vec<_> = groups.values().filter_map(|g| g.first().copied()).collect();
    ensure!(
        candidates.len() <= 1000,
        "identification gallery exceeds SDK's 1000-candidate limit"
    );
    for &threshold in grid {
        let (mut attempted, mut failed, mut correct_first, mut false_identifications, mut no_match) =
            (0usize, 0usize, 0usize, 0usize, 0usize);
        let mut ranks: BTreeMap<usize, usize> = BTreeMap::new();
        for probe in samples.iter().filter(|s| s.sample.role == "probe") {
            attempted += 1;
            let gallery: Option<Vec<_>> = candidates.iter().map(|s| s.template.clone()).collect();
            let Some(gallery) = gallery else {
                failed += 1;
                continue;
            };
            let Some(template) = &probe.template else {
                failed += 1;
                continue;
            };
            if gallery.is_empty() {
                no_match += 1;
                continue;
            }
            match engine.identify(template, &gallery, threshold, gallery.len()) {
                Err(_) => failed += 1,
                Ok(found) => {
                    if let Some(first) = found.first() {
                        let sample = candidates[first.index].sample;
                        if sample.subject_id == probe.sample.subject_id
                            && sample.finger_id == probe.sample.finger_id
                        {
                            correct_first += 1;
                        } else {
                            false_identifications += 1;
                        }
                    } else {
                        no_match += 1;
                    }
                    if let Some(rank) = found.iter().position(|v| {
                        let sample = candidates[v.index].sample;
                        sample.subject_id == probe.sample.subject_id
                            && sample.finger_id == probe.sample.finger_id
                    }) {
                        *ranks.entry(rank + 1).or_default() += 1;
                    }
                }
            }
        }
        identification.push(json!({"threshold":threshold,"attempted_probes":attempted,"operation_or_extraction_failures":failed,"correct_first":correct_first,"false_identifications":false_identifications,"no_match":no_match,"correct_finger_rank_histogram":ranks}));
    }
    Ok(json!({"enrollment":result,"identification":identification}))
}
pub(crate) fn run(manifest_path: &Path, output: &Path) -> Result<()> {
    ensure!(!output.exists(), "output directory already exists");
    let (manifest, root, digest) = dataset::load(manifest_path)?;
    let engine = RecognitionEngine {
        executable: dataset::resolve(&root, &manifest.engine_path),
        timeout: Duration::from_millis(manifest.timeout_ms),
    };
    ensure!(
        engine.executable.is_file(),
        "recognition helper unavailable"
    );
    let engine_hash = format!("{:x}", Sha256::digest(fs::read(&engine.executable)?));
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(output)?;
    let development = prepare(&manifest, &root, "development", &engine)?;
    let (dev_scores, dev_counts) = comparisons(&development, &engine)?;
    let genuine = dev_scores.iter().filter(|s| s.genuine).count();
    let impostor = dev_scores.len() - genuine;
    if genuine == 0 || impostor == 0 {
        write_new(
            output,
            "preflight-failed.json",
            &json!({"status":"failed","reason":"development split lacks completed eligible genuine/impostor comparisons","counts":dev_counts,"genuine_completed":genuine,"cross_subject_impostor_completed":impostor}),
        )?;
        anyhow::bail!("development comparisons insufficient; held-out evaluation was not run");
    }
    let identity = development
        .iter()
        .find_map(|sample| sample.template.as_ref())
        .context("no development templates")?
        .identity
        .clone();
    ensure!(
        development
            .iter()
            .filter_map(|sample| sample.template.as_ref())
            .all(|template| template.identity == identity),
        "helper identity changed during development extraction"
    );
    let authority_hash = format!(
        "{:x}",
        Sha256::digest(dataset::read_bounded(
            &dataset::resolve(&root, &manifest.data_use_authority),
            1024 * 1024
        )?)
    );
    let evidence_hashes: Vec<_> = manifest.samples.iter().map(|sample| {
        Ok(json!({"image_sha256":sample.sha256,"scale_evidence_sha256":format!("{:x}",Sha256::digest(
            dataset::read_bounded(&dataset::resolve(&root,&sample.scale_evidence),64*1024)?))}))
    }).collect::<Result<_>>()?;
    let grid =
        match metrics::threshold_grid(&dev_scores.iter().map(|s| s.score).collect::<Vec<_>>()) {
            Ok(value) => value,
            Err(error) => {
                write_new(
                    output,
                    "preflight-failed.json",
                    &json!({"status":"failed","reason":error.to_string(),"counts":dev_counts}),
                )?;
                return Err(error);
            }
        };
    // Persist the frozen grid before extracting/scoring held-out images.
    write_new(
        output,
        "frozen-configuration.json",
        &json!({"schema_version":1,"manifest_sha256":digest,"helper_executable_sha256":engine_hash,"engine_identity":identity,"data_use_authority_sha256":authority_hash,"scale_evidence":evidence_hashes,"seed":manifest.seed,"thresholds":grid,"threshold_grid":"positive development empirical quantiles, floor((n-1)*p/100), p=0..100, plus next representable above maximum","sdk_package_version":env!("CARGO_PKG_VERSION"),"bootstrap_replicates":2000}),
    )?;
    let evaluation = prepare(&manifest, &root, "evaluation", &engine)?;
    ensure!(
        evaluation
            .iter()
            .filter_map(|sample| sample.template.as_ref())
            .all(|template| template.identity == identity),
        "helper identity changed before held-out extraction"
    );
    let (eval_scores, eval_counts) = comparisons(&evaluation, &engine)?;
    let evaluation_decisions = decisions(&evaluation, &grid, &engine)?;
    ensure!(
        format!("{:x}", Sha256::digest(fs::read(&engine.executable)?)) == engine_hash,
        "helper executable changed during evaluation"
    );
    let report = json!({"schema_version":1,"status":"completed_exploratory_evaluation","manifest_sha256":digest,
        "development":{"counts":dev_counts,"threshold_results":summarize(&dev_scores,&grid,subjects(&development).len(),manifest.seed,false)?},
        "evaluation":{"counts":eval_counts,"threshold_results":summarize(&eval_scores,&grid,subjects(&evaluation).len(),manifest.seed,true)?,"decisions":evaluation_decisions},
        "limitations":["No production threshold selected; cross-subject FMR does not cover same-subject different fingers.","No reader performance, demographic fairness, liveness or certification claim; a representative corpus and independent review are required.","Failed samples and operations remain reported; no bootstrap interval proves a zero population error."]});
    write_new(output, "report.json", &report)?;
    println!("Exploratory aggregate report written; no production operating point selected.");
    Ok(())
}
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let result = if args.len() == 2 {
        run(Path::new(&args[0]), Path::new(&args[1]))
    } else {
        Err(anyhow::anyhow!(
            "usage: evaluate_recognition MANIFEST OUTPUT_DIRECTORY"
        ))
    };
    if let Err(error) = result.context("evaluation did not complete") {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}
