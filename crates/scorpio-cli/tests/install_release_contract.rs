use std::{fs, path::PathBuf};

// Walk upward from this crate's manifest directory until `.github/workflows/release.yml`
// resolves next to the path. Starting with the Cargo workspace conversion, this test
// lives inside a member crate rather than at the repo root, so a literal `../../` parent
// jump is brittle — any future re-nesting silently breaks the contract. The walker form
// stays correct regardless of how deep the crate sits under the workspace.
fn repo_root() -> PathBuf {
    let start = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for ancestor in start.ancestors() {
        if ancestor.join(".github/workflows/release.yml").is_file() {
            return ancestor.to_path_buf();
        }
    }
    panic!(
        "could not locate repo root (no .github/workflows/release.yml found above {})",
        start.display()
    );
}

fn read_repo_file(path: &str) -> String {
    fs::read_to_string(repo_root().join(path))
        .unwrap_or_else(|err| panic!("failed to read {path}: {err}"))
}

#[test]
fn release_workflow_publishes_archive_only_installer_assets() {
    let workflow = read_repo_file(".github/workflows/release.yml");

    for required in [
        "publish_and_verify_release_assets",
        "scorpio-${{ matrix.target }}.tar.gz",
        "scorpio-${{ matrix.target }}.zip",
        "Upload release archive",
        "install.sh",
        "install.ps1",
        "scorpio-x86_64-unknown-linux-gnu.tar.gz",
        "scorpio-aarch64-unknown-linux-gnu.tar.gz",
        "scorpio-aarch64-apple-darwin.tar.gz",
        "scorpio-x86_64-apple-darwin.tar.gz",
        "scorpio-x86_64-pc-windows-msvc.zip",
        // SHA-256 sidecars are consumed by `scorpio upgrade` to verify downloaded
        // archives before replacing the binary. The install scripts remain
        // archive-only; sidecar verification is a CLI-upgrade-only contract.
        "scorpio-x86_64-unknown-linux-gnu.tar.gz.sha256",
        "scorpio-aarch64-unknown-linux-gnu.tar.gz.sha256",
        "scorpio-aarch64-apple-darwin.tar.gz.sha256",
        "scorpio-x86_64-apple-darwin.tar.gz.sha256",
        "scorpio-x86_64-pc-windows-msvc.zip.sha256",
    ] {
        assert!(
            workflow.contains(required),
            "release workflow missing contract fragment: {required}"
        );
    }

    for forbidden in [
        // `.sha256.sig` was the OpenSSL-signed checksum — that signing flow
        // was removed and is not being reintroduced. Unsigned `.sha256`
        // sidecars are published instead.
        ".sha256.sig",
        "Install OpenSSL (Windows)",
        "Sign checksum",
        "Upload signed release assets",
    ] {
        assert!(
            !workflow.contains(forbidden),
            "release workflow still contains obsolete signing fragment: {forbidden}"
        );
    }

    assert!(
        !repo_root()
            .join("packaging/install-signing-public.pem")
            .exists(),
        "obsolete signing public key artifact should be removed"
    );
}

// Mirrors the shell invocation at `.github/workflows/release.yml:24-42`:
//     grep -m 1 '^version' Cargo.toml | cut -d '"' -f2
// After the workspace conversion the root `Cargo.toml` no longer declares a
// package-level `version`; the inherited value lives under `[workspace.package]`.
// Dropping that line would silently break the next release (the tag match step
// would see an empty version string). This test fails loudly instead.
#[test]
fn root_cargo_toml_exposes_version_line_for_release_workflow() {
    let cargo_toml = read_repo_file("Cargo.toml");
    let version_line = cargo_toml
        .lines()
        .find(|line| line.starts_with("version"))
        .expect("root Cargo.toml must contain a top-level `version = \"...\"` line");
    let version = version_line
        .split('"')
        .nth(1)
        .expect("version line must be quoted, e.g. `version = \"0.2.5\"`");
    assert!(
        version.chars().next().is_some_and(|c| c.is_ascii_digit()),
        "parsed version `{version}` must start with a digit (e.g. 0.2.5)"
    );
}

#[test]
fn install_sh_uses_release_archive_assets() {
    let script = read_repo_file("install.sh");
    let resolved_archive = script.replace("${TARGET}", "aarch64-apple-darwin");

    for required in [
        "#!/usr/bin/env bash",
        "set -euo pipefail",
        "releases/latest",
        "scorpio-${TARGET}.tar.gz",
        "aarch64-apple-darwin",
        "INSTALL_DIR=\"$HOME/.local/bin\"",
        "Installed: $HOME/.local/bin/scorpio",
    ] {
        assert!(
            script.contains(required),
            "install.sh missing contract fragment: {required}"
        );
    }

    assert!(
        resolved_archive.contains("scorpio-aarch64-apple-darwin.tar.gz"),
        "install.sh must resolve the macOS archive-only asset name"
    );

    for forbidden in [
        "SCORPIO_INSTALL_DIR",
        ".sha256",
        ".sha256.sig",
        "openssl dgst -sha256 -verify",
        "BEGIN PUBLIC KEY",
    ] {
        assert!(
            !script.contains(forbidden),
            "install.sh still contains obsolete signing fragment: {forbidden}"
        );
    }
}

#[test]
fn install_ps1_uses_release_archive_assets() {
    let script = read_repo_file("install.ps1");

    for required in [
        "$ErrorActionPreference = \"Stop\"",
        "[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12",
        "-UseBasicParsing",
        "releases/latest",
        "OSArchitecture.ToString()",
        "Unsupported Windows architecture: $Arch",
        "scorpio-x86_64-pc-windows-msvc.zip",
        "Invoke-WebRequest -TimeoutSec 30 -Method Head -Uri $ArchiveUrl -UseBasicParsing",
        "Latest release does not include x86_64-pc-windows-msvc yet.",
        "Expected scorpio.exe missing from archive.",
        "TrimEnd('\\\\')",
    ] {
        assert!(
            script.contains(required),
            "install.ps1 missing contract fragment: {required}"
        );
    }

    for forbidden in [
        "SCORPIO_INSTALL_DIR",
        ".sha256",
        ".sha256.sig",
        "RSACryptoServiceProvider",
        "<RSAKeyValue>",
    ] {
        assert!(
            !script.contains(forbidden),
            "install.ps1 still contains obsolete signing fragment: {forbidden}"
        );
    }
}
