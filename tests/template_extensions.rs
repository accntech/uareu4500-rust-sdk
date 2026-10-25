// SPDX-License-Identifier: GPL-3.0-or-later
use uareu::template_io::{
    self, ExtendedMultiViewRecord, FingerprintView, MultiViewRecord, TemplateExtension,
    TemplateFormat,
};
fn record() -> ExtendedMultiViewRecord {
    ExtendedMultiViewRecord {
        record: MultiViewRecord {
            width: 20,
            height: 20,
            pixels_per_cm_x: 197,
            pixels_per_cm_y: 197,
            vendor_id: 1,
            subformat: 0,
            device_info: 0,
            views: vec![FingerprintView {
                finger_position: 1,
                view_number: 0,
                impression: 0,
                quality: 80,
                minutiae: vec![],
            }],
        },
        extensions: vec![vec![
            TemplateExtension {
                type_code: 0x101,
                data: vec![0, 255, 17],
            },
            TemplateExtension {
                type_code: 0x102,
                data: vec![],
            },
        ]],
    }
}
#[test]
fn format_specific_lengths_preserve_order_and_old_apis_reject_extensions() {
    for (format, header) in [
        (TemplateFormat::Ansi378_2004, 26),
        (TemplateFormat::Iso19794_2_2005, 24),
    ] {
        let original = record();
        let bytes = template_io::encode_extended(&original, format).unwrap();
        assert_eq!(&bytes[header + 4..header + 6], &11u16.to_be_bytes());
        assert_eq!(
            &bytes[header + 8..header + 10],
            &(if format == TemplateFormat::Ansi378_2004 {
                7u16
            } else {
                3u16
            })
            .to_be_bytes()
        );
        let restored = template_io::decode_extended(&bytes, format).unwrap();
        assert_eq!(restored.extensions, original.extensions);
        assert_eq!(
            template_io::encode_extended(&restored, format).unwrap(),
            bytes
        );
        assert!(template_io::decode_views(&bytes, format).is_err());
        assert!(template_io::decode(&bytes, format).is_err());
        for n in 0..bytes.len() {
            assert!(
                template_io::decode_extended(&bytes[..n], format).is_err(),
                "prefix {n}"
            );
        }
        let mut malformed = bytes.clone();
        malformed[header + 5] = 10;
        assert!(template_io::decode_extended(&malformed, format).is_err());
        let mut malformed = bytes.clone();
        malformed[header + 9] = 255;
        assert!(template_io::decode_extended(&malformed, format).is_err());
    }
}
#[test]
fn bounds_reserved_types_and_atomic_editing() {
    for format in [
        TemplateFormat::Ansi378_2004,
        TemplateFormat::Iso19794_2_2005,
    ] {
        for reserved in [0, 4, 0x100, 0x200] {
            let mut bad = record();
            bad.extensions[0][0].type_code = reserved;
            assert!(template_io::encode_extended(&bad, format).is_err());
        }
        let mut bad = record();
        bad.extensions.clear();
        assert!(template_io::encode_extended(&bad, format).is_err());
        let mut bad = record();
        bad.extensions[0][0].data = vec![0; 65532];
        assert!(template_io::encode_extended(&bad, format).is_err());
        let mut value = record();
        let before = template_io::encode_extended(&value, format).unwrap();
        assert!(
            value
                .edit(format, |r| r.extensions[0][0].type_code = 0)
                .is_err()
        );
        assert_eq!(
            template_io::encode_extended(&value, format).unwrap(),
            before
        );
        value
            .edit(format, |r| r.extensions[0][0].data.push(42))
            .unwrap();
        assert_eq!(value.extensions[0][0].data, [0, 255, 17, 42]);
    }
}
#[test]
fn ansi_long_total_length_and_empty_extension_compatibility() {
    let mut value = record();
    value.extensions[0] = vec![TemplateExtension {
        type_code: 0x101,
        data: vec![23; 65531],
    }];
    let bytes = template_io::encode_extended(&value, TemplateFormat::Ansi378_2004).unwrap();
    assert_eq!(&bytes[8..10], &[0, 0]);
    assert_eq!(
        template_io::decode_extended(&bytes, TemplateFormat::Ansi378_2004)
            .unwrap()
            .extensions,
        value.extensions
    );
    value.extensions[0].clear();
    for format in [
        TemplateFormat::Ansi378_2004,
        TemplateFormat::Iso19794_2_2005,
    ] {
        let plain = template_io::encode_views(&value.record, format).unwrap();
        assert_eq!(template_io::encode_extended(&value, format).unwrap(), plain);
        assert_eq!(
            template_io::decode_extended(&plain, format)
                .unwrap()
                .extensions,
            vec![vec![]]
        );
    }
}

