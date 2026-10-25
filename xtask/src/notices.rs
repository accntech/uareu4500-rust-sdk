// SPDX-License-Identifier: GPL-3.0-or-later
use crate::common::{read_required, sha256, walk_files};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

fn policy() -> Result<Value> {
    serde_json::from_str(crate::policy::REVIEWED_POLICY)
        .context("embedded license policy cannot be parsed; restore the reviewed policy")
}

const PREFERENCE: &[&str] = &[
    "MIT",
    "0BSD",
    "BSD-2-Clause",
    "Zlib",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "Unicode-3.0",
    "LGPL-2.1-or-later",
    "Unlicense",
];
fn allowed(identifier: &str) -> bool {
    PREFERENCE[..7].contains(&identifier)
}
fn license_choices(expression: &str) -> Result<Vec<Vec<String>>> {
    struct Parser {
        tokens: Vec<String>,
        position: usize,
    }
    impl Parser {
        fn eat(&mut self, token: &str) -> bool {
            if self
                .tokens
                .get(self.position)
                .is_some_and(|value| value == token)
            {
                self.position += 1;
                true
            } else {
                false
            }
        }
        fn atom(&mut self) -> Result<Vec<Vec<String>>> {
            if self.eat("(") {
                let result = self.alternative()?;
                ensure!(
                    self.eat(")"),
                    "unbalanced license expression; review the crate declaration"
                );
                return Ok(result);
            }
            let mut token = self
                .tokens
                .get(self.position)
                .context("incomplete license expression; review the crate declaration")?
                .clone();
            self.position += 1;
            if self.eat("WITH") {
                let exception = self
                    .tokens
                    .get(self.position)
                    .context("incomplete license exception; review the crate declaration")?;
                token.push_str(" WITH ");
                token.push_str(exception);
                self.position += 1;
            }
            ensure!(
                PREFERENCE.contains(&token.as_str()),
                "unreviewed license {token:?}; review its terms and extend the explicit policy before packaging"
            );
            Ok(vec![vec![token]])
        }
        fn conjunction(&mut self) -> Result<Vec<Vec<String>>> {
            let mut result = self.atom()?;
            while self.eat("AND") {
                let right = self.atom()?;
                result = result
                    .into_iter()
                    .flat_map(|left| {
                        right.iter().map(move |r| {
                            let mut v = left.clone();
                            v.extend(r.clone());
                            v
                        })
                    })
                    .collect();
            }
            Ok(result)
        }
        fn alternative(&mut self) -> Result<Vec<Vec<String>>> {
            let mut result = self.conjunction()?;
            while self.eat("OR") {
                result.extend(self.conjunction()?);
            }
            Ok(result)
        }
    }
    let expression = expression
        .replace("MIT/Apache-2.0", "MIT OR Apache-2.0")
        .replace('(', " ( ")
        .replace(')', " ) ");
    let mut parser = Parser {
        tokens: expression.split_whitespace().map(str::to_owned).collect(),
        position: 0,
    };
    let mut choices = parser.alternative()?;
    ensure!(
        parser.position == parser.tokens.len(),
        "unrecognized license syntax; review the crate declaration"
    );
    choices.sort_by_key(|choice| {
        (
            choice
                .iter()
                .map(|s| PREFERENCE.iter().position(|p| p == s).unwrap_or(100))
                .sum::<usize>(),
            choice.clone(),
        )
    });
    Ok(choices)
}

fn normalized_terms(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .trim_end_matches('.')
        .to_owned()
}

