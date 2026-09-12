//! Shared build-time source fingerprinting; no target paths or timestamps enter
//! the identity, so supported native and WASM profiles negotiate the same input.
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

pub fn generate(workspace_relative: &str) {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let workspace = manifest.join(workspace_relative);
    let mut hash = blake3::Hasher::new();
    field(
        &mut hash,
        "identity_format",
        b"crate-source-lock-compiler-v1",
    );
    field(&mut hash, "generator", include_bytes!("source_identity.rs"));
    println!(
        "cargo:rerun-if-changed={}",
        workspace
            .join("engine/build-support/source_identity.rs")
            .display()
    );
    for (name, path) in [
        ("crate_manifest", manifest.join("Cargo.toml")),
        ("build_script", manifest.join("build.rs")),
        ("workspace_manifest", workspace.join("Cargo.toml")),
        ("dependency_lock", workspace.join("Cargo.lock")),
    ] {
        println!("cargo:rerun-if-changed={}", path.display());
        field(
            &mut hash,
            name,
            &fs::read(path).expect("source identity input"),
        );
    }
    let source = manifest.join("src");
    println!("cargo:rerun-if-changed={}", source.display());
    let mut files = Vec::new();
    collect(&source, &mut files);
    files.sort();
    for path in files {
        let label = path
            .strip_prefix(&manifest)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        field(
            &mut hash,
            &label,
            &fs::read(&path).expect("source identity file"),
        );
    }
    println!("cargo:rerun-if-env-changed=RUSTC");
    let compiler = Command::new(env::var_os("RUSTC").unwrap())
        .arg("--version")
        .output()
        .expect("compiler identity");
    assert!(compiler.status.success(), "compiler identity unavailable");
    field(&mut hash, "compiler", &compiler.stdout);
    let value = hash.finalize();
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("source_identity.rs"),
        format!(
            "pub const SOURCE_IDENTITY: [u8; 32] = {:?};\n",
            value.as_bytes()
        ),
    )
    .unwrap();
}

fn field(hash: &mut blake3::Hasher, label: &str, bytes: &[u8]) {
    hash.update(&(label.len() as u64).to_le_bytes());
    hash.update(label.as_bytes());
    hash.update(&(bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

fn collect(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("source identity directory") {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        assert!(
            !kind.is_symlink(),
            "source identity does not follow symlinks"
        );
        if kind.is_dir() {
            collect(&entry.path(), files);
        } else if kind.is_file() {
            files.push(entry.path());
        }
    }
}
