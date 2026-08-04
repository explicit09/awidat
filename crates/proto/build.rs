use std::env;
use std::path::Path;
use std::process::Command;

use sha2::{Digest, Sha256};

const TRACKED_BUILD_SETTINGS: &[&str] = &[
    "RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "RUSTC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTC_BOOTSTRAP",
    "CARGO_BUILD_TARGET",
    "CARGO_BUILD_RUSTFLAGS",
    "CARGO_BUILD_RUSTC",
    "CARGO_BUILD_RUSTC_WRAPPER",
    "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
    "CARGO_PROFILE_RELEASE_OPT_LEVEL",
    "CARGO_PROFILE_RELEASE_DEBUG",
    "CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS",
    "CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS",
    "CARGO_PROFILE_RELEASE_LTO",
    "CARGO_PROFILE_RELEASE_PANIC",
    "CARGO_PROFILE_RELEASE_INCREMENTAL",
    "CARGO_PROFILE_RELEASE_CODEGEN_UNITS",
    "CARGO_PROFILE_RELEASE_RPATH",
    "CARGO_PROFILE_RELEASE_STRIP",
    "CARGO_PROFILE_RELEASE_SPLIT_DEBUGINFO",
    "MACOSX_DEPLOYMENT_TARGET",
    "SDKROOT",
    "CC",
    "CFLAGS",
    "AR",
    "LDFLAGS",
];

fn main() {
    let profile = env::var("OUT_DIR")
        .ok()
        .and_then(|out_dir| {
            Path::new(&out_dir)
                .ancestors()
                .nth(3)
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=MONTAGE_PROJECT_READ_CARGO_PROFILE={profile}");
    for variable in ["OPT_LEVEL", "DEBUG", "TARGET"] {
        let value = env::var(variable).unwrap_or_else(|_| "unknown".into());
        println!("cargo:rustc-env=MONTAGE_PROJECT_READ_{variable}={value}");
        println!("cargo:rerun-if-env-changed={variable}");
    }
    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let rustc_version = Command::new(&rustc)
        .arg("-Vv")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|output| output.lines().collect::<Vec<_>>().join(" | "))
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=MONTAGE_PROJECT_READ_RUSTC_VV={rustc_version}");
    let mut build_environment: Vec<_> = env::vars_os()
        .filter_map(|(key, value)| {
            let key = key.into_string().ok()?;
            relevant_build_setting(&key).then_some((key, value))
        })
        .collect();
    build_environment.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    let mut build_environment_hasher = Sha256::new();
    for (key, value) in &build_environment {
        let value = value.as_encoded_bytes();
        build_environment_hasher.update((key.len() as u64).to_le_bytes());
        build_environment_hasher.update(key.as_bytes());
        build_environment_hasher.update((value.len() as u64).to_le_bytes());
        build_environment_hasher.update(value);
        println!("cargo:rerun-if-env-changed={key}");
    }
    println!(
        "cargo:rustc-env=MONTAGE_PROJECT_READ_BUILD_ENV_SHA256={:x}",
        build_environment_hasher.finalize()
    );
    for variable in TRACKED_BUILD_SETTINGS {
        println!("cargo:rerun-if-env-changed={variable}");
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/bin/montage-project-read-perf.rs");
    println!("cargo:rerun-if-changed=src/project.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=../../Cargo.toml");
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    println!("cargo:rerun-if-changed=../../.cargo/config.toml");
}

fn relevant_build_setting(key: &str) -> bool {
    matches!(
        key,
        "RUSTFLAGS"
            | "CARGO_ENCODED_RUSTFLAGS"
            | "RUSTC"
            | "RUSTC_WRAPPER"
            | "RUSTC_WORKSPACE_WRAPPER"
            | "RUSTC_BOOTSTRAP"
            | "CARGO_BUILD_TARGET"
            | "CARGO_BUILD_RUSTFLAGS"
            | "CARGO_BUILD_RUSTC"
            | "CARGO_BUILD_RUSTC_WRAPPER"
            | "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER"
            | "MACOSX_DEPLOYMENT_TARGET"
            | "SDKROOT"
            | "CC"
            | "CFLAGS"
            | "AR"
            | "LDFLAGS"
    ) || key.starts_with("CARGO_PROFILE_")
        || (key.starts_with("CARGO_TARGET_") && key != "CARGO_TARGET_DIR")
        || key.starts_with("CC_")
        || key.starts_with("CFLAGS_")
        || key.starts_with("AR_")
        || key.starts_with("LDFLAGS_")
}
