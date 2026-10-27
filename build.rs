//! Stamps the binary with the git commit it was built from, marked
//! "+modified" when the program's sources differ from that commit.
use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn main() {
    // Sources, plus the git files that change on commit and checkout.
    for path in ["src", "Cargo.toml", "build.rs", ".git/HEAD", ".git/index", ".git/refs/heads"] {
        if Path::new(path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    let commit = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let modified = git(&["status", "--porcelain", "--", "src", "Cargo.toml", "build.rs"])
        .is_some_and(|changes| !changes.is_empty());
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let suffix = if modified { "+modified" } else { "" };
    println!("cargo:rustc-env=AKLER_BUILD={version} {commit}{suffix}");
}
