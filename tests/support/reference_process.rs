// SPDX-License-Identifier: GPL-3.0-or-later
use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub struct Workspace(pub PathBuf);
impl Workspace {
    pub fn new() -> Result<Self> {
        let mut id = [0u8; 16];
        getrandom::fill(&mut id)?;
        let path =
            std::env::temp_dir().join(format!("uareu-reference-{:032x}", u128::from_le_bytes(id)));
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(&path)?;
        }
        #[cfg(not(unix))]
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut b = Vec::new();
    fs::File::open(path)?.take(limit + 1).read_to_end(&mut b)?;
    ensure!(
        b.len() as u64 <= limit,
        "external reference output exceeded limit"
    );
    Ok(b)
}
pub fn run(executable: &Path, args: &[OsString], cwd: &Path, timeout: Duration) -> Result<Vec<u8>> {
    ensure!(
        timeout > Duration::ZERO && timeout <= Duration::from_secs(300),
        "invalid reference deadline"
    );
    let log = Workspace::new()?;
    let stdout = log.0.join("stdout");
    let stderr = log.0.join("stderr");
    let file = |p: &Path| -> Result<fs::File> {
        let mut o = OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        Ok(o.open(p)?)
    };
    let mut cmd =
        Command::new(fs::canonicalize(executable).context("reference executable not found")?);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(file(&stdout)?)
        .stderr(file(&stderr)?);
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = Child(cmd.spawn()?);
    let start = Instant::now();
    loop {
        ensure!(start.elapsed() < timeout, "reference process timed out");
        ensure!(
            fs::metadata(&stdout)?.len() <= 4 * 1024 * 1024
                && fs::metadata(&stderr)?.len() <= 65536,
            "reference output exceeded limit"
        );
        if let Some(status) = child.0.try_wait()? {
            ensure!(
                status.success(),
                "reference process failed ({status}); stderr hash only: {:x}",
                <sha2::Sha256 as sha2::Digest>::digest(bounded(&stderr, 65536)?)
            );
            return bounded(&stdout, 4 * 1024 * 1024);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
