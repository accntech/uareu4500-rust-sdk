// SPDX-License-Identifier: GPL-3.0-or-later
use uareu::{
    ansi381::{self, Ansi381Record},
    image_record::{ImageCompression, ImageView, MultiImageRecord},
    metadata::Resolution,
};
fn sample() -> Ansi381Record {
    Ansi381Record {
        cbeff_product_id: 0x0103_0055,
        appendix_f_compliant: true,
        record: MultiImageRecord {
            capture_device_id: 0x123,
            acquisition_level: 31,
            scale_units: 1,
            scan_resolution_x: 500,
            scan_resolution_y: 500,
            image_resolution_x: 500,
            image_resolution_y: 500,
            compression: ImageCompression::Raw8Bit,
            images: vec![ImageView {
                finger_position: 1,
                view_count: 1,
                view_number: 1,
                quality: 254,
                impression: 0,
                width: 2,
                height: 2,
                data: vec![0, 255, 128, 64],
            }],
        },
    }
}
#[test]
fn ansi_header_offsets_quality_and_scale_roundtrip() {
    let s = sample();
    let b = ansi381::encode(&s).unwrap();
    assert_eq!(b.len(), 54);
    assert_eq!(
        &b[..8],
        b"FIR\0".iter().chain(b"010\0").copied().collect::<Vec<_>>()
    );
    assert_eq!(&b[8..14], &[0, 0, 0, 0, 0, 54]);
    assert_eq!(&b[14..18], &0x01030055u32.to_be_bytes());
    assert_eq!(&b[18..20], &0x8123u16.to_be_bytes());
    assert_eq!(b[43], 254);
    assert_eq!(&b[50..], &[0, 255, 128, 64]);
    let d = ansi381::decode(&b).unwrap();
    assert_eq!(ansi381::encode(&d).unwrap(), b);
    let image = d.raw_image(0).unwrap();
    assert_eq!(image.image.pixels, [0, 255, 128, 64]);
    assert!(matches!(
        image.metadata.resolution,
        Resolution::Known {
            x_ppi: 500.0,
            y_ppi: 500.0,
            ..
        }
    ));
    assert!(uareu::image_record::decode_images(&b).is_err());
}
#[test]
fn malformed_lengths_flags_quality_and_invalid_declarations_rejected() {
    let b = ansi381::encode(&sample()).unwrap();
    for n in 0..b.len() {
        assert!(ansi381::decode(&b[..n]).is_err(), "prefix{n}");
    }
    for (offset, value) in [
        (13, 53),
        (14, 0),
        (18, 0x91),
        (22, 2),
        (32, 16),
        (33, 2),
        (34, 1),
        (39, 17),
        (43, 101),
        (49, 1),
    ] {
        let mut bad = b.clone();
        bad[offset] = value;
        if offset == 14 {
            bad[15] = 0;
        }
        assert!(ansi381::decode(&bad).is_err(), "offset{offset}");
    }
    let mut bad = sample();
    bad.record.images[0].data.pop();
    assert!(ansi381::encode(&bad).is_err());
    let mut bad = sample();
    bad.record.compression = ImageCompression::OpaqueWsq;
    assert!(ansi381::encode(&bad).is_err());
    let mut bad = sample();
    bad.record.images[0].quality = 0;
    assert!(ansi381::encode(&bad).is_err());
    bad.record.images[0].quality = 255;
    assert!(ansi381::encode(&bad).is_err());
    let mut bad = sample();
    bad.cbeff_product_id = 0;
    assert!(ansi381::encode(&bad).is_err());
    let mut bad = sample();
    bad.record.scan_resolution_x = 1001;
    assert!(ansi381::encode(&bad).is_err());
}
#[test]
fn multiview_editing_is_atomic_and_preserves_original_scale_declaration() {
    let mut s = sample();
    s.record.scale_units = 2;
    s.record.image_resolution_x = 197;
    s.record.image_resolution_y = 197;
    s.record.scan_resolution_x = 197;
    s.record.scan_resolution_y = 197;
    s.record.images[0].view_count = 2;
    let mut second = s.record.images[0].clone();
    second.view_number = 2;
    second.quality = 80;
    s.record.images.push(second);
    let b = ansi381::encode(&s).unwrap();
    let d = ansi381::decode(&b).unwrap();
    assert_eq!(d.record.images.len(), 2);
    assert_eq!(d.record.images[1].quality, 80);
    assert_eq!(d.record.image_resolution_x, 197);
    let before = ansi381::encode(&s).unwrap();
    assert!(s.edit(|r| r.record.images[0].width = 0).is_err());
    assert_eq!(ansi381::encode(&s).unwrap(), before);
    s.edit(|r| r.record.images[1].data[0] = 42).unwrap();
    assert_eq!(s.raw_image(1).unwrap().image.pixels[0], 42);
}

