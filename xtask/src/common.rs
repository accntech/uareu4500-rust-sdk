// SPDX-License-Identifier: GPL-3.0-or-later
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub fn sha256(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}
pub fn read_required(path: &Path) -> Result<Vec<u8>> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    ensure!(
        !bytes.is_empty(),
        "required file is empty: {}",
        path.display()
    );
    Ok(bytes)
}
pub fn walk_files(path: &Path) -> Result<Vec<PathBuf>> {
    fn visit(path: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "symlink not allowed: {}",
            path.display()
        );
        if metadata.is_dir() {
            for entry in fs::read_dir(path)? {
                visit(&entry?.path(), files)?;
            }
        } else {
            ensure!(metadata.is_file(), "non-regular file: {}", path.display());
            files.push(path.to_owned());
        }
        Ok(())
    }
    let mut files = Vec::new();
    visit(path, &mut files)?;
    files.sort();
    Ok(files)
}
pub struct TempDir(PathBuf);
impl TempDir {
    pub fn new(prefix: &str) -> Result<Self> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        ensure!(
            !prefix.is_empty()
                && prefix
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-'),
            "invalid temporary directory prefix"
        );
        for _ in 0..100 {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "{prefix}-{}-{stamp}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        anyhow::bail!("cannot allocate temporary directory")
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hash_matches_known_sha256() {
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
    #[test]
    fn temporary_directory_has_owned_lifetime_and_walk_rejects_links() {
        let dir = TempDir::new("xtask-test").unwrap();
        let path = dir.path().to_owned();
        std::fs::write(path.join("z"), b"value").unwrap();
        std::fs::write(path.join("a"), b"first").unwrap();
        assert_eq!(walk_files(&path).unwrap(), [path.join("a"), path.join("z")]);
        assert_eq!(read_required(&path.join("z")).unwrap(), b"value");
        std::fs::write(path.join("empty"), b"").unwrap();
        assert!(read_required(&path.join("empty")).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(path.join("a"), path.join("link")).unwrap();
            assert!(walk_files(&path).is_err());
        }
        drop(dir);
        assert!(!path.exists());
    }
}