#[path = "support/reference_process.rs"]
mod process;
#[test]
#[ignore = "requires pinned FingerprintIO1.3.3 jars and JDK; verifies opaque extension transport"]
fn fingerprintio_extension_envelopes_are_bidirectional() -> anyhow::Result<()> {
    use anyhow::{Context, ensure};
    use sha2::{Digest, Sha256};
    use std::{fs, path::PathBuf, time::Duration};
    let cp = std::env::var_os("UAREU_FINGERPRINTIO_CLASSPATH").context("set pinned classpath")?;
    let jars = std::env::split_paths(&cp).collect::<Vec<_>>();
    let pins = [
        (
            "fingerprintio-1.3.3.jar",
            "daebcf2dd68726c543e334b16fd09d6a7aa709ff0e807e643c251697fd784ca7",
        ),
        (
            "stagean-1.3.0.jar",
            "19a77e39c6db01950c6caedc72323995f4be38368a16f72ffce5ea49ef3c7650",
        ),
        (
            "noexception-1.9.1.jar",
            "29b002c8ccb17e73f06e2a237b083a38ca8e83cf833e0483c2366ff492965a8d",
        ),
        (
            "closeablescope-1.0.1.jar",
            "3e3554e56a831a68e168ee0e267b7e5ede9a9623c5814065888de49de0debfc8",
        ),
        (
            "slf4j-api-2.0.9.jar",
            "0818930dc8d7debb403204611691da58e49d42c50b6ffcfdce02dadb7c3c2b6c",
        ),
    ];
    ensure!(jars.len() == pins.len(), "expected five pinned jars");
    for (name, hash) in pins {
        let p = jars
            .iter()
            .find(|p| p.file_name().is_some_and(|f| f == name))
            .context("missing pinned jar")?;
        ensure!(
            format!("{:x}", Sha256::digest(fs::read(p)?)) == hash,
            "jar hash mismatch"
        );
    }
    let workspace = process::Workspace::new()?;
    let source = workspace.0.join("ExtensionProbe.java");
    fs::write(
        &source,
        r#"
import java.nio.file.*;
import com.machinezoo.fingerprintio.ansi378v2004.*;
import com.machinezoo.fingerprintio.iso19794p2v2005.*;
public class ExtensionProbe {
 public static void main(String[] args) throws Exception {
  boolean ansi=args[1].equals("ansi"); byte[] result;
  if(args[0].equals("zonal")) {
   Iso19794p2v2005Template t=new Iso19794p2v2005Template();t.width=20;t.height=20;t.resolutionX=197;t.resolutionY=197;
   Iso19794p2v2005Fingerprint f=new Iso19794p2v2005Fingerprint();f.position=Iso19794p2v2005Position.values()[1];f.quality=80;
   f.zones=new Iso19794p2v2005ZonalExtension();f.zones.zoneWidth=20;f.zones.zoneHeight=20;f.zones.bits=8;f.zones.quality=new int[]{80};t.fingerprints.add(f);result=t.toByteArray();
  } else if(args[0].equals("produce")) {
   if(ansi) {
    Ansi378v2004Template t=new Ansi378v2004Template();t.width=20;t.height=20;t.resolutionX=197;t.resolutionY=197;t.vendorId=1;
    for(int i=0;i<2;i++){Ansi378v2004Fingerprint f=new Ansi378v2004Fingerprint();f.position=Ansi378v2004Position.values()[1];f.view=i;f.quality=80;
     for(int j=0;j<2;j++){Ansi378v2004Extension e=new Ansi378v2004Extension();e.type=0x101+j;e.data=j==0?new byte[]{0,(byte)255,17}:new byte[0];f.extensions.add(e);}t.fingerprints.add(f);}
    result=t.toByteArray();
   } else {
    Iso19794p2v2005Template t=new Iso19794p2v2005Template();t.width=20;t.height=20;t.resolutionX=197;t.resolutionY=197;
    for(int i=0;i<2;i++){Iso19794p2v2005Fingerprint f=new Iso19794p2v2005Fingerprint();f.position=Iso19794p2v2005Position.values()[1];f.view=i;f.quality=80;
     for(int j=0;j<2;j++){Iso19794p2v2005Extension e=new Iso19794p2v2005Extension();e.type=0x101+j;e.data=j==0?new byte[]{0,(byte)255,17}:new byte[0];f.extensions.add(e);}t.fingerprints.add(f);}
    result=t.toByteArray();
   }
  } else {
   byte[] bytes=Files.readAllBytes(Path.of(args[2]));
   result=ansi?new Ansi378v2004Template(bytes).toByteArray():new Iso19794p2v2005Template(bytes).toByteArray();
  }
  Files.write(Path.of(args[3]),result);
 }
}
"#,
    )?;
    let javac = PathBuf::from(std::env::var_os("UAREU_JAVAC_EXE").context("set UAREU_JAVAC_EXE")?);
    let java = PathBuf::from(std::env::var_os("UAREU_JAVA_EXE").context("set UAREU_JAVA_EXE")?);
    process::run(
        &javac,
        &["-cp".into(), cp.clone(), source.into_os_string()],
        &workspace.0,
        Duration::from_secs(30),
    )?;
    let mut runtime = jars;
    runtime.push(workspace.0.clone());
    let runtime = std::env::join_paths(runtime)?;
    let probe = |mode: &str, name: &str, input: &std::path::Path, output: &std::path::Path| {
        process::run(
            &java,
            &[
                "-cp".into(),
                runtime.clone(),
                "ExtensionProbe".into(),
                mode.into(),
                name.into(),
                input.as_os_str().to_owned(),
                output.as_os_str().to_owned(),
            ],
            &workspace.0,
            Duration::from_secs(30),
        )
    };
    for (name, format) in [
        ("ansi", TemplateFormat::Ansi378_2004),
        ("iso", TemplateFormat::Iso19794_2_2005),
    ] {
        let fixture = workspace.0.join(format!("java-{name}.fmr"));
        probe("produce", name, &fixture, &fixture)?;
        let bytes = fs::read(&fixture)?;
        let imported = template_io::decode_extended(&bytes, format)?;
        ensure!(
            imported.record.views.len() == 2
                && imported.extensions == vec![record().extensions[0].clone(); 2],
            "independent extension fields disagree"
        );
        ensure!(
            template_io::encode_extended(&imported, format)? == bytes,
            "Rust changed independent record"
        );
        let mut ours = record();
        let mut next = ours.record.views[0].clone();
        next.view_number = 1;
        ours.record.views.push(next);
        ours.extensions.push(ours.extensions[0].clone());
        let exported = template_io::encode_extended(&ours, format)?;
        let input = workspace.0.join(format!("rust-{name}.fmr"));
        let output = workspace.0.join(format!("accepted-{name}.fmr"));
        fs::write(&input, &exported)?;
        probe("consume", name, &input, &output)?;
        ensure!(
            fs::read(output)? == exported,
            "independent consumer changed Rust record"
        );
        println!(
            "FingerprintIO1.3.3 {name}: two views, ordered opaque extension payloads bidirectional byte agreement ({} bytes)",
            bytes.len()
        );
    }
    let fixture = workspace.0.join("java-zonal.fmr");
    probe("zonal", "iso", &fixture, &fixture)?;
    let bytes = fs::read(&fixture)?;
    let imported = template_io::decode_extended(&bytes, TemplateFormat::Iso19794_2_2005)?;
    ensure!(
        bytes.len() == 38
            && imported.extensions
                == vec![vec![TemplateExtension {
                    type_code: 3,
                    data: vec![20, 20, 8, 80]
                }]],
        "independent zonal payload disagrees"
    );
    ensure!(
        template_io::encode_extended(&imported, TemplateFormat::Iso19794_2_2005)? == bytes,
        "Rust changed independent zonal record"
    );
    let mut ours = record();
    ours.extensions = imported.extensions;
    let exported = template_io::encode_extended(&ours, TemplateFormat::Iso19794_2_2005)?;
    let input = workspace.0.join("rust-zonal.fmr");
    let output = workspace.0.join("accepted-zonal.fmr");
    fs::write(&input, &exported)?;
    probe("consume", "iso", &input, &output)?;
    ensure!(
        fs::read(output)? == exported,
        "independent consumer changed zonal record"
    );
    println!(
        "FingerprintIO1.3.3 iso: predefined zonal type3 bidirectional byte agreement (38 bytes)"
    );
    Ok(())
}

#[test]
fn iso_zonal_type_three_is_preserved_but_reserved_for_ansi() {
    let mut value = record();
    value.extensions[0] = vec![TemplateExtension {
        type_code: 3,
        data: vec![20, 20, 8, 80],
    }];
    let bytes = template_io::encode_extended(&value, TemplateFormat::Iso19794_2_2005).unwrap();
    assert_eq!(
        template_io::decode_extended(&bytes, TemplateFormat::Iso19794_2_2005)
            .unwrap()
            .extensions,
        value.extensions
    );
    assert!(template_io::encode_extended(&value, TemplateFormat::Ansi378_2004).is_err());
}