/// Read only relative, non-symlink files inside the reviewed source directory.
fn pinned_path(root: &Path, relative: &str) -> Result<PathBuf> {
    let relative_path = Path::new(relative);
    ensure!(
        !relative_path.as_os_str().is_empty()
            && relative_path
                .components()
                .all(|part| matches!(part, Component::Normal(_) | Component::CurDir)),
        "license path {relative:?} is outside its package; restore a relative reviewed source"
    );
    let mut path = root.to_owned();
    for component in relative_path.components() {
        path.push(component);
        let metadata = std::fs::symlink_metadata(&path).with_context(|| {
            format!("license source {relative} is missing; restore reviewed license sources")
        })?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "license source {relative} is a symlink; restore a regular reviewed source file"
        );
    }
    ensure!(
        path.is_file(),
        "license source {relative} is not a regular file; restore reviewed license sources"
    );
    Ok(path)
}
fn pinned_text(root: &Path, item: &Value) -> Result<String> {
    let relative = field(item, "path")?;
    let raw = read_required(&pinned_path(root, relative)?)?;
    ensure!(
        sha256(&raw) == field(item, "sha256")?,
        "pinned license source {relative} changed; review before updating its recorded hash"
    );
    String::from_utf8(raw).with_context(|| {
        format!("license source {relative} is not UTF-8; review its encoding explicitly")
    })
}
fn field<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    value.get(name).and_then(Value::as_str).with_context(|| format!("missing {name} in license input; regenerate complete Cargo metadata or restore reviewed policy"))
}
fn array<'a>(value: &'a Value, name: &str) -> Result<&'a Vec<Value>> {
    value.get(name).and_then(Value::as_array).with_context(|| {
        format!(
            "missing {name} array; regenerate complete Cargo metadata or restore reviewed policy"
        )
    })
}
fn canonical_terms(policy: &Value, root: &Path) -> Result<BTreeMap<String, String>> {
    let mut forms = BTreeMap::new();
    for entry in array(policy, "canonical_terms")? {
        let identifier = field(entry, "identifier")?;
        let start = match identifier {
            "MIT" => "Permission is hereby granted",
            "Apache-2.0" => "Apache License",
            "Unicode-3.0" => "NOTICE TO USER:",
            "Zlib" => "This software is provided 'as-is'",
            "0BSD" => "Permission to use",
            "BSD-2-Clause" => "Redistribution and use",
            "LLVM-exception" => "As an exception,",
            _ => bail!("unreviewed canonical license {identifier}; review its complete terms"),
        };
        let text = pinned_text(root, entry)?;
        let begin = text.find(start).with_context(|| {
            format!("canonical {identifier} has no complete terms; restore the pinned supplement")
        })?;
        let mut terms = &text[begin..];
        if identifier == "Apache-2.0" {
            let marker = "END OF TERMS AND CONDITIONS";
            let end = terms
                .find(marker)
                .context("canonical Apache terms are incomplete; restore the pinned supplement")?;
            terms = &terms[..end + marker.len()];
        }
        forms.insert(identifier.to_owned(), normalized_terms(terms));
    }
    Ok(forms)
}
fn text_covers(identifier: &str, text: &str, forms: &BTreeMap<String, String>) -> bool {
    if !allowed(identifier) {
        return false;
    }
    let required = if identifier == "Apache-2.0 WITH LLVM-exception" {
        vec!["Apache-2.0", "LLVM-exception"]
    } else {
        vec![identifier]
    };
    let normalized = normalized_terms(text);
    required.iter().all(|key| {
        forms
            .get(*key)
            .is_some_and(|terms| !terms.is_empty() && normalized.contains(terms))
    })
}
fn source_attributions(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut included = BTreeSet::new();
    for (index, line) in lines.iter().enumerate() {
        if !line.to_lowercase().contains("copyright") && !line.contains('©') {
            continue;
        }
        included.insert(index);
        for (following, continuation) in lines.iter().enumerate().skip(index + 1).take(24) {
            let continuation = continuation.trim_start();
            if continuation.is_empty()
                || !["//", "*", "#", "<!--"]
                    .iter()
                    .any(|prefix| continuation.starts_with(prefix))
            {
                break;
            }
            included.insert(following);
        }
    }
    included
        .iter()
        .map(|i| format!("{}\n", lines[*i]))
        .collect()
}
fn reachable(metadata: &Value) -> Result<BTreeSet<String>> {
    let resolve = metadata.get("resolve").context(
        "Cargo metadata has no resolved graph; run cargo metadata --locked without --no-deps",
    )?;
    let nodes: BTreeMap<&str, &Value> = array(resolve, "nodes")?
        .iter()
        .map(|node| Ok((field(node, "id")?, node)))
        .collect::<Result<_>>()?;
    let members = array(metadata, "workspace_members")?;
    let workspace: BTreeSet<String> = members
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .context("invalid workspace member; regenerate Cargo metadata")
        })
        .collect::<Result<_>>()?;
    let mut queue: Vec<String> = workspace.iter().cloned().collect();
    if queue.is_empty() {
        queue.push(field(resolve, "root")?.to_owned());
    }
    ensure!(
        !nodes.is_empty(),
        "Cargo metadata has no complete resolved graph; run cargo metadata --locked without --no-deps"
    );
    let mut visited = BTreeSet::new();
    while let Some(id) = queue.pop() {
        if !visited.insert(id.clone()) {
            continue;
        }
        let node = nodes.get(id.as_str()).context("Cargo dependency graph is incomplete; run cargo fetch --locked and regenerate metadata")?;
        if let Some(deps) = node.get("deps").and_then(Value::as_array) {
            for dep in deps {
                queue.push(field(dep, "pkg")?.to_owned());
            }
        }
    }
    Ok(visited.difference(&workspace).cloned().collect())
}

