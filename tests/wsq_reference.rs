// SPDX-License-Identifier: GPL-3.0-or-later
//! Real NBIS integration is opt-in and cannot be satisfied by internal round trips.
#[path = "support/reference_process.rs"]
mod process;
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, time::Duration};
use uareu::{
    frame::Image,
    metadata::{CapturedImage, Resolution},
    wsq::{WsqCodec, WsqTargetOptions, WsqTargetStatus},
};
#[test]
#[ignore = "requires UAREU_CWSQ_EXE and UAREU_DWSQ_EXE; executes installed NBIS"]
fn real_nbis_adapter_agrees_with_direct_decoder_and_encoder() -> Result<()> {
    let encoder = PathBuf::from(std::env::var_os("UAREU_CWSQ_EXE").context("set UAREU_CWSQ_EXE")?);
    let decoder = PathBuf::from(std::env::var_os("UAREU_DWSQ_EXE").context("set UAREU_DWSQ_EXE")?);
    let codec = WsqCodec {
        encoder: encoder.clone(),
        decoder: decoder.clone(),
        timeout: Duration::from_secs(30),
    };
    let mut image = CapturedImage::new(Image {
        width: 256,
        height: 256,
        pixels: (0..65536)
            .map(|i| {
                let (x, y) = (i % 256, i / 256);
                (40 + ((x / 4 + y / 9) % 12) * 15) as u8
            })
            .collect(),
    });
    image.metadata.resolution = Resolution::declared(500.0, 500.0)?;
    let encoded = codec.encode(&image, 0.75)?;
    let decoded = codec.decode(&encoded)?;
    let workspace = process::Workspace::new()?;
    let input = workspace.0.join("reference.wsq");
    fs::write(&input, &encoded)?;
    process::run(
        &decoder,
        &["raw".into(), input.into_os_string(), "-raw_out".into()],
        &workspace.0,
        Duration::from_secs(30),
    )?;
    ensure!(
        decoded.image.pixels == fs::read(workspace.0.join("reference.raw"))?,
        "adapter/direct decoder pixels differ"
    );
    ensure!(
        decoded.image.width == 256 && decoded.image.height == 256,
        "decoded geometry differs"
    );
    ensure!(
        matches!(decoded.metadata.resolution,Resolution::Known{x_ppi,y_ppi,..} if x_ppi==500.0&&y_ppi==500.0),
        "decoder did not preserve declared PPI"
    );
    let raw = workspace.0.join("direct.raw");
    fs::write(&raw, &image.image.pixels)?;
    process::run(
        &encoder,
        &[
            "0.75".into(),
            "wsq".into(),
            raw.into_os_string(),
            "-raw_in".into(),
            "256,256,8,500".into(),
        ],
        &workspace.0,
        Duration::from_secs(30),
    )?;
    let direct = codec.decode(&fs::read(workspace.0.join("direct.wsq"))?)?;
    ensure!(
        direct.image == decoded.image,
        "adapter/direct encoding produce different decoded pixels"
    );
    for n in [0, 1, encoded.len() / 2, encoded.len() - 1] {
        ensure!(
            codec.decode(&encoded[..n]).is_err(),
            "truncated WSQ accepted"
        );
    }
    ensure!(
        codec.decode(&[0xff, 0xa0, 0xff, 0x01, 0xff, 0xa1]).is_err(),
        "malformed WSQ accepted"
    );
    let short = WsqCodec {
        encoder: encoder.clone(),
        decoder: decoder.clone(),
        timeout: Duration::from_nanos(1),
    };
    ensure!(
        short.encode(&image, 0.75).is_err(),
        "codec deadline not enforced"
    );
    let target_options = WsqTargetOptions {
        target_bytes: 4000,
        max_encodes: 8,
        deadline: Duration::from_secs(10),
    };
    let targeted = codec.encode_to_target(&image, target_options)?;
    ensure!(
        targeted.status == WsqTargetStatus::Met && targeted.bytes.len() <= 4000,
        "real measured target search did not fit"
    );
    ensure!(
        codec.decode(&targeted.bytes)?.image.width == 256,
        "target output not decodable"
    );
    let impossible = codec.encode_to_target(
        &image,
        WsqTargetOptions {
            target_bytes: 1,
            ..target_options
        },
    )?;
    ensure!(
        impossible.status == WsqTargetStatus::Unattainable
            && impossible.bytes.len() > 1
            && impossible.encodes == 2,
        "real impossible target outcome was not explicit"
    );
    let mut measured_sizes = Vec::new();
    for rate in [0.1, 0.75, 2.25] {
        let data = codec.encode(&image, rate)?;
        let output = codec.decode(&data)?;
        ensure!(
            output.image.pixels.len() == 65536,
            "rate sweep decode failed"
        );
        measured_sizes.push(serde_json::json!({"bitrate":rate, "bytes":data.len()}));
    }
    let mse = image
        .image
        .pixels
        .iter()
        .zip(&decoded.image.pixels)
        .map(|(&a, &b)| (f64::from(a) - f64::from(b)).powi(2))
        .sum::<f64>()
        / 65536.0;
    println!(
        "{}",
        serde_json::json!({"status":"passed","fixture":"synthetic_256_grid_declared_500ppi","input_sha256":format!("{:x}",Sha256::digest(&image.image.pixels)),"encoder_sha256":format!("{:x}",Sha256::digest(fs::read(encoder)?)),"decoder_sha256":format!("{:x}",Sha256::digest(fs::read(decoder)?)),"bitrate":0.75,"encoded_bytes":encoded.len(),"mse":mse,"rate_sweep":measured_sizes,"target":{"bytes":targeted.bytes.len(),"bitrate":targeted.bitrate,"encodes":targeted.encodes},"unattainable":{"bytes":impossible.bytes.len(),"encodes":impossible.encodes},"limitation":"adapter agreement, not WSQ certification or reader scale"})
    );
    Ok(())
}
