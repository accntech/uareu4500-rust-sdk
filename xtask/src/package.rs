// SPDX-License-Identifier: GPL-3.0-or-later
//! Source-inclusive, offline and reproducible GPL release packaging.
use crate::common::{TempDir, read_required, sha256, walk_files};
use anyhow::{Context, Result, bail};
use flate2::{Compression, GzBuilder};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

type Receipt = BTreeMap<String, String>;
const VENDOR_CONFIG: &str = "[alias]\nxtask = \"run --locked --package xtask --\"\n\n[source.crates-io]\nreplace-with = \"vendored-sources\"\n[source.vendored-sources]\ndirectory = \"vendor\"\n[net]\noffline = true\n";
const BUILD_ENV: &[&str] = &[
    "RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "MACOSX_DEPLOYMENT_TARGET",
    "CARGO_PROFILE_RELEASE_LTO",
    "CARGO_PROFILE_RELEASE_CODEGEN_UNITS",
    "CARGO_PROFILE_RELEASE_OPT_LEVEL",
    "CARGO_PROFILE_RELEASE_DEBUG",
    "CARGO_PROFILE_RELEASE_STRIP",
    "CARGO_PROFILE_RELEASE_PANIC",
];
fn parse(path: &Path) -> Result<toml::Value> {
    toml::from_str(std::str::from_utf8(&read_required(path)?)?).with_context(|| {
        format!(
            "cannot parse {}; restore valid TOML before packaging",
            path.display()
        )
    })
}
fn relative(path: &Path, root: &Path) -> Result<String> {
    Ok(path
        .strip_prefix(root)?
        .to_str()
        .context("source path is not UTF-8; rename it before packaging")?
        .replace('\\', "/"))
}
fn project_paths(root: &Path, vendor: bool) -> Result<BTreeMap<String, PathBuf>> {
    let mut paths = BTreeMap::new();
    for name in [
        "Cargo.toml",
        "Cargo.lock",
        "LICENSE",
        "NOTICE",
        "README.md",
        "build.rs",
        "rust-toolchain.toml",
        ".gitignore",
        ".gitattributes",
        "REBUILD.sh",
        "fixtures/docs/PROTOCOL_FACTS.md",
        ".cargo/config.toml",
        "xtask/Cargo.toml",
    ] {
        let path = root.join(name);
        if path.symlink_metadata().is_ok() {
            if path.symlink_metadata()?.file_type().is_symlink() || !path.is_file() {
                bail!("source input {name} is not a regular file; restore a regular file");
            }
            paths.insert(name.into(), path);
        }
    }
    for (group, extensions) in [
        ("src", &["rs"][..]),
        ("tests", &["rs"][..]),
        ("xtask/src", &["rs"][..]),
        ("fixtures", &["wire", "gray"][..]),
        ("udev", &["rules"][..]),
        (".github/workflows", &["yml", "yaml"][..]),
        ("licenses", &[][..]),
    ] {
        let directory = root.join(group);
        if directory.symlink_metadata().is_err() {
            continue;
        }
        for path in walk_files(&directory)? {
            let name = relative(&path, root)?;
            if name
                .split('/')
                .any(|part| [".git", ".agents", "target", "dist", "__pycache__"].contains(&part))
            {
                continue;
            }
            if extensions.is_empty()
                || path
                    .extension()
                    .and_then(|x| x.to_str())
                    .is_some_and(|ext| extensions.contains(&ext))
            {
                paths.insert(name, path);
            }
        }
    }
    if vendor && root.join("vendor").symlink_metadata().is_ok() {
        for path in walk_files(&root.join("vendor"))? {
            paths.insert(relative(&path, root)?, path);
        }
    }
    Ok(paths)
}
fn source_manifest(root: &Path) -> Result<Receipt> {
    project_paths(root, true)?
        .into_iter()
        .map(|(name, path)| Ok((name, sha256(&fs::read(path)?))))
        .collect()
}
fn reject_external_dependencies(value: &toml::Value) -> Result<()> {
    if let Some(table) = value.as_table() {
        for (key, item) in table {
            if [
                "dependencies",
                "dev-dependencies",
                "build-dependencies",
                "patch",
                "replace",
            ]
            .contains(&key.as_str())
            {
                check_dependency_table(item)?;
            }
            reject_external_dependencies(item)?;
        }
    }
    Ok(())
}
fn check_dependency_table(value: &toml::Value) -> Result<()> {
    if let Some(table) = value.as_table() {
        if table.contains_key("path") || table.contains_key("git") {
            bail!(
                "unsupported path/git dependency; use locked registry dependencies before releasing"
            );
        }
        for value in table.values() {
            check_dependency_table(value)?;
        }
    }
    Ok(())
}
fn snapshot_project(root: &Path, destination: &Path) -> Result<()> {
    if destination.exists() {
        bail!("snapshot destination exists; use a fresh temporary directory");
    }
    if root.join("vendor").exists() {
        validate_vendor(root)?;
    }
    for manifest in ["Cargo.toml", "xtask/Cargo.toml"] {
        reject_external_dependencies(&parse(&root.join(manifest))?)?;
    }
    fs::create_dir_all(destination)?;
    for (name, source) in project_paths(root, true)? {
        let target = destination.join(name);
        fs::create_dir_all(target.parent().context("source has no parent")?)?;
        fs::copy(source, target)?;
    }
    for name in [
        "Cargo.toml",
        "Cargo.lock",
        "LICENSE",
        "NOTICE",
        "README.md",
        "xtask/Cargo.toml",
    ] {
        read_required(&destination.join(name))?;
    }
    if !destination.join("src/main.rs").is_file()
        || !destination.join("xtask/src/main.rs").is_file()
    {
        bail!("snapshot lacks Rust entry points; restore src/main.rs and xtask/src/main.rs");
    }
    Ok(())
}
fn validate_vendor(root: &Path) -> Result<Vec<Value>> {
    let config = parse(&root.join(".cargo/config.toml"))?;
    if config
        .get("source")
        .and_then(|v| v.get("crates-io"))
        .and_then(|v| v.get("replace-with"))
        .and_then(|v| v.as_str())
        != Some("vendored-sources")
        || config
            .get("source")
            .and_then(|v| v.get("vendored-sources"))
            .and_then(|v| v.get("directory"))
            .and_then(|v| v.as_str())
            != Some("vendor")
        || config
            .get("net")
            .and_then(|v| v.get("offline"))
            .and_then(|v| v.as_bool())
            != Some(true)
    {
        bail!(
            "vendor configuration must use relative vendor/ and net.offline=true; regenerate source packaging"
        );
    }
    let lock = parse(&root.join("Cargo.lock"))?;
    let mut expected = BTreeMap::new();
    for package in lock
        .get("package")
        .and_then(|v| v.as_array())
        .context("lockfile has no packages; regenerate Cargo.lock")?
    {
        let name = package
            .get("name")
            .and_then(|v| v.as_str())
            .context("lockfile package lacks name")?;
        let version = package
            .get("version")
            .and_then(|v| v.as_str())
            .context("lockfile package lacks version")?;
        let Some(source) = package.get("source").and_then(|v| v.as_str()) else {
            if !["uareu", "xtask"].contains(&name) {
                bail!(
                    "unsupported local dependency {name}; include only reviewed workspace packages"
                );
            }
            continue;
        };
        let checksum = package
            .get("checksum")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !source.starts_with("registry+")
            || checksum.len() != 64
            || !checksum.bytes().all(|b| b.is_ascii_hexdigit())
        {
            bail!(
                "unreviewed dependency source/checksum; use locked registry sources before releasing"
            );
        }
        if expected
            .insert((name.to_owned(), version.to_owned()), checksum.to_owned())
            .is_some()
        {
            bail!("duplicate locked package; review Cargo.lock before releasing");
        }
    }
    if expected.is_empty() {
        bail!("vendor has no locked dependencies; regenerate with cargo vendor --locked");
    }
    let mut found = BTreeMap::new();
    for entry in fs::read_dir(root.join("vendor"))
        .context("vendor directory missing; run cargo fetch --locked and retry packaging")?
    {
        let directory = entry?.path();
        if directory.symlink_metadata()?.file_type().is_symlink() || !directory.is_dir() {
            bail!("vendor has an unexpected entry; regenerate with cargo vendor");
        }
        let manifest = parse(&directory.join("Cargo.toml"))?;
        let info = manifest
            .get("package")
            .context("vendor manifest lacks package; regenerate vendor")?;
        let identity = (
            info.get("name")
                .and_then(|v| v.as_str())
                .context("vendor lacks name")?
                .to_owned(),
            info.get("version")
                .and_then(|v| v.as_str())
                .context("vendor lacks version")?
                .to_owned(),
        );
        let recorded: Value =
            serde_json::from_slice(&read_required(&directory.join(".cargo-checksum.json"))?)?;
        let checksum = recorded
            .get("package")
            .and_then(Value::as_str)
            .unwrap_or("");
        if expected.get(&identity).map(String::as_str) != Some(checksum)
            || found
                .insert(identity.clone(), checksum.to_owned())
                .is_some()
        {
            bail!("vendor package checksum does not match lockfile; regenerate locked sources");
        }
        let files = recorded
            .get("files")
            .and_then(Value::as_object)
            .filter(|v| !v.is_empty())
            .context("vendor file checksum inventory missing; regenerate locked sources")?;
        let mut actual = BTreeMap::new();
        for path in walk_files(&directory)? {
            let name = relative(&path, &directory)?;
            if name != ".cargo-checksum.json" {
                actual.insert(name, sha256(&fs::read(path)?));
            }
        }
        if actual.len() != files.len()
            || actual
                .iter()
                .any(|(name, hash)| files.get(name).and_then(Value::as_str) != Some(hash.as_str()))
        {
            bail!(
                "vendor file checksum/inventory mismatch for {}; regenerate locked sources",
                identity.0
            );
        }
    }
    if found != expected {
        bail!("vendor package inventory does not match Cargo.lock; regenerate locked sources");
    }
    Ok(found.into_iter().map(|((name, version), checksum)| json!({"name":name,"version":version,"checksum":checksum})).collect())
}
fn command(root: &Path, program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .with_context(|| format!("cannot start {program}; install it and retry"))?;
    if !output.status.success() {
        bail!(
            "{program} {} failed; fix the reported toolchain/dependency error and retry:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}
fn validate_binary(data: &[u8], target: &str) -> Result<()> {
    let valid = match target {
        "x86_64-unknown-linux-gnu" | "aarch64-unknown-linux-gnu" => {
            let cpu = if target.starts_with("x86_64") {
                62u16
            } else {
                183
            };
            data.len() >= 64
                && data.starts_with(b"\x7fELF\x02\x01")
                && data[18..20] == cpu.to_le_bytes()
        }
        "x86_64-apple-darwin" | "aarch64-apple-darwin" => {
            let cpu = if target.starts_with("x86_64") {
                0x01000007u32
            } else {
                0x0100000c
            };
            data.len() >= 32
                && data.starts_with(b"\xcf\xfa\xed\xfe")
                && data[4..8] == cpu.to_le_bytes()
        }
        _ => bail!("unsupported release target {target}; choose x86_64/aarch64 Linux GNU or macOS"),
    };
    if !valid {
        bail!("binary format/architecture does not match {target}; rebuild for that target");
    }
    Ok(())
}
struct ArchiveInputs<'a> {
    root: &'a Path,
    target: &'a str,
    binary: &'a Path,
    notice_text: &'a str,
    notice_manifest: &'a Value,
    rust_docs: &'a Path,
    rustc: &'a str,
    receipt: &'a Receipt,
    environment: &'a BTreeMap<String, String>,
}
fn build_archive(input: ArchiveInputs<'_>, output: &Path) -> Result<(PathBuf, PathBuf)> {
    let ArchiveInputs {
        root,
        target,
        binary,
        notice_text,
        notice_manifest,
        rust_docs,
        rustc,
        receipt,
        environment,
    } = input;
    let metadata = parse(&root.join("Cargo.toml"))?;
    let package = &metadata["package"];
    let name = package
        .get("name")
        .and_then(|v| v.as_str())
        .context("package name missing")?;
    let version = package
        .get("version")
        .and_then(|v| v.as_str())
        .context("package version missing")?;
    if package.get("license").and_then(|v| v.as_str()) != Some("GPL-3.0-or-later") {
        bail!("project license must be GPL-3.0-or-later for this release flow");
    }
    if name.is_empty()
        || version.is_empty()
        || !name
            .bytes()
            .chain(version.bytes())
            .all(|c| c.is_ascii_alphanumeric() || b"-._".contains(&c))
    {
        bail!("unsupported package name/version; use portable filename characters");
    }
    let lock_hash = sha256(&read_required(&root.join("Cargo.lock"))?);
    if notice_manifest.get("target").and_then(Value::as_str) != Some(target) {
        bail!("dependency notices target mismatch; regenerate notices for {target}");
    }
    if notice_manifest
        .get("cargo_lock_sha256")
        .and_then(Value::as_str)
        != Some(&lock_hash)
    {
        bail!("dependency notices lockfile hash is stale; regenerate notices");
    }
    if notice_text.is_empty()
        || notice_manifest
            .get("notices_sha256")
            .and_then(Value::as_str)
            != Some(&sha256(notice_text.as_bytes()))
    {
        bail!("dependency notice text hash mismatch; regenerate notices");
    }
    if notice_manifest
        .get("packages")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty)
    {
        bail!("dependency notice manifest contains no packages; regenerate notices");
    }
    let executable = read_required(binary)?;
    validate_binary(&executable, target)?;
    let vendored = validate_vendor(root)?;
    let current = source_manifest(root)?;
    if &current != receipt {
        bail!("frozen source changed after build began; retry from a new snapshot");
    }
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut modes = BTreeMap::new();
    files.insert(format!("bin/{name}"), executable);
    modes.insert(format!("bin/{name}"), 0o755);
    for file in ["LICENSE", "NOTICE", "README.md"] {
        files.insert(file.into(), read_required(&root.join(file))?);
    }
    files.insert(
        "THIRD_PARTY_NOTICES.md".into(),
        notice_text.as_bytes().to_vec(),
    );
    files.insert(
        "THIRD_PARTY_NOTICES.json".into(),
        serde_json::to_vec_pretty(notice_manifest)?,
    );
    files.insert(
        "rust/COPYRIGHT-library.html".into(),
        read_required(&rust_docs.join("COPYRIGHT-library.html"))?,
    );
    for needed in ["MIT.txt", "Apache-2.0.txt"] {
        read_required(&rust_docs.join("licenses").join(needed))?;
    }
    for path in walk_files(&rust_docs.join("licenses"))? {
        files.insert(
            format!(
                "rust/licenses/{}",
                relative(&path, &rust_docs.join("licenses"))?
            ),
            read_required(&path)?,
        );
    }
    for (relative, path) in project_paths(root, true)? {
        if relative == "REBUILD.sh" {
            modes.insert(format!("source/{relative}"), 0o755);
        }
        let bytes = fs::read(path)?;
        if receipt.get(&relative) != Some(&sha256(&bytes)) {
            bail!("frozen source changed while reading {relative}; retry from a new snapshot");
        }
        files.insert(format!("source/{relative}"), bytes);
    }
    if target.contains("linux") {
        files.insert(
            "udev/70-uareu.rules".into(),
            read_required(&root.join("udev/70-uareu.rules"))?,
        );
    }
    let file_info: BTreeMap<_, _> = files
        .iter()
        .map(|(path, data)| {
            (
                path.clone(),
                json!({"sha256":sha256(data),"bytes":data.len()}),
            )
        })
        .collect();
    let info = json!({"name":name,"version":version,"target":target,"rustc":rustc.trim(),"cargo_lock_sha256":lock_hash,
        "build_command":["cargo","build","--frozen","--release","--target",target,"--bin",name],"build_environment":environment,
        "source_files":current,"source_tree_sha256":sha256(&serde_json::to_vec(&current)?),"vendored_dependencies":vendored,
        "source_delivery":"matching frozen project tree, build scripts, offline Cargo configuration and all locked registry dependency sources in source/",
        "toolchain_requirements":"use the recorded Rust compiler and target linker/OS SDK; general-purpose unmodified build tools and OS/system libraries are not bundled",
        "dependency_notice_scope":"target-filtered resolved packages, conservatively including build dependencies",
        "rust_notice_scope":"version-matched standard-library report and accompanying license texts; conservative superset",
        "files":file_info,"not_legal_or_provenance_clearance":true});
    files.insert(
        "BUILD_INFO.json".into(),
        [serde_json::to_vec_pretty(&info)?, vec![b'\n']].concat(),
    );
    let base = format!("{name}-{version}-{target}");
    fs::create_dir_all(output)?;
    let archive = output.join(format!("{base}.tar.gz"));
    let checksum = output.join(format!("{base}.tar.gz.sha256"));
    if archive.exists() || checksum.exists() {
        bail!(
            "release already exists: {}; use a new output directory",
            archive.display()
        );
    }
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&archive)?;
    let result = (|| -> Result<()> {
        let compressed = GzBuilder::new()
            .mtime(0)
            .write(file, Compression::default());
        let mut bundle = tar::Builder::new(compressed);
        for (path, data) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(*modes.get(&path).unwrap_or(&0o644));
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            header.set_cksum();
            bundle.append_data(&mut header, format!("{base}/{path}"), data.as_slice())?;
        }
        bundle.into_inner()?.finish()?.sync_all()?;
        let mut sidecar = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&checksum)?;
        if let Err(error) = writeln!(sidecar, "{}  {}.tar.gz", sha256(&fs::read(&archive)?), base)
            .and_then(|()| sidecar.sync_all())
        {
            let _ = fs::remove_file(&checksum);
            return Err(error.into());
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&archive);
    }
    result?;
    Ok((archive, checksum))
}
/// Build from the exact frozen source tree shipped in the archive.
pub fn run(root: &Path, target: &str, output_dir: &Path) -> Result<()> {
    // Reject unsupported targets before performing network or build work.
    if ![
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
    ]
    .contains(&target)
    {
        bail!("unsupported release target {target}; choose x86_64/aarch64 Linux GNU or macOS");
    }
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let rust_version = command(root, &rustc, &["-Vv"])?;
    let docs =
        PathBuf::from(command(root, &rustc, &["--print", "sysroot"])?).join("share/doc/rust");
    read_required(&docs.join("COPYRIGHT-library.html"))
        .context("Rust copyright report missing; run rustup component add rust-docs and retry")?;
    let temporary = TempDir::new("uareu-release")?;
    let snapshot = temporary.path().join("source");
    snapshot_project(root, &snapshot)?;
    // Fetch only after rejecting manifests that could resolve outside the snapshot.
    command(&snapshot, "cargo", &["fetch", "--locked"])?;
    if !snapshot.join("vendor").is_dir() {
        command(
            &snapshot,
            "cargo",
            &[
                "vendor",
                "--locked",
                "--offline",
                "--versioned-dirs",
                "vendor",
            ],
        )
        .context("locked vendoring failed; run cargo fetch --locked and retry")?;
    }
    fs::create_dir_all(snapshot.join(".cargo"))?;
    fs::write(snapshot.join(".cargo/config.toml"), VENDOR_CONFIG)?;
    validate_vendor(&snapshot)?;
    let metadata = parse(&snapshot.join("Cargo.toml"))?;
    let name = metadata["package"]["name"]
        .as_str()
        .context("missing package name")?;
    let environment: BTreeMap<String, String> = BUILD_ENV
        .iter()
        .filter_map(|key| {
            std::env::var(key)
                .ok()
                .map(|value| ((*key).to_owned(), value))
        })
        .collect();
    let mut rebuild = "#!/bin/sh\nset -eu\ncd \"$(dirname \"$0\")\"\n".to_owned();
    for (key, value) in &environment {
        rebuild.push_str(&format!(
            "export {key}='{}'\n",
            value.replace('\'', "'\\''")
        ));
    }
    rebuild.push_str(&format!(
        "cargo build --frozen --release --target {target} --bin {name}\n"
    ));
    fs::write(snapshot.join("REBUILD.sh"), rebuild)?;
    let receipt = source_manifest(&snapshot)?;
    let build = temporary.path().join("build");
    let status = Command::new("cargo")
        .args([
            "build",
            "--frozen",
            "--release",
            "--target",
            target,
            "--bin",
            name,
        ])
        .env("CARGO_TARGET_DIR", &build)
        .current_dir(&snapshot)
        .status()
        .context("cannot start cargo; install Rust and retry")?;
    if !status.success() {
        bail!(
            "frozen release build failed; fix the target linker/SDK or dependency error and retry"
        );
    }
    let (notice_text, notice_manifest) = crate::notices::generate(&snapshot, Some(target))?;
    let (archive, checksum) = build_archive(
        ArchiveInputs {
            root: &snapshot,
            target,
            binary: &build.join(target).join("release").join(name),
            notice_text: &notice_text,
            notice_manifest: &notice_manifest,
            rust_docs: &docs,
            rustc: &rust_version,
            receipt: &receipt,
            environment: &environment,
        },
        output_dir,
    )?;
    println!("{}\n{}", archive.display(), checksum.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        temp: TempDir,
        root: PathBuf,
        docs: PathBuf,
        binary: PathBuf,
        text: String,
        manifest: Value,
    }
    const TARGET: &str = "aarch64-apple-darwin";
    fn write(root: &Path, name: &str, data: impl AsRef<[u8]>) {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, data).unwrap();
    }
    fn fixture() -> Fixture {
        let temp = TempDir::new("package-test").unwrap();
        let root = temp.path().join("project");
        write(
            &root,
            "Cargo.toml",
            "[package]\nname='uareu'\nversion='0.1.0'\nlicense='GPL-3.0-or-later'\n",
        );
        write(
            &root,
            "xtask/Cargo.toml",
            "[package]\nname='xtask'\nversion='0.1.0'\n",
        );
        write(
            &root,
            "Cargo.lock",
            format!(
                "version=4\n[[package]]\nname='uareu'\nversion='0.1.0'\n[[package]]\nname='example'\nversion='1.0.0'\nsource='registry+https://github.com/rust-lang/crates.io-index'\nchecksum='{}'\n",
                "a".repeat(64)
            ),
        );
        for name in [
            "LICENSE",
            "NOTICE",
            "README.md",
            "src/main.rs",
            "xtask/src/main.rs",
            "udev/70-uareu.rules",
            "REBUILD.sh",
            "fixtures/synthetic-plain.wire",
            "licenses/supplements/example/LICENSE",
        ] {
            write(
                &root,
                name,
                format!("synthetic {name}\nhttps://example.org/source?raw=1&version=2\n"),
            );
        }
        write(&root, ".cargo/config.toml", VENDOR_CONFIG);
        let crate_root = root.join("vendor/example-1.0.0");
        write(
            &crate_root,
            "Cargo.toml",
            "[package]\nname='example'\nversion='1.0.0'\n",
        );
        write(&crate_root, "lib.rs", "// synthetic source\n");
        let inventory: BTreeMap<_, _> = walk_files(&crate_root)
            .unwrap()
            .into_iter()
            .map(|p| {
                (
                    relative(&p, &crate_root).unwrap(),
                    sha256(&fs::read(p).unwrap()),
                )
            })
            .collect();
        write(
            &crate_root,
            ".cargo-checksum.json",
            serde_json::to_vec(&json!({"package":"a".repeat(64),"files":inventory})).unwrap(),
        );
        let docs = temp.path().join("rust-docs");
        for name in [
            "COPYRIGHT-library.html",
            "licenses/MIT.txt",
            "licenses/Apache-2.0.txt",
        ] {
            write(&docs, name, format!("full Rust notice {name}"));
        }
        let binary = temp.path().join("binary");
        let mut header = vec![0; 64];
        header[..4].copy_from_slice(b"\xcf\xfa\xed\xfe");
        header[4..8].copy_from_slice(&0x0100000cu32.to_le_bytes());
        fs::write(&binary, header).unwrap();
        let text = "Copyright Example Author\nMIT license text\nhttps://example.org/source?raw=1&version=2\n".to_owned();
        let manifest = json!({"target":TARGET,"cargo_lock_sha256":sha256(&fs::read(root.join("Cargo.lock")).unwrap()),"notices_sha256":sha256(text.as_bytes()),"packages":[{"name":"example","version":"1.0.0"}]});
        Fixture {
            temp,
            root,
            docs,
            binary,
            text,
            manifest,
        }
    }
    impl Fixture {
        fn build(&self, output: &str, receipt: Option<&Receipt>) -> Result<(PathBuf, PathBuf)> {
            let current = source_manifest(&self.root)?;
            build_archive(
                ArchiveInputs {
                    root: &self.root,
                    target: TARGET,
                    binary: &self.binary,
                    notice_text: &self.text,
                    notice_manifest: &self.manifest,
                    rust_docs: &self.docs,
                    rustc: "rustc synthetic\nhost: aarch64-apple-darwin",
                    receipt: receipt.unwrap_or(&current),
                    environment: &BTreeMap::new(),
                },
                &self.temp.path().join(output),
            )
        }
    }
    fn contents(archive: &Path) -> BTreeMap<String, (u32, Vec<u8>)> {
        use std::io::Read;
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(
            fs::File::open(archive).unwrap(),
        ));
        archive
            .entries()
            .unwrap()
            .map(|entry| {
                let mut entry = entry.unwrap();
                let name = entry.path().unwrap().to_str().unwrap().to_owned();
                let mode = entry.header().mode().unwrap();
                let mut bytes = vec![];
                entry.read_to_end(&mut bytes).unwrap();
                (name, (mode, bytes))
            })
            .collect()
    }
    #[test]
    fn archive_preserves_source_notice_bytes_and_build_receipt() {
        let f = fixture();
        let (archive, checksum) = f.build("dist", None).unwrap();
        let files = contents(&archive);
        let prefix = "uareu-0.1.0-aarch64-apple-darwin/";
        for name in [
            "source/xtask/src/main.rs",
            "source/xtask/Cargo.toml",
            "source/REBUILD.sh",
            "source/vendor/example-1.0.0/lib.rs",
            "source/licenses/supplements/example/LICENSE",
            "rust/COPYRIGHT-library.html",
            "rust/licenses/MIT.txt",
            "LICENSE",
            "NOTICE",
        ] {
            assert!(files.contains_key(&format!("{prefix}{name}")), "{name}");
        }
        assert_eq!(files[&format!("{prefix}bin/uareu")].0, 0o755);
        assert_eq!(files[&format!("{prefix}source/REBUILD.sh")].0, 0o755);
        assert_eq!(
            files[&format!("{prefix}THIRD_PARTY_NOTICES.md")].1,
            f.text.as_bytes()
        );
        assert_eq!(
            files[&format!("{prefix}source/NOTICE")].1,
            fs::read(f.root.join("NOTICE")).unwrap()
        );
        let info: Value =
            serde_json::from_slice(&files[&format!("{prefix}BUILD_INFO.json")].1).unwrap();
        assert_eq!(
            info["source_files"]["src/main.rs"],
            sha256(&fs::read(f.root.join("src/main.rs")).unwrap())
        );
        assert_eq!(
            info["files"]["THIRD_PARTY_NOTICES.md"]["sha256"],
            sha256(f.text.as_bytes())
        );
        assert!(
            fs::read_to_string(checksum)
                .unwrap()
                .starts_with(&sha256(&fs::read(archive).unwrap()))
        );
    }
    #[test]
    fn reproducible_and_no_overwrite() {
        let f = fixture();
        let (a, _) = f.build("a", None).unwrap();
        let (b, _) = f.build("b", None).unwrap();
        assert_eq!(fs::read(&a).unwrap(), fs::read(b).unwrap());
        assert!(
            f.build("a", None)
                .unwrap_err()
                .to_string()
                .contains("already exists")
        );
    }
    #[test]
    fn source_drift_rejected() {
        let f = fixture();
        let receipt = source_manifest(&f.root).unwrap();
        write(&f.root, "src/main.rs", "changed");
        assert!(
            f.build("dist", Some(&receipt))
                .unwrap_err()
                .to_string()
                .contains("source changed")
        );
    }
    #[test]
    fn snapshot_allowlist_excludes_python_private_and_generated_inputs() {
        let f = fixture();
        for name in [
            ".env",
            "private.md",
            "tools/package_release.py",
            "tests/test_package_release.py",
            "captures/private.gray",
            "docs/VALIDATION.md",
            "target/debug/private",
            ".agents/private",
            ".git/config",
        ] {
            write(&f.root, name, "must not ship");
        }
        let snapshot = f.temp.path().join("snapshot");
        snapshot_project(&f.root, &snapshot).unwrap();
        for name in [
            ".env",
            "private.md",
            "tools",
            "tests/test_package_release.py",
            "captures",
            "docs/VALIDATION.md",
            "target",
            ".agents",
            ".git",
        ] {
            assert!(!snapshot.join(name).exists(), "{name}");
        }
        assert_eq!(
            fs::read(snapshot.join("NOTICE")).unwrap(),
            fs::read(f.root.join("NOTICE")).unwrap()
        );
        assert!(
            fs::read_to_string(snapshot.join(".cargo/config.toml"))
                .unwrap()
                .contains("run --locked --package xtask --")
        );
    }
    #[test]
    fn vendor_corruption_and_inventory_changes_rejected() {
        for corruption in ["changed", "missing", "extra", "package", "inventory"] {
            let f = fixture();
            let directory = f.root.join("vendor/example-1.0.0");
            match corruption {
                "changed" => write(&directory, "lib.rs", "tampered"),
                "missing" => fs::remove_file(directory.join("lib.rs")).unwrap(),
                "extra" => write(&directory, "extra.rs", "unlisted"),
                _ => {
                    let path = directory.join(".cargo-checksum.json");
                    let mut value: Value =
                        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                    if corruption == "package" {
                        value["package"] = json!("b".repeat(64));
                    } else {
                        value["files"]["absent.rs"] = json!("a".repeat(64));
                    }
                    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
                }
            }
            assert!(validate_vendor(&f.root).is_err(), "{corruption}");
        }
    }
    #[test]
    fn notice_target_lock_text_and_package_checks_fail_closed() {
        for key in ["target", "cargo_lock_sha256", "notices_sha256", "packages"] {
            let mut f = fixture();
            f.manifest[key] = if key == "packages" {
                json!([])
            } else {
                json!("stale")
            };
            assert!(f.build("dist", None).is_err(), "{key}");
        }
    }
    #[test]
    fn architecture_checks_all_supported_targets() {
        for (target, cpu) in [
            ("x86_64-unknown-linux-gnu", 62u16),
            ("aarch64-unknown-linux-gnu", 183),
        ] {
            let mut bytes = vec![0; 64];
            bytes[..6].copy_from_slice(b"\x7fELF\x02\x01");
            bytes[18..20].copy_from_slice(&cpu.to_le_bytes());
            validate_binary(&bytes, target).unwrap();
            bytes[18] ^= 1;
            assert!(validate_binary(&bytes, target).is_err());
        }
        for (target, cpu) in [("x86_64-apple-darwin", 0x01000007u32), (TARGET, 0x0100000c)] {
            let mut bytes = vec![0; 32];
            bytes[..4].copy_from_slice(b"\xcf\xfa\xed\xfe");
            bytes[4..8].copy_from_slice(&cpu.to_le_bytes());
            validate_binary(&bytes, target).unwrap();
            bytes[4] ^= 1;
            assert!(validate_binary(&bytes, target).is_err());
        }
        assert!(validate_binary(&[], "unsupported").is_err());
    }
    #[test]
    fn linux_archive_includes_matching_usb_rule() {
        let mut f = fixture();
        let target = "x86_64-unknown-linux-gnu";
        let mut bytes = vec![0; 64];
        bytes[..6].copy_from_slice(b"\x7fELF\x02\x01");
        bytes[18..20].copy_from_slice(&62u16.to_le_bytes());
        fs::write(&f.binary, bytes).unwrap();
        f.manifest["target"] = json!(target);
        let (archive, _) = build_archive(
            ArchiveInputs {
                root: &f.root,
                target,
                binary: &f.binary,
                notice_text: &f.text,
                notice_manifest: &f.manifest,
                rust_docs: &f.docs,
                rustc: "rustc synthetic",
                receipt: &source_manifest(&f.root).unwrap(),
                environment: &BTreeMap::new(),
            },
            &f.temp.path().join("linux"),
        )
        .unwrap();
        let files = contents(&archive);
        assert_eq!(
            files["uareu-0.1.0-x86_64-unknown-linux-gnu/udev/70-uareu.rules"].1,
            fs::read(f.root.join("udev/70-uareu.rules")).unwrap()
        );
    }
    #[test]
    fn nonportable_vendor_config_and_external_dependencies_rejected() {
        let f = fixture();
        write(
            &f.root,
            ".cargo/config.toml",
            "[source.vendored-sources]\ndirectory='/outside/vendor'\n",
        );
        assert!(validate_vendor(&f.root).is_err());
        for manifest in [
            "[dependencies]\nexternal={path='../external'}",
            "[target.'cfg(unix)'.dependencies]\nexternal={git='https://example.org/external'}",
            "[patch.crates-io]\nexternal={path='../external'}",
        ] {
            assert!(reject_external_dependencies(&toml::from_str(manifest).unwrap()).is_err());
        }
    }
    #[cfg(unix)]
    #[test]
    fn symlink_sources_and_vendor_files_rejected() {
        use std::os::unix::fs::symlink;
        let f = fixture();
        symlink(f.root.join("NOTICE"), f.root.join("src/link.rs")).unwrap();
        assert!(source_manifest(&f.root).is_err());
        fs::remove_file(f.root.join("src/link.rs")).unwrap();
        symlink(
            f.root.join("NOTICE"),
            f.root.join("vendor/example-1.0.0/link"),
        )
        .unwrap();
        assert!(validate_vendor(&f.root).is_err());
    }
    #[test]
    fn missing_rust_and_project_notices_rejected() {
        let f = fixture();
        fs::remove_file(f.docs.join("licenses/MIT.txt")).unwrap();
        assert!(f.build("dist", None).is_err());
        let f = fixture();
        fs::remove_file(f.root.join("NOTICE")).unwrap();
        assert!(f.build("dist", None).is_err());
    }
}
