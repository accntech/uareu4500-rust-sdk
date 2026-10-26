// SPDX-License-Identifier: GPL-3.0-or-later
//! Private filesystem and child-process primitives shared by external adapters.
use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

pub(crate) fn read_bounded(path: &Path, maximum: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(maximum as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= maximum, "engine output limit exceeded");
    Ok(bytes)
}
pub(crate) struct Workspace(pub(crate) PathBuf);
impl Workspace {
    pub(crate) fn new() -> Result<Self> {
        let root = fs::canonicalize(std::env::temp_dir())?;
        for _ in 0..10 {
            let mut nonce = [0; 16];
            getrandom::fill(&mut nonce)?;
            let path = root.join(format!(
                "uareu-external-{:032x}",
                u128::from_le_bytes(nonce)
            ));
            #[allow(unused_mut)]
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        bail!("cannot allocate external-tool workspace")
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        if self.0.is_absolute()
            && self
                .0
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("uareu-external-"))
            && fs::canonicalize(std::env::temp_dir()).ok().as_deref() == self.0.parent()
        {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
pub(crate) fn private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

// External model parsers join paths with '/', incompatible with verbatim Windows paths.
pub(crate) fn canonical_cli_path(path: &Path) -> std::io::Result<PathBuf> {
    let canonical = path.canonicalize()?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let wide: Vec<_> = canonical.as_os_str().encode_wide().collect();
        let unc: Vec<_> = "\\\\?\\UNC\\".encode_utf16().collect();
        let verbatim: Vec<_> = "\\\\?\\".encode_utf16().collect();
        if wide.starts_with(&unc) {
            let mut normal: Vec<_> = "\\\\".encode_utf16().collect();
            normal.extend_from_slice(&wide[unc.len()..]);
            return Ok(std::ffi::OsString::from_wide(&normal).into());
        }
        if wide.starts_with(&verbatim) && wide.get(5) == Some(&(b':' as u16)) {
            return Ok(std::ffi::OsString::from_wide(&wide[verbatim.len()..]).into());
        }
    }
    Ok(canonical)
}

pub(crate) fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).context("cannot open engine/model for fingerprinting")?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 8192];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub(crate) struct Child(std::process::Child);
impl Child {
    pub(crate) fn spawn(command: &mut Command) -> std::io::Result<Self> {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        command.spawn().map(Self)
    }

    pub(crate) fn wait_checked(
        mut self,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<ExitStatus> {
        loop {
            check()?;
            if let Some(status) = self.0.try_wait()? {
                return Ok(status);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub(crate) fn run_captured(
    executable: &Path,
    args: &[&std::ffi::OsStr],
    directory: &Path,
    stage: &str,
    timeout: Duration,
) -> Result<Vec<u8>> {
    const OUTPUT_LIMIT: usize = 128 * 1024;
    let stdout = directory.join(format!("{stage}.stdout"));
    let stderr = directory.join(format!("{stage}.stderr"));
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(private_file(&stdout)?)
        .stderr(private_file(&stderr)?);
    let child = Child::spawn(&mut command).context("cannot start quality engine")?;
    let started = Instant::now();
    let status = child.wait_checked(|| {
        ensure!(
            [stdout.as_path(), stderr.as_path()]
                .iter()
                .all(|path| fs::metadata(path).is_ok_and(|m| m.len() <= OUTPUT_LIMIT as u64)),
            "quality engine output exceeded 128 KiB"
        );
        ensure!(
            started.elapsed() < timeout,
            "quality engine evaluation timed out"
        );
        Ok(())
    })?;
    ensure!(
        status.success(),
        "quality engine process failed ({status}): {}",
        String::from_utf8_lossy(&read_bounded(&stderr, OUTPUT_LIMIT)?).trim()
    );
    let output = read_bounded(&stdout, OUTPUT_LIMIT)?;
    ensure!(
        fs::metadata(&stderr)?.len() <= OUTPUT_LIMIT as u64,
        "quality engine output exceeded 128 KiB"
    );
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn workspace_files_are_exclusive_bounded_and_removed_with_the_owner() {
        let workspace = Workspace::new().unwrap();
        let directory = workspace.0.clone();
        let path = directory.join("input");
        private_file(&path).unwrap().write_all(b"abc").unwrap();
        assert!(private_file(&path).is_err());
        assert_eq!(read_bounded(&path, 3).unwrap(), b"abc");
        assert!(read_bounded(&path, 2).is_err());
        assert_eq!(
            hash_file(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        drop(workspace);
        assert!(!directory.exists());
    }
}
