use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::process::Stdio;

pub(crate) fn slugify(input: &str) -> String {
    let mut slug = String::new();
    for c in input.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if c.is_whitespace() || c == '-' || c == '_' {
            if !slug.ends_with('-') {
                slug.push('-');
            }
        }
    }

    if slug.ends_with('-') {
        slug.pop();
    }

    if slug.is_empty() {
        "note".to_string()
    } else {
        slug
    }
}

pub(crate) fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

pub fn launch_subl_if_installed(path: &PathBuf) {
    if !is_subl_available() {
        return;
    }

    let _ = std::process::Command::new("subl")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn is_subl_available() -> bool {
    std::process::Command::new("subl")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}