#[path = "support/reference_process.rs"]
mod process;
#[test]
#[ignore = "requires pinned UAREU_BIOMDI_SOURCE and UAREU_CC to compile independent NIST reference"]
fn independent_nist_biomdi_producer_consumer_agree() -> anyhow::Result<()> {
    use sha2::{Digest, Sha256};
    use std::{fs, path::PathBuf, time::Duration};
    let root = fs::canonicalize(PathBuf::from(
        std::env::var_os("UAREU_BIOMDI_SOURCE").expect("set pinned BiomDI source"),
    ))?;
    let compiler = PathBuf::from(std::env::var_os("UAREU_CC").expect("set C compiler"));
    for (path, expected) in [
        (
            "common/src/include/biomdi.h",
            "90a1d2a47f528d4af491819e3125e89771c4c5fc7f12c864d72cb2dd76ec59e7",
        ),
        (
            "common/src/include/biomdimacro.h",
            "f3cc22ae5f3500d1570f83f556f9fa6da3deb87657476a30bef32353439e12e9",
        ),
        (
            "common/src/libbiomdi/biomdi.c",
            "0dc31a8e51ec41501ebdef3f98dcf8522cf1abe485120349fc810a99d79db801",
        ),
        (
            "fingerimage/src/include/fir.h",
            "23a6075d08f249d672f9ba70fafad0b6ea2007b4a39427fc98d4419cf13b837c",
        ),
        (
            "fingerimage/src/libfir/fir.c",
            "d2a3993e820d7d63c28834ce3b9587282140ca628c9db9a13feff8bc1b6f918e",
        ),
        (
            "fingerimage/src/libfir/fivr.c",
            "d49c4cd09e492178d4fe75ff8df40f3b609b73632f44ad300bf55a5d2feb05fa",
        ),
        (
            "fingerimage/src/libfir/validate.c",
            "a3fadfb84db91e0161c1283a66d73f630d7b5650c00533d94912dd841970fba1",
        ),
    ] {
        assert_eq!(
            format!("{:x}", Sha256::digest(fs::read(root.join(path))?)),
            expected,
            "reference source drift: {path}"
        );
    }
    let workspace = process::Workspace::new()?;
    let source = workspace.0.join("probe.c");
    let probe = workspace.0.join("probe");
    fs::write(
        &source,
        r#"
#include <sys/queue.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <biomdi.h>
#include <biomdimacro.h>
#include <fir.h>
int main(int argc,char **argv) {
  FIR *r=NULL; FIVR *v=NULL; FILE *f=NULL; int i;
  if(argc<3 || new_fir(FIR_STD_ANSI,&r))return 1;
  if(strcmp(argv[1],"generate")==0) {
    memcpy(r->format_id,"FIR",4);memcpy(r->spec_version,"010",4);r->record_length=72;
    r->product_identifier_owner=0x0103;r->product_identifier_type=0x55;r->scanner_id=0x123;r->compliance=8;
    r->image_acquisition_level=31;r->num_fingers_or_palm_images=2;r->scale_units=1;
    r->x_scan_resolution=r->y_scan_resolution=r->x_image_resolution=r->y_image_resolution=500;r->pixel_depth=8;
    for(i=0;i<2;++i) {
      if(new_fivr(&v))return 2;v->length=18;v->finger_palm_position=(i==0?1:7);v->count_of_views=v->view_number=1;
      v->quality=254;v->horizontal_line_length=v->vertical_line_length=2;
      char *pixels=malloc(4);if(!pixels)return 3;pixels[0]=0;pixels[1]=(char)255;pixels[2]=(char)128;pixels[3]=64;
      add_image_to_fivr(pixels,4,v);add_fivr_to_fir(v,r);
    }
    f=fopen(argv[2],"wb");
  } else {
    if(argc!=4)return 4;f=fopen(argv[2],"rb");if(!f)return 5;
    if(read_fir(f,r)!=READ_OK || fgetc(f)!=EOF)return 6;fclose(f);f=NULL;
    f=fopen(argv[3],"wb");
  }
  if(!f || validate_fir(r)!=VALIDATE_OK)return 7;
  TAILQ_FOREACH(v,&r->finger_views,list)if(validate_fivr(v)!=VALIDATE_OK)return 8;
  if(write_fir(f,r)!=WRITE_OK)return 9;fclose(f);free_fir(r);return 0;
}
"#,
    )?;
    let mut args: Vec<std::ffi::OsString> = vec![
        "-std=c99".into(),
        "-I".into(),
        root.join("common/src/include").into_os_string(),
        "-I".into(),
        root.join("fingerimage/src/include").into_os_string(),
        source.into_os_string(),
    ];
    for p in [
        "common/src/libbiomdi/biomdi.c",
        "fingerimage/src/libfir/fir.c",
        "fingerimage/src/libfir/fivr.c",
        "fingerimage/src/libfir/validate.c",
    ] {
        args.push(root.join(p).into_os_string());
    }
    args.extend(["-o".into(), probe.clone().into_os_string()]);
    process::run(&compiler, &args, &workspace.0, Duration::from_secs(30))?;
    let generated = workspace.0.join("nist.fir");
    process::run(
        &probe,
        &["generate".into(), generated.clone().into_os_string()],
        &workspace.0,
        Duration::from_secs(5),
    )?;
    let nist = fs::read(&generated)?;
    let parsed = ansi381::decode(&nist)?;
    assert_eq!(parsed.record.images.len(), 2);
    assert_eq!(parsed.cbeff_product_id, 0x01030055);
    assert!(parsed.appendix_f_compliant);
    assert_eq!(ansi381::encode(&parsed)?, nist);
    let sdk = workspace.0.join("sdk.fir");
    let copied = workspace.0.join("copied.fir");
    let bytes = ansi381::encode(&sample())?;
    fs::write(&sdk, &bytes)?;
    process::run(
        &probe,
        &[
            "copy".into(),
            sdk.into_os_string(),
            copied.clone().into_os_string(),
        ],
        &workspace.0,
        Duration::from_secs(5),
    )?;
    assert_eq!(fs::read(copied)?, bytes);
    eprintln!(
        "NIST BiomDI producer72bytes and SDK54bytes independently validated/reserialized; sourcecommitacf76a030b0193faafb55fb469b27741b6aa8874"
    );
    Ok(())
}