fn generate_bundle(
    metadata: &Value,
    lock_text: &str,
    reviewed: &Value,
    root: &Path,
    target: Option<&str>,
) -> Result<(String, Value)> {
    let forms = canonical_terms(reviewed, root)?;
    generate_with_forms(metadata, lock_text, reviewed, root, target, &forms)
}
fn generate_with_forms(
    metadata: &Value,
    lock_text: &str,
    reviewed: &Value,
    root: &Path,
    target: Option<&str>,
    forms: &BTreeMap<String, String>,
) -> Result<(String, Value)> {
    ensure!(
        reviewed["schema_version"] == 1,
        "unsupported license policy schema; restore schema_version 1"
    );
    let lock: toml::Value = toml::from_str(lock_text)
        .context("Cargo.lock cannot be parsed; regenerate the lockfile with Cargo")?;
    let locked = lock
        .get("package")
        .and_then(toml::Value::as_array)
        .context("Cargo.lock has no package entries; regenerate the lockfile with Cargo")?;
    let packages_by_id: BTreeMap<&str, &Value> = array(metadata, "packages")?
        .iter()
        .map(|p| Ok((field(p, "id")?, p)))
        .collect::<Result<_>>()?;
    let selected = reachable(metadata)?;
    let mut sources: Vec<&Value> = selected
        .iter()
        .map(|id| {
            packages_by_id.get(id.as_str()).copied().with_context(|| {
                format!("resolved package {id} is absent; regenerate complete Cargo metadata")
            })
        })
        .collect::<Result<_>>()?;
    sources.sort_by_key(|p| {
        (
            p["name"].as_str().unwrap_or(""),
            p["version"].as_str().unwrap_or(""),
            p["source"].as_str().unwrap_or(""),
        )
    });
    let mut packages = Vec::new();
    let mut sections = vec![
        "# Third-party license and copyright notices\n".to_owned(),
        "Generated from Cargo.lock and the resolved dependency sources. Full license texts and supplemental notices are reproduced below.\n".to_owned(),
        target.map(|t| format!("Target: `{t}`.\n")).unwrap_or_else(|| "Target: all platforms represented by the resolved Cargo graph.\n".to_owned()),
        "Coverage is a conservative dependency-graph superset, including build dependencies, procedural macros, and development dependencies where resolved. This is not a claim that every listed crate is embedded in the executable. Rust standard-library/runtime notices are packaged separately.\n".to_owned(),
        "The selected license is shown for each crate. Other license alternatives and nested notices are also preserved where present; reproducing an alternative is not an election to use it. Additional component notices remain applicable.\n".to_owned(),
    ];
    for package in sources {
        let name = field(package, "name")?;
        let version = field(package, "version")?;
        let source = package["source"].as_str();
        let matches: Vec<&toml::Value> = locked
            .iter()
            .filter(|p| {
                p.get("name").and_then(toml::Value::as_str) == Some(name)
                    && p.get("version").and_then(toml::Value::as_str) == Some(version)
                    && p.get("source").and_then(toml::Value::as_str) == source
            })
            .collect();
        ensure!(
            matches.len() == 1,
            "{name} {version} does not uniquely match Cargo.lock; regenerate with --locked"
        );
        let checksum = matches[0]
            .get("checksum")
            .and_then(toml::Value::as_str)
            .unwrap_or("");
        ensure!(
            source.is_some_and(|s| s.starts_with("registry+"))
                && checksum.len() == 64
                && checksum
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "{name} has an unreviewed non-registry source/checksum; add explicit audited support before packaging"
        );
        let expression = field(package, "license").with_context(|| format!("{name} has no license declaration; review and add explicit source policy before packaging"))?;
        let choices = license_choices(expression)?;
        let directory = Path::new(field(package, "manifest_path")?)
            .parent()
            .context("crate manifest has no directory; regenerate Cargo metadata")?;
        ensure!(
            directory.is_dir(),
            "sources missing for {name} {version}; run cargo fetch --locked"
        );
        let explicit = package["license_file"]
            .as_str()
            .map(|path| pinned_path(directory, path))
            .transpose()?;
        let mut content: Vec<(Value, String)> = Vec::new();
        for path in walk_files(directory)? {
            let relative = path
                .strip_prefix(directory)
                .context("crate source is outside package; review its notice provenance")?;
            if relative
                .components()
                .any(|p| matches!(p.as_os_str().to_str(), Some(".git" | ".hg" | "target")))
            {
                continue;
            }
            let filename = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            let notice = [
                "license",
                "licence",
                "copying",
                "notice",
                "copyright",
                "authors",
            ]
            .iter()
            .any(|prefix| filename.starts_with(prefix))
                || explicit.as_ref().is_some_and(|p| p == &path);
            let suffix = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            if !notice
                && ![
                    "rs", "c", "h", "cc", "cpp", "m", "mm", "s", "md", "txt", "toml", "py", "html",
                ]
                .contains(&suffix.as_str())
            {
                continue;
            }
            let raw = std::fs::read(&path).with_context(|| {
                format!(
                    "cannot read crate source {}; restore Cargo sources with cargo fetch --locked",
                    relative.display()
                )
            })?;
            let original = match std::str::from_utf8(&raw) {
                Ok(text) => text,
                Err(_) if !notice => continue,
                Err(_) => {
                    bail!("{name} notice {filename} is not UTF-8; review its encoding explicitly")
                }
            };
            let text = if notice {
                original.to_owned()
            } else {
                source_attributions(original)
            };
            if text.trim().is_empty() {
                continue;
            }
            let relative = relative
                .to_str()
                .context("crate source path is not UTF-8; review its notice provenance")?
                .replace('\\', "/");
            let doc = json!({"path":relative,"kind":if notice {"notice"} else {"copyright-excerpts"},"sha256":sha256(&raw),"text_sha256":sha256(text.as_bytes()),"origin":"crate"});
            content.push((doc, text));
        }
        for supplement in array(reviewed, "supplements")? {
            if supplement["name"].as_str() != Some(name)
                || supplement["version"].as_str() != Some(version)
                || supplement["source"].as_str() != source
            {
                continue;
            }
            for item in array(supplement, "files")? {
                let text = pinned_text(root, item)?;
                let digest = sha256(text.as_bytes());
                let doc = json!({"path":field(item,"path")?,"kind":"notice","sha256":digest,"text_sha256":digest,"origin":"pinned-supplement","url":field(item,"url")?,"vcs_commit":supplement["vcs_commit"],"path_in_vcs":supplement["path_in_vcs"]});
                content.push((doc, text));
            }
        }
        let choice = choices.iter().find(|choice| {
            choice.iter().all(|item| {
                allowed(item)
                    && content
                        .iter()
                        .any(|(_, text)| text_covers(item, text, forms))
            })
        });
        let choice = match choice {
            Some(choice) => choice,
            None if !choices
                .iter()
                .any(|choice| choice.iter().all(|item| allowed(item))) =>
            {
                bail!(
                    "{name} {version} needs manual distribution review: the notice-only packager does not support copyleft source/relink obligations"
                )
            }
            None => bail!(
                "{name} {version} lacks full text for a permitted license choice; restore permissive crate notices or add an exact-version pinned supplement before manual distribution review"
            ),
        };
        let selected_license = choice.join(" AND ");
        let documents: Vec<Value> = content.iter().map(|(doc, _)| doc.clone()).collect();
        let authors = package.get("authors").cloned().unwrap_or(json!([]));
        let row = json!({"name":name,"version":version,"source":source,"checksum":checksum,"license_expression":expression,"selected_license":selected_license,"authors":authors,"repository":package["repository"],"documents":documents});
        sections.push(format!("## {name} {version}\n"));
        sections.push(format!("- Declared: `{expression}`\n- Selected: `{selected_license}`\n- Cargo.lock checksum: `{checksum}`\n"));
        if let Some(repository) = package["repository"].as_str() {
            sections.push(format!("- Upstream repository: {repository}\n"));
        }
        if let Some(authors) = authors.as_array().filter(|a| !a.is_empty()) {
            let names = authors
                .iter()
                .map(|a| {
                    a.as_str()
                        .context("invalid author attribution; regenerate Cargo metadata")
                })
                .collect::<Result<Vec<_>>>()?;
            sections.push(format!(
                "- Upstream Cargo author attribution (not an ownership determination): {}\n",
                names.join("; ")
            ));
        }
        for (document, text) in content {
            sections.push(format!(
                "### {} ({})\n",
                field(&document, "path")?,
                field(&document, "kind")?
            ));
            if let Some(url) = document["url"].as_str() {
                sections.push(format!("Pinned source: {url}\n"));
            }
            let longest = text.split(|ch| ch != '`').map(str::len).max().unwrap_or(0);
            let fence = "`".repeat(4.max(longest + 1));
            sections.push(format!(
                "{fence}text\n{text}{}{fence}\n",
                if text.ends_with('\n') { "" } else { "\n" }
            ));
        }
        packages.push(row);
    }
    let markdown = sections.join("\n");
    let manifest = json!({"schema_version":1,"target":target,"cargo_lock_sha256":sha256(lock_text.as_bytes()),"policy_sha256":sha256(serde_json::to_string(reviewed)?.as_bytes()),"coverage":"conservative resolved graph including build/proc-macro/dev dependencies where present; excludes workspace members and Rust sysroot","packages":packages,"notices_sha256":sha256(markdown.as_bytes())});
    Ok((markdown, manifest))
}

