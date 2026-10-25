// SPDX-License-Identifier: GPL-3.0-or-later
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    about = "Generate synthetic fixtures, dependency notices, and source-inclusive release packages"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Generate deterministic, non-biometric fixtures (or verify without writing).
    Fixtures {
        #[arg(long)]
        check: bool,
    },
    /// Generate audited dependency copyright and license reports.
    Notices {
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long)]
        manifest: Option<PathBuf>,
        #[arg(long)]
        check: bool,
    },
    /// Build Linux/macOS archives with matching source and offline dependencies.
    Package {
        #[arg(long)]
        target: String,
        #[arg(long, default_value = "dist")]
        output_dir: PathBuf,
    },
}
fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root");
    match Cli::parse().command {
        Command::Fixtures { check } => xtask::fixtures::run(root, check),
        Command::Notices {
            target,
            output,
            manifest,
            check,
        } => xtask::notices::run(
            root,
            target.as_deref(),
            output.as_deref(),
            manifest.as_deref(),
            check,
        ),
        Command::Package { target, output_dir } => xtask::package::run(root, &target, &output_dir),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cli_requires_explicit_package_target_and_accepts_check_options() {
        assert!(Cli::try_parse_from(["xtask", "package"]).is_err());
        assert!(Cli::try_parse_from(["xtask", "fixtures", "--check"]).is_ok());
        assert!(
            Cli::try_parse_from([
                "xtask",
                "notices",
                "--target",
                "aarch64-apple-darwin",
                "--check"
            ])
            .is_ok()
        );
        let Cli {
            command: Command::Package { output_dir, .. },
        } = Cli::try_parse_from(["xtask", "package", "--target", "aarch64-apple-darwin"]).unwrap()
        else {
            panic!("wrong command")
        };
        assert_eq!(output_dir, Path::new("dist"));
    }
}
