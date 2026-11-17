//! Derives the protocol version from the crate's source: an FNV-1a hash of
//! `Cargo.toml` and every file under `src/`, in sorted path order. Any change
//! to the protocol crate gives a new version.

use std::fs;
use std::path::{Path, PathBuf};

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn main() {
    println!("cargo::rerun-if-changed=Cargo.toml");
    println!("cargo::rerun-if-changed=src");
    let mut files = vec![PathBuf::from("Cargo.toml")];
    collect(Path::new("src"), &mut files);
    files.sort();
    let mut hash = FNV_OFFSET;
    for file in files {
        let path = file.to_str().expect("protocol source paths are UTF-8");
        let contents = fs::read(&file).expect("protocol source reads");
        for bytes in [path.as_bytes(), &[0], &contents, &[0]] {
            for byte in bytes {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(FNV_PRIME);
            }
        }
    }
    println!("cargo::rustc-env=OX_PROTOCOL_VERSION={hash:016x}");
}

/// Adds every file under `dir` to `files`.
fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("the protocol source directory reads") {
        let path = entry.expect("a directory entry reads").path();
        if path.is_dir() {
            collect(&path, files);
        } else {
            files.push(path);
        }
    }
}