pub fn generate(root: &Path, target: Option<&str>) -> Result<(String, Value)> {
    let mut command = Command::new("cargo");
    command
        .args(["metadata", "--locked", "--offline", "--format-version", "1"])
        .current_dir(root);
    if let Some(target) = target {
        command.args(["--filter-platform", target]);
    }
    let output = command.output().context(
        "Cargo metadata could not start; install Cargo, run cargo fetch --locked, then retry",
    )?;
    ensure!(
        output.status.success(),
        "Cargo metadata failed; run cargo fetch --locked, then retry.\n{}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let metadata: Value = serde_json::from_slice(&output.stdout)
        .context("Cargo metadata was not valid JSON; verify your Cargo installation and retry")?;
    let lock = String::from_utf8(read_required(&root.join("Cargo.lock"))?)
        .context("Cargo.lock is not UTF-8; regenerate it with Cargo")?;
    generate_bundle(&metadata, &lock, &policy()?, root, target)
}

pub fn write_or_check(
    markdown: &str,
    manifest: &Value,
    output: &Path,
    manifest_path: &Path,
    check: bool,
) -> Result<()> {
    ensure!(
        output != manifest_path,
        "notice and manifest destinations are identical; choose separate --output and --manifest paths"
    );
    let manifest_text = format!("{}\n", serde_json::to_string_pretty(manifest)?);
    let outputs = [(output, markdown), (manifest_path, manifest_text.as_str())];
    if check {
        for (path, text) in outputs {
            ensure!(
                std::fs::read(path).is_ok_and(|raw| raw == text.as_bytes()),
                "{} is stale or missing; run cargo xtask notices without --check and review the changes",
                path.display()
            );
        }
    } else {
        for (path, text) in outputs {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!(
                        "cannot create {}; choose a writable notice destination",
                        parent.display()
                    )
                })?;
            }
            std::fs::write(path, text).with_context(|| {
                format!(
                    "cannot write {}; choose a writable notice destination",
                    path.display()
                )
            })?;
        }
    }
    Ok(())
}
pub fn run(
    root: &Path,
    target: Option<&str>,
    output: Option<&Path>,
    manifest: Option<&Path>,
    check: bool,
) -> Result<()> {
    let (markdown, provenance) = generate(root, target)?;
    let default_output = root.join("target/license-notices/THIRD_PARTY_NOTICES.md");
    let default_manifest = root.join("target/license-notices/THIRD_PARTY_NOTICES.json");
    write_or_check(
        &markdown,
        &provenance,
        output.unwrap_or(&default_output),
        manifest.unwrap_or(&default_manifest),
        check,
    )?;
    println!(
        "{} notices for {} locked dependencies",
        if check { "verified" } else { "generated" },
        provenance["packages"].as_array().map_or(0, Vec::len)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::TempDir;
    fn project_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_owned()
    }
    fn fixture() -> (TempDir, Value, String) {
        let temp = TempDir::new("notices-test").unwrap();
        let dir = temp.path().join("vendor/example-1.2.3");
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname=\"example\"\nversion=\"1.2.3\"\n",
        )
        .unwrap();
        let mit =
            std::fs::read_to_string(project_root().join("licenses/supplements/MIT-standard.txt"))
                .unwrap();
        std::fs::write(dir.join("LICENSE"), mit).unwrap();
        let package = json!({"id":"example-id","name":"example","version":"1.2.3","source":"registry+https://example.invalid/index","license":"MIT","license_file":null,"manifest_path":dir.join("Cargo.toml"),"authors":["Example Authors"],"repository":"https://example.invalid/example"});
        let meta = json!({"packages":[package],"workspace_members":[],"resolve":{"root":"example-id","nodes":[{"id":"example-id","deps":[]}]}});
        let lock = format!(
            "version=4\n[[package]]\nname=\"example\"\nversion=\"1.2.3\"\nsource=\"registry+https://example.invalid/index\"\nchecksum=\"{}\"\n",
            "a".repeat(64)
        );
        (temp, meta, lock)
    }
    fn bundle(meta: &Value, lock: &str) -> Result<(String, Value)> {
        let root = project_root();
        generate_bundle(meta, lock, &policy()?, &root, None)
    }
    #[test]
    fn parses_conjunction_without_dropping_unicode() {
        assert_eq!(
            license_choices("(MIT OR Apache-2.0) AND Unicode-3.0").unwrap()[0],
            vec!["MIT", "Unicode-3.0"]
        );
        assert!(license_choices("MIT OR LicenseRef-Unknown").is_err());
        for expression in ["MIT AND", "(MIT", "MIT Apache-2.0", "MIT WITH", "MIT)"] {
            assert!(license_choices(expression).is_err(), "{expression}");
        }
    }
    #[test]
    fn recognizes_unlicense_alternative_but_selects_complete_mit() {
        let (_temp, mut meta, lock) = fixture();
        meta["packages"][0]["license"] = json!("Unlicense OR MIT");
        assert_eq!(
            bundle(&meta, &lock).unwrap().1["packages"][0]["selected_license"],
            "MIT"
        );
        meta["packages"][0]["license"] = json!("Unlicense");
        assert!(bundle(&meta, &lock).is_err());
    }
    #[test]
    fn preserves_nested_notices_source_attribution_and_vendor_identity() {
        let (temp, meta, lock) = fixture();
        let dir = temp.path().join("vendor/example-1.2.3");
        std::fs::write(
            dir.join("nested/NOTICE"),
            "Additional component attribution\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("lib.rs"),
            "// Copyright 2025 Original Holder\n// and Another Holder\npub fn example() {}\n",
        )
        .unwrap();
        let first = bundle(&meta, &lock).unwrap();
        assert!(first.0.contains("Additional component attribution"));
        assert!(first.0.contains("// and Another Holder"));
        assert!(!first.0.contains(&temp.path().display().to_string()));
        assert_eq!(first.1["packages"][0]["checksum"], "a".repeat(64));
        assert_eq!(first.1["notices_sha256"], sha256(first.0.as_bytes()));
        assert_eq!(first, bundle(&meta, &lock).unwrap());
        std::fs::write(dir.join("nested/NOTICE"), "Changed component attribution\n").unwrap();
        assert_ne!(first.1, bundle(&meta, &lock).unwrap().1);
    }
    #[test]
    fn fails_closed_for_incomplete_terms_unknown_copyleft_and_lock_drift() {
        let (temp, mut meta, lock) = fixture();
        let dir = temp.path().join("vendor/example-1.2.3");
        meta["packages"][0]["version"] = json!("9.9.9");
        assert!(bundle(&meta, &lock).is_err());
        meta["packages"][0]["version"] = json!("1.2.3");
        for expression in [
            "LicenseRef-Unknown",
            "LGPL-2.1-or-later",
            "(MIT OR Apache-2.0) AND Unicode-3.0",
        ] {
            meta["packages"][0]["license"] = json!(expression);
            assert!(bundle(&meta, &lock).is_err(), "{expression}");
        }
        for (license, text) in [
            (
                "MIT",
                "Permission is hereby granted. Copyright notice. Software is provided.",
            ),
            (
                "Apache-2.0",
                "Apache License Version 2.0. Grant of Patent License.",
            ),
        ] {
            meta["packages"][0]["license"] = json!(license);
            std::fs::write(dir.join("LICENSE"), text).unwrap();
            assert!(bundle(&meta, &lock).is_err());
        }
    }
    #[test]
    fn includes_required_unicode_terms_and_ignores_empty_source_files() {
        let (temp, mut meta, lock) = fixture();
        meta["packages"][0]["license"] = json!("(MIT OR Apache-2.0) AND Unicode-3.0");
        let directory = temp.path().join("vendor/example-1.2.3");
        std::fs::write(directory.join("empty.rs"), "").unwrap();
        let unicode =
            std::fs::read(project_root().join("licenses/supplements/Unicode-3.0-standard.txt"))
                .unwrap();
        std::fs::write(directory.join("LICENSE-UNICODE"), unicode).unwrap();
        let (markdown, manifest) = bundle(&meta, &lock).unwrap();
        assert_eq!(
            manifest["packages"][0]["selected_license"],
            "MIT AND Unicode-3.0"
        );
        assert!(markdown.contains("NOTICE TO USER:"));
        assert!(
            !manifest
                .to_string()
                .contains(&temp.path().display().to_string())
        );
    }
    #[test]
    fn traverses_every_workspace_member_and_excludes_unreachable_packages() {
        let (_temp, mut meta, lock) = fixture();
        meta["workspace_members"] = json!(["root-one", "root-two"]);
        meta["resolve"]["nodes"] = json!([{"id":"root-one","deps":[]},{"id":"root-two","deps":[{"pkg":"example-id"}]},{"id":"example-id","deps":[]},{"id":"unreachable","deps":[]}]);
        let mut unknown = meta["packages"][0].clone();
        unknown["id"] = json!("unreachable");
        unknown["license"] = json!("LicenseRef-Unknown");
        meta["packages"].as_array_mut().unwrap().push(unknown);
        assert_eq!(
            bundle(&meta, &lock).unwrap().1["packages"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        meta["resolve"]["nodes"][1]["deps"][0]["pkg"] = json!("missing");
        assert!(bundle(&meta, &lock).is_err());
    }
    #[test]
    fn supplements_are_exact_identity_and_hash_pinned() {
        let (temp, meta, lock) = fixture();
        let mit =
            std::fs::read(project_root().join("licenses/supplements/MIT-standard.txt")).unwrap();
        std::fs::remove_file(temp.path().join("vendor/example-1.2.3/LICENSE")).unwrap();
        std::fs::write(temp.path().join("supplement.txt"), &mit).unwrap();
        let mut reviewed = policy().unwrap();
        reviewed["supplements"] = json!([{"name":"example","version":"1.2.3","source":"registry+https://example.invalid/index","files":[{"path":"supplement.txt","url":"https://example.invalid/commit/LICENSE","sha256":sha256(&mit)}]}]);
        // Canonical forms stay rooted in the project; custom supplements use a separate fixture root.
        let forms = canonical_terms(&policy().unwrap(), &project_root()).unwrap();
        let result =
            generate_with_forms(&meta, &lock, &reviewed, temp.path(), None, &forms).unwrap();
        assert!(result.0.contains("https://example.invalid/commit/LICENSE"));
        std::fs::write(temp.path().join("supplement.txt"), "tampered").unwrap();
        assert!(generate_with_forms(&meta, &lock, &reviewed, temp.path(), None, &forms).is_err());
        reviewed["supplements"][0]["version"] = json!("1.2.4");
        assert!(generate_with_forms(&meta, &lock, &reviewed, temp.path(), None, &forms).is_err());
    }
    #[test]
    fn canonical_terms_are_hash_verified_and_check_never_writes() {
        let temp = TempDir::new("notice-check").unwrap();
        let mut reviewed = policy().unwrap();
        reviewed["canonical_terms"][0]["sha256"] = json!("0".repeat(64));
        assert!(canonical_terms(&reviewed, &project_root()).is_err());
        let out = temp.path().join("notices.md");
        let manifest = temp.path().join("notices.json");
        std::fs::write(&out, "stale").unwrap();
        assert!(write_or_check("fresh", &json!({}), &out, &manifest, true).is_err());
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "stale");
        assert!(!manifest.exists());
        write_or_check("fresh", &json!({}), &out, &manifest, false).unwrap();
        write_or_check("fresh", &json!({}), &out, &manifest, true).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn rejects_links_and_escaping_explicit_license_paths() {
        let (temp, mut meta, lock) = fixture();
        meta["packages"][0]["license_file"] = json!("../../outside");
        assert!(bundle(&meta, &lock).is_err());
        meta["packages"][0]["license_file"] = Value::Null;
        std::os::unix::fs::symlink("LICENSE", temp.path().join("vendor/example-1.2.3/COPYING"))
            .unwrap();
        assert!(bundle(&meta, &lock).is_err());
    }
}
