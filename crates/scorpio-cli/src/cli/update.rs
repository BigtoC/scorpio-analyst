//! Release check and self-upgrade.
//!
//! Every `scorpio` invocation spawns a background task that calls
//! [`check_latest_version`]; if it returns `Some(v)` once the subcommand
//! finishes, [`format_update_notice`] is rendered to stderr. The `scorpio
//! upgrade` subcommand calls [`run_upgrade`] directly.
//!
//! All blocking work (GitHub API, sidecar + archive downloads, in-place binary
//! replacement) is bridged into async via `tokio::task::spawn_blocking`. The
//! background check is total — every error path converts to `None` so a panic
//! inside `spawn_blocking` cannot leak to the default tokio panic hook and
//! pollute stderr around the user-facing output.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, anyhow};
use colored::Colorize;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::task;

// ── compile-time constants ─────────────────────────────────────────────

/// GitHub repository owner for the release feed.
const REPO_OWNER: &str = "BigtoC";

/// GitHub repository name for the release feed.
const REPO_NAME: &str = "scorpio-analyst";

/// Binary name inside release archives (`scorpio`, not the Cargo package
/// `scorpio-analyst`).
const BIN_NAME: &str = "scorpio";

/// Windows archives contain `scorpio.exe`, while Unix archives contain `scorpio`.
const WINDOWS_BIN_NAME: &str = "scorpio.exe";

/// Wall-clock ceiling on the background update check so a stalled DNS or
/// captive portal can never delay user-visible output past this.
pub const UPDATE_CHECK_TIMEOUT_SECS: u64 = 5;

/// Explicit timeout for the blocking upgrade flow. Upgrades are expected to
/// take longer than the background check but must still fail in bounded time.
const UPGRADE_HTTP_TIMEOUT_SECS: u64 = 120;

/// Shared GitHub API base.
const GITHUB_API_BASE_URL: &str = "https://api.github.com";

/// Keep connect handshakes short even when the overall request timeout is longer.
const HTTP_CONNECT_TIMEOUT_SECS: u64 = 5;

// ── data types ─────────────────────────────────────────────────────────

/// Release descriptor independent of any HTTP client type. Pre-stripped of
/// any leading `v` in the version field so callers don't have to re-strip.
#[derive(Debug, Clone)]
pub(crate) struct UpdateRelease {
    pub version: String,
    pub assets: Vec<UpdateAsset>,
}

/// A single downloadable asset attached to a release.
#[derive(Debug, Clone)]
pub(crate) struct UpdateAsset {
    pub name: String,
    pub download_url: String,
}

#[derive(Debug, Deserialize)]
struct GithubReleaseResponse {
    tag_name: String,
    assets: Vec<GithubAssetResponse>,
}

#[derive(Debug, Deserialize)]
struct GithubAssetResponse {
    name: String,
    browser_download_url: String,
}

impl From<GithubReleaseResponse> for UpdateRelease {
    fn from(value: GithubReleaseResponse) -> Self {
        Self {
            version: value.tag_name.trim_start_matches('v').to_owned(),
            assets: value
                .assets
                .into_iter()
                .map(|asset| UpdateAsset {
                    name: asset.name,
                    download_url: asset.browser_download_url,
                })
                .collect(),
        }
    }
}

/// Result of an upgrade attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UpgradeOutcome {
    /// Already running the latest released version.
    UpToDate(String),
    /// Binary was replaced.
    Updated { from: String, to: String },
}

// ── trait seam ─────────────────────────────────────────────────────────

/// Abstraction over the blocking I/O of the release pipeline so both
/// [`check_latest_version`] and [`run_upgrade`] are unit-testable without a
/// network.
pub(crate) trait Updater: Send + Sync + 'static {
    fn current_version(&self) -> String;
    fn target(&self) -> String;
    fn get_release(&self, timeout: Duration) -> anyhow::Result<UpdateRelease>;
    fn fetch_sidecar(&self, asset: &UpdateAsset) -> anyhow::Result<String>;
    fn fetch_archive(&self, asset: &UpdateAsset) -> anyhow::Result<Vec<u8>>;
    fn install_archive(
        &self,
        asset_name: &str,
        archive_bytes: &[u8],
        bin_name_in_archive: &str,
    ) -> anyhow::Result<()>;
}

// ── GithubUpdater (real impl) ──────────────────────────────────────────

pub(crate) struct GithubUpdater {
    current_version: String,
}

impl GithubUpdater {
    pub(crate) fn new(current_version: impl Into<String>) -> Self {
        Self {
            current_version: current_version.into(),
        }
    }

    fn build_http_client(timeout: Duration) -> anyhow::Result<reqwest::blocking::Client> {
        reqwest::blocking::Client::builder()
            .user_agent(concat!("scorpio-analyst/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(HTTP_CONNECT_TIMEOUT_SECS))
            .timeout(timeout)
            .build()
            .context("failed to build HTTP client")
    }
}

impl Updater for GithubUpdater {
    fn current_version(&self) -> String {
        self.current_version.clone()
    }

    fn target(&self) -> String {
        self_update::get_target().to_owned()
    }

    fn get_release(&self, timeout: Duration) -> anyhow::Result<UpdateRelease> {
        fetch_github_release(
            &format!("{GITHUB_API_BASE_URL}/repos/{REPO_OWNER}/{REPO_NAME}/releases/latest"),
            timeout,
        )
    }

    fn fetch_sidecar(&self, asset: &UpdateAsset) -> anyhow::Result<String> {
        let client = Self::build_http_client(Duration::from_secs(UPGRADE_HTTP_TIMEOUT_SECS))?;
        let url = format!("{}.sha256", asset.download_url);
        let resp = client.get(&url).send().with_context(|| {
            format!("could not verify integrity: sidecar not available ({url})")
        })?;
        if !resp.status().is_success() {
            anyhow::bail!(
                "could not verify integrity: sidecar not available (HTTP {} at {url})",
                resp.status()
            );
        }
        resp.text()
            .context("could not verify integrity: failed to read sidecar body")
    }

    fn fetch_archive(&self, asset: &UpdateAsset) -> anyhow::Result<Vec<u8>> {
        let client = Self::build_http_client(Duration::from_secs(UPGRADE_HTTP_TIMEOUT_SECS))?;
        let resp = client
            .get(&asset.download_url)
            .header("Accept", "application/octet-stream")
            .send()
            .with_context(|| {
                format!(
                    "failed to download release archive from {}",
                    asset.download_url
                )
            })?;
        if !resp.status().is_success() {
            anyhow::bail!(
                "failed to download release archive (HTTP {} at {})",
                resp.status(),
                asset.download_url
            );
        }
        resp.bytes()
            .context("failed to read release archive body")
            .map(|b| b.to_vec())
    }

    fn install_archive(
        &self,
        asset_name: &str,
        archive_bytes: &[u8],
        bin_name_in_archive: &str,
    ) -> anyhow::Result<()> {
        install_verified_archive(
            asset_name,
            archive_bytes,
            bin_name_in_archive,
            &std::env::current_exe().context("failed to determine current executable path")?,
        )
    }
}

// ── pure helpers (unit-testable without I/O) ───────────────────────────

/// Pure semver comparison. Returns `true` iff `latest` parses to a strictly
/// greater version than `current`. Any parse error → `false` (never panics).
///
/// Both inputs may include a leading `v`; it is stripped defensively so this
/// function is safe to call on raw tags as well as pre-stripped versions.
pub(crate) fn should_notify(current: &str, latest: &str) -> bool {
    let current = current.trim_start_matches('v');
    let latest = latest.trim_start_matches('v');
    match (Version::parse(current), Version::parse(latest)) {
        (Ok(c), Ok(l)) => l > c,
        _ => false,
    }
}

/// Check that `archive_bytes` hashes to the hex digest named in `sidecar_text`.
///
/// The sidecar is either bare hex or `sha256sum`-style `{hex}  {filename}`; we
/// parse the first whitespace-delimited token.
pub(crate) fn verify_checksum(archive_bytes: &[u8], sidecar_text: &str) -> anyhow::Result<()> {
    let expected = sidecar_text
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow!("integrity check failed: sidecar is empty"))?;
    if expected.len() != 64 || !expected.chars().all(|c| c.is_ascii_hexdigit()) {
        anyhow::bail!("integrity check failed: malformed sidecar digest");
    }

    let mut hasher = Sha256::new();
    hasher.update(archive_bytes);
    let actual = hex::encode(hasher.finalize());

    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        anyhow::bail!(
            "integrity check failed: checksum mismatch (expected {expected}, got {actual})"
        )
    }
}

fn asset_archive_suffix(asset_name: &str) -> Option<&'static str> {
    if asset_name.ends_with(".tar.gz") {
        Some(".tar.gz")
    } else if asset_name.ends_with(".zip") {
        Some(".zip")
    } else {
        None
    }
}

fn expected_asset_name(target: &str, archive_suffix: &str) -> String {
    format!("{BIN_NAME}-{target}{archive_suffix}")
}

fn bin_name_in_archive(asset_name: &str) -> &'static str {
    if asset_name.ends_with(".zip") {
        WINDOWS_BIN_NAME
    } else {
        BIN_NAME
    }
}

fn select_release_asset(release: &UpdateRelease, target: &str) -> anyhow::Result<UpdateAsset> {
    release
        .assets
        .iter()
        .find(|asset| {
            asset_archive_suffix(&asset.name)
                .map(|suffix| asset.name == expected_asset_name(target, suffix))
                .unwrap_or(false)
        })
        .cloned()
        .ok_or_else(|| anyhow!("no release archive found for target '{target}'"))
}

fn fetch_github_release(url: &str, timeout: Duration) -> anyhow::Result<UpdateRelease> {
    let client = GithubUpdater::build_http_client(timeout)?;
    let response = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .with_context(|| format!("failed to fetch release metadata from {url}"))?;

    if !response.status().is_success() {
        anyhow::bail!(
            "failed to fetch release metadata (HTTP {} at {url})",
            response.status()
        );
    }

    response
        .json::<GithubReleaseResponse>()
        .context("failed to decode release metadata")
        .map(Into::into)
}

fn install_verified_archive(
    asset_name: &str,
    archive_bytes: &[u8],
    bin_name_in_archive: &str,
    install_path: &Path,
) -> anyhow::Result<()> {
    let archive_suffix = asset_archive_suffix(asset_name)
        .ok_or_else(|| anyhow!("unsupported release archive format: {asset_name}"))?;

    let temp_parent = install_path.parent().unwrap_or_else(|| Path::new("."));
    let temp_dir = tempfile::Builder::new()
        .prefix("scorpio-upgrade-")
        .tempdir_in(temp_parent)
        .context("failed to create temporary upgrade directory")?;
    let archive_path = temp_dir.path().join(asset_name);
    std::fs::write(&archive_path, archive_bytes)
        .with_context(|| format!("failed to stage verified archive {asset_name}"))?;

    let extracted_bin = temp_dir.path().join(bin_name_in_archive);
    match archive_suffix {
        ".tar.gz" => {
            self_update::Extract::from_source(&archive_path)
                .archive(self_update::ArchiveKind::Tar(Some(
                    self_update::Compression::Gz,
                )))
                .extract_file(temp_dir.path(), bin_name_in_archive)
                .with_context(|| {
                    format!("failed to extract {bin_name_in_archive} from {asset_name}")
                })?;
        }
        ".zip" => {
            self_update::Extract::from_source(&archive_path)
                .archive(self_update::ArchiveKind::Zip)
                .extract_file(temp_dir.path(), bin_name_in_archive)
                .with_context(|| {
                    format!("failed to extract {bin_name_in_archive} from {asset_name}")
                })?;
        }
        _ => unreachable!("archive suffix already validated"),
    }

    if install_path == std::env::current_exe()?.as_path() {
        self_update::self_replace::self_replace(&extracted_bin)
            .context("failed to replace current executable")?;
    } else {
        let backup_path = temp_dir.path().join("previous-binary");
        self_update::Move::from_source(&extracted_bin)
            .replace_using_temp(&backup_path)
            .to_dest(install_path)
            .with_context(|| format!("failed to replace binary at {}", install_path.display()))?;
    }

    Ok(())
}

/// Render the boxed "update available" notice.  Returns a String (the caller
/// is responsible for writing to stderr) to keep this function pure and
/// testable without stderr-capture tricks, mirroring `format_final_report`.
pub fn format_update_notice(current: &str, latest: &str) -> String {
    let title = "Update available".yellow().bold();
    let versions = format!("{current} → {latest}").bold();
    let hint = format!("Run {} to install.", "`scorpio upgrade`".cyan().bold());

    // Dynamic width: fit the widest visible line. ANSI escapes are invisible
    // to `char_count`, so measure the plain strings.
    let title_plain = "Update available";
    let versions_plain = format!("{current} → {latest}");
    let hint_plain = "Run `scorpio upgrade` to install.";
    let inner = [
        display_width(title_plain),
        display_width(&versions_plain),
        display_width(hint_plain),
    ]
    .into_iter()
    .max()
    .unwrap_or(0)
        + 2; // one-space padding each side

    let horizontal = "─".repeat(inner);
    let top = format!("╭{horizontal}╮");
    let bottom = format!("╰{horizontal}╯");
    let row = |content: &str, plain_width: usize| {
        let pad = inner - 1 - plain_width;
        format!("│ {content}{}│", " ".repeat(pad))
    };
    let title_row = row(&title.to_string(), display_width(title_plain));
    let versions_row = row(&versions.to_string(), display_width(&versions_plain));
    let hint_row = row(&hint, display_width(hint_plain));

    format!("{top}\n{title_row}\n{versions_row}\n{hint_row}\n{bottom}")
}

/// Approximate display width: one column per Unicode scalar value. Good enough
/// for ASCII + the arrow `→` used in our notice.
fn display_width(s: &str) -> usize {
    s.chars().count()
}

// ── check_latest_version ───────────────────────────────────────────────

/// Public entry used by the background task in `main.rs`. Never panics; every
/// failure path — network error, join panic, or version parse error — returns
/// `None`.
pub async fn check_latest_version() -> Option<String> {
    check_latest_version_with(GithubUpdater::new(env!("CARGO_PKG_VERSION"))).await
}

async fn check_latest_version_with<U: Updater>(updater: U) -> Option<String> {
    let current = updater.current_version();
    let fetch = task::spawn_blocking(move || {
        updater.get_release(Duration::from_secs(UPDATE_CHECK_TIMEOUT_SECS))
    });

    let release = match fetch.await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            tracing::debug!("update check skipped: {e:#}");
            return None;
        }
        Err(e) => {
            tracing::debug!("update check skipped (task join error): {e}");
            return None;
        }
    };

    if !should_notify(&current, &release.version) {
        return None;
    }

    // Return the canonical parsed semver string so downstream code never sees
    // a malformed or ANSI-injected tag.
    match Version::parse(release.version.trim_start_matches('v')) {
        Ok(v) => Some(v.to_string()),
        Err(e) => {
            tracing::debug!(
                "update check skipped: parse error for '{}': {e}",
                release.version
            );
            None
        }
    }
}

// ── notice dispatch ────────────────────────────────────────────────────

/// Pick between the boxed/colored notice (TTY) and the plain-text notice
/// (redirected stderr). Shared by the sync `try_*` and async `show_*`
/// dispatchers so both paths render identically.
fn format_notice_for_tty(current: &str, latest: &str, stderr_is_terminal: bool) -> String {
    if stderr_is_terminal {
        format_update_notice(current, latest)
    } else {
        format!("Update available: {current} -> {latest}\nRun `scorpio upgrade` to install.")
    }
}

/// Best-effort non-blocking drain of the background update-check result.
///
/// Returns `Some(formatted_notice)` when the channel has already delivered a
/// `Some(latest)` payload; returns `None` otherwise (channel empty, sender
/// dropped, or the task reported "up to date"). This is intentional: the
/// subcommand should not wait on the check.
///
/// `stderr_is_terminal` selects between the boxed/colored notice (TTY) and the
/// plain-text notice (redirected stderr). Callers typically pass
/// `std::io::stderr().is_terminal()`.
pub fn try_show_update_notice_with_tty(
    rx: tokio::sync::oneshot::Receiver<Option<String>>,
    current: &str,
    stderr_is_terminal: bool,
) -> Option<String> {
    // Consume the Receiver by value so callers can't reuse it.
    let mut rx = rx;
    match rx.try_recv() {
        Ok(Some(latest)) => Some(format_notice_for_tty(current, &latest, stderr_is_terminal)),
        Ok(None)
        | Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        | Err(tokio::sync::oneshot::error::TryRecvError::Closed) => None,
    }
}

/// Outcome of an async update-notice attempt.
///
/// Distinguishing `Pending(rx)` from `Resolved(None)` lets callers retry the
/// same receiver later (e.g. post-command) when the grace window is too short
/// to catch a cold network fetch pre-command.
pub enum NoticeOutcome {
    /// The background check finished and produced a notice.
    Ready(String),
    /// The background check finished with "up to date" or sender-dropped; no
    /// notice to show and no point in retrying.
    Resolved,
    /// The grace window elapsed before the check finished. The caller may try
    /// awaiting `rx` again later.
    Pending(tokio::sync::oneshot::Receiver<Option<String>>),
}

/// Awaits the background update-check result up to `grace`, then dispatches.
///
/// Preferred over [`try_show_update_notice_with_tty`] for fast subcommands
/// that would otherwise race the background HTTP call. On grace-timeout the
/// receiver is returned inside [`NoticeOutcome::Pending`] so callers can
/// retry (useful for showing the notice pre-banner *and* post-command).
pub async fn show_update_notice_with_tty(
    rx: tokio::sync::oneshot::Receiver<Option<String>>,
    current: &str,
    stderr_is_terminal: bool,
    grace: Duration,
) -> NoticeOutcome {
    // `oneshot::Receiver` is `Unpin`, so `&mut rx` is itself a `Future` and
    // can be awaited inside `select!` without consuming `rx`. If the sleep
    // arm wins, `rx` is still alive and returned to the caller.
    let mut rx = rx;
    tokio::select! {
        biased;
        result = &mut rx => match result {
            Ok(Some(latest)) => {
                NoticeOutcome::Ready(format_notice_for_tty(current, &latest, stderr_is_terminal))
            }
            // Up-to-date or sender dropped (background task panicked). Either
            // way, no notice is coming on this channel.
            Ok(None) | Err(_) => NoticeOutcome::Resolved,
        },
        _ = tokio::time::sleep(grace) => NoticeOutcome::Pending(rx),
    }
}

// ── run_upgrade ────────────────────────────────────────────────────────

/// Performs the checksum-verified self-update flow. The binary is replaced only
/// from the archive bytes that were successfully verified against the release
/// `.sha256` sidecar.
pub async fn run_upgrade() -> anyhow::Result<()> {
    run_upgrade_with(GithubUpdater::new(env!("CARGO_PKG_VERSION"))).await
}

async fn run_upgrade_with<U: Updater>(updater: U) -> anyhow::Result<()> {
    let current = updater.current_version();
    println!("Current version: {}", current.bold());

    // Pre-flight: the installed binary must be writable or the upgrade will
    // fail later with a less helpful message.
    let exe = std::env::current_exe().context("failed to determine current executable path")?;
    check_binary_writable(&exe)?;

    let outcome = task::spawn_blocking(move || -> anyhow::Result<UpgradeOutcome> {
        let release = updater
            .get_release(Duration::from_secs(UPGRADE_HTTP_TIMEOUT_SECS))
            .context("failed to fetch latest release")?;

        // Short-circuit if no newer release exists. Skipping the sidecar +
        // archive fetches also spares an unnecessary network round-trip.
        if !should_notify(&current, &release.version) {
            return Ok(UpgradeOutcome::UpToDate(current.clone()));
        }

        let target = updater.target();
        let asset = select_release_asset(&release, &target)?;

        let sidecar = updater.fetch_sidecar(&asset)?;
        let archive = updater.fetch_archive(&asset)?;
        verify_checksum(&archive, &sidecar)
            .with_context(|| format!("integrity check failed for {}", asset.name))?;

        updater
            .install_archive(&asset.name, &archive, bin_name_in_archive(&asset.name))
            .with_context(|| format!("failed to install verified archive {}", asset.name))?;

        Ok(UpgradeOutcome::Updated {
            from: current,
            to: release.version,
        })
    })
    .await
    .map_err(|e| anyhow!("upgrade task failed to join: {e}"))??;

    match outcome {
        UpgradeOutcome::UpToDate(v) => {
            println!("{} (v{v})", "Already up to date".green().bold());
        }
        UpgradeOutcome::Updated { from, to } => {
            println!("{}: v{from} → v{to}", "Updated successfully".green().bold());
        }
    }
    Ok(())
}

/// Returns `Ok(())` if `path` appears writable. On Unix, uses the filesystem
/// metadata `readonly` bit as a coarse indicator — good enough to give the
/// user a clear pre-flight error instead of a confusing replace-time failure.
pub(crate) fn check_binary_writable(path: &Path) -> anyhow::Result<()> {
    let metadata =
        std::fs::metadata(path).with_context(|| format!("failed to stat {}", path.display()))?;
    if metadata.permissions().readonly() {
        anyhow::bail!(
            "cannot replace binary at {}: permission denied (re-run with appropriate permissions)",
            path.display()
        );
    }
    Ok(())
}

// ── tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::io::Read;
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread;

    // ── should_notify ──────────────────────────────────────────────

    mod should_notify {
        use super::*;

        #[test]
        fn newer_latest_returns_true() {
            assert!(should_notify("0.2.0", "v0.3.0"));
            assert!(should_notify("0.2.0", "0.3.0"));
        }

        #[test]
        fn equal_returns_false() {
            assert!(!should_notify("0.3.0", "v0.3.0"));
        }

        #[test]
        fn current_newer_than_release_returns_false() {
            assert!(!should_notify("0.3.1", "v0.3.0"));
        }

        #[test]
        fn prerelease_is_lower_than_release() {
            // 0.3.0-beta.1 is semver-less-than 0.3.0 → no notification.
            assert!(!should_notify("0.3.0", "v0.3.0-beta.1"));
        }

        #[test]
        fn invalid_latest_returns_false_without_panic() {
            assert!(!should_notify("0.2.0", "not-semver"));
            assert!(!should_notify("0.2.0", ""));
        }

        #[test]
        fn invalid_current_returns_false_without_panic() {
            assert!(!should_notify("", "0.3.0"));
            assert!(!should_notify("garbage", "0.3.0"));
        }

        proptest! {
            #[test]
            fn equal_version_never_notifies(
                maj in 0u64..100, min in 0u64..100, patch in 0u64..100
            ) {
                let v = format!("{maj}.{min}.{patch}");
                prop_assert!(!should_notify(&v, &v));
            }

            #[test]
            fn arbitrary_strings_never_panic(
                a in ".*", b in ".*"
            ) {
                // Just exercise the code path; assertion is that it returns.
                let _ = should_notify(&a, &b);
            }
        }
    }

    // ── verify_checksum ────────────────────────────────────────────

    mod verify_checksum {
        use super::*;

        fn sha256_hex(bytes: &[u8]) -> String {
            let mut h = Sha256::new();
            h.update(bytes);
            hex::encode(h.finalize())
        }

        #[test]
        fn matching_digest_returns_ok() {
            let bytes = b"hello world";
            let sidecar = sha256_hex(bytes);
            assert!(verify_checksum(bytes, &sidecar).is_ok());
        }

        #[test]
        fn mismatched_digest_returns_err() {
            let bytes = b"hello world";
            // Flip one nibble.
            let mut bad = sha256_hex(bytes);
            bad.replace_range(0..1, "0");
            if bad == sha256_hex(bytes) {
                bad.replace_range(0..1, "1");
            }
            let err = verify_checksum(bytes, &bad).unwrap_err();
            assert!(
                format!("{err:#}").contains("checksum mismatch"),
                "got: {err:#}"
            );
        }

        #[test]
        fn sidecar_with_filename_parses_first_token() {
            let bytes = b"payload";
            let hex_digest = sha256_hex(bytes);
            let sidecar = format!("{hex_digest}  scorpio-x86_64.tar.gz");
            assert!(verify_checksum(bytes, &sidecar).is_ok());
        }

        #[test]
        fn bare_hex_sidecar_parses_correctly() {
            let bytes = b"payload";
            let hex_digest = sha256_hex(bytes);
            assert!(verify_checksum(bytes, &hex_digest).is_ok());
        }

        #[test]
        fn empty_sidecar_returns_err() {
            let err = verify_checksum(b"anything", "").unwrap_err();
            assert!(
                format!("{err:#}").contains("sidecar is empty")
                    || format!("{err:#}").contains("malformed"),
                "got: {err:#}"
            );
        }

        #[test]
        fn malformed_hex_returns_err() {
            let err = verify_checksum(b"anything", "not-hex-at-all").unwrap_err();
            assert!(format!("{err:#}").contains("malformed"), "got: {err:#}");
        }

        #[test]
        fn wrong_length_digest_is_rejected() {
            let err = verify_checksum(b"anything", "deadbeef").unwrap_err();
            assert!(format!("{err:#}").contains("malformed"), "got: {err:#}");
        }

        #[test]
        fn case_insensitive_hex_matches() {
            let bytes = b"hello";
            let lower = sha256_hex(bytes);
            let upper = lower.to_uppercase();
            assert!(verify_checksum(bytes, &upper).is_ok());
        }
    }

    // ── upgrade helpers ───────────────────────────────────────────

    mod upgrade_helpers {
        use super::*;

        fn build_tar_gz_archive_bytes(bin_path: &str, contents: &[u8]) -> Vec<u8> {
            let mut tar = tar::Builder::new(Vec::new());
            let mut header = tar::Header::new_gnu();
            header.set_path(bin_path).unwrap();
            header.set_size(contents.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append(&header, contents).unwrap();
            let tar_bytes = tar.into_inner().unwrap();

            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(&tar_bytes).unwrap();
            encoder.finish().unwrap()
        }

        fn build_zip_archive_bytes(bin_path: &str, contents: &[u8]) -> Vec<u8> {
            let cursor = std::io::Cursor::new(Vec::new());
            let mut zip = zip::ZipWriter::new(cursor);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file(bin_path, options).unwrap();
            zip.write_all(contents).unwrap();
            zip.finish().unwrap().into_inner()
        }

        #[test]
        fn select_release_asset_ignores_sha256_sidecars() {
            let release = UpdateRelease {
                version: "0.3.0".into(),
                assets: vec![
                    UpdateAsset {
                        name: "scorpio-aarch64-apple-darwin.tar.gz.sha256".into(),
                        download_url:
                            "https://example.invalid/scorpio-aarch64-apple-darwin.tar.gz.sha256"
                                .into(),
                    },
                    UpdateAsset {
                        name: "scorpio-aarch64-apple-darwin.tar.gz".into(),
                        download_url: "https://example.invalid/scorpio-aarch64-apple-darwin.tar.gz"
                            .into(),
                    },
                ],
            };

            let asset = select_release_asset(&release, "aarch64-apple-darwin").unwrap();
            assert_eq!(asset.name, "scorpio-aarch64-apple-darwin.tar.gz");
        }

        #[test]
        fn select_release_asset_accepts_windows_zip_archive() {
            let release = UpdateRelease {
                version: "0.3.0".into(),
                assets: vec![
                    UpdateAsset {
                        name: "scorpio-x86_64-pc-windows-msvc.zip.sha256".into(),
                        download_url:
                            "https://example.invalid/scorpio-x86_64-pc-windows-msvc.zip.sha256"
                                .into(),
                    },
                    UpdateAsset {
                        name: "scorpio-x86_64-pc-windows-msvc.zip".into(),
                        download_url: "https://example.invalid/scorpio-x86_64-pc-windows-msvc.zip"
                            .into(),
                    },
                ],
            };

            let asset = select_release_asset(&release, "x86_64-pc-windows-msvc").unwrap();
            assert_eq!(asset.name, "scorpio-x86_64-pc-windows-msvc.zip");
        }

        #[test]
        fn bin_name_in_archive_uses_exe_for_zip_assets() {
            assert_eq!(
                bin_name_in_archive("scorpio-x86_64-pc-windows-msvc.zip"),
                "scorpio.exe"
            );
            assert_eq!(
                bin_name_in_archive("scorpio-aarch64-apple-darwin.tar.gz"),
                "scorpio"
            );
        }

        #[test]
        fn install_verified_archive_replaces_destination_from_tar_gz_bytes() {
            let dir = tempfile::tempdir().unwrap();
            let install_path = dir.path().join("scorpio");
            std::fs::write(&install_path, b"old-binary").unwrap();

            let archive = build_tar_gz_archive_bytes("scorpio", b"new-binary");

            install_verified_archive(
                "scorpio-aarch64-apple-darwin.tar.gz",
                &archive,
                "scorpio",
                &install_path,
            )
            .unwrap();

            assert_eq!(std::fs::read(&install_path).unwrap(), b"new-binary");
        }

        #[test]
        fn install_verified_archive_replaces_destination_from_zip_bytes() {
            let dir = tempfile::tempdir().unwrap();
            let install_path = dir.path().join("scorpio.exe");
            std::fs::write(&install_path, b"old-binary").unwrap();

            let archive = build_zip_archive_bytes("scorpio.exe", b"new-binary");

            install_verified_archive(
                "scorpio-x86_64-pc-windows-msvc.zip",
                &archive,
                "scorpio.exe",
                &install_path,
            )
            .unwrap();

            assert_eq!(std::fs::read(&install_path).unwrap(), b"new-binary");
        }

        #[test]
        fn fetch_github_release_returns_timeout_error_for_hanging_server() {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let handle = thread::spawn(move || {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut buf = [0_u8; 1024];
                    let _ = stream.read(&mut buf);
                    thread::sleep(Duration::from_millis(700));
                }
            });

            let release_url =
                format!("http://{addr}/repos/{REPO_OWNER}/{REPO_NAME}/releases/latest");
            let err = fetch_github_release(&release_url, Duration::from_millis(150)).unwrap_err();
            assert!(
                format!("{err:#}").to_lowercase().contains("timed out"),
                "expected timeout error, got: {err:#}"
            );

            handle.join().unwrap();
        }
    }

    // ── format_update_notice ───────────────────────────────────────

    mod format_update_notice_tests {
        use super::*;

        #[test]
        fn contains_both_versions_and_upgrade_hint() {
            let out = format_update_notice("0.2.1", "0.3.0");
            assert!(out.contains("0.2.1"), "missing current version in: {out}");
            assert!(out.contains("0.3.0"), "missing latest version in: {out}");
            assert!(
                out.contains("scorpio upgrade"),
                "missing upgrade hint in: {out}"
            );
            assert!(out.contains('╭') && out.contains('╰') && out.contains('│'));
        }

        #[test]
        fn equal_versions_do_not_panic_or_return_empty() {
            // Defensive: caller should not invoke with equal versions, but if
            // they do we must not panic.
            let out = format_update_notice("0.2.1", "0.2.1");
            assert!(!out.is_empty());
        }

        proptest! {
            #[test]
            fn arbitrary_inputs_never_panic(a in ".*", b in ".*") {
                let out = format_update_notice(&a, &b);
                prop_assert!(!out.is_empty());
            }
        }
    }

    // ── MockUpdater + async run_upgrade_with ───────────────────────

    #[derive(Clone)]
    enum Reply<T> {
        Ok(T),
        Err(&'static str),
        Panic,
    }

    impl<T> Reply<T> {
        fn take(self) -> anyhow::Result<T> {
            match self {
                Reply::Ok(v) => Ok(v),
                Reply::Err(msg) => Err(anyhow!(msg)),
                Reply::Panic => panic!("mock updater panicked"),
            }
        }
    }

    #[derive(Clone)]
    struct MockUpdater {
        current_version: String,
        target: String,
        release: std::sync::Arc<std::sync::Mutex<Option<Reply<UpdateRelease>>>>,
        sidecar: std::sync::Arc<std::sync::Mutex<Option<Reply<String>>>>,
        archive: std::sync::Arc<std::sync::Mutex<Option<Reply<Vec<u8>>>>>,
        install: std::sync::Arc<std::sync::Mutex<Option<Reply<()>>>>,
    }

    impl MockUpdater {
        fn new(current: &str, target: &str) -> Self {
            Self {
                current_version: current.to_string(),
                target: target.to_string(),
                release: std::sync::Arc::new(std::sync::Mutex::new(None)),
                sidecar: std::sync::Arc::new(std::sync::Mutex::new(None)),
                archive: std::sync::Arc::new(std::sync::Mutex::new(None)),
                install: std::sync::Arc::new(std::sync::Mutex::new(None)),
            }
        }

        fn with_release(self, r: Reply<UpdateRelease>) -> Self {
            *self.release.lock().unwrap() = Some(r);
            self
        }
        fn with_sidecar(self, r: Reply<String>) -> Self {
            *self.sidecar.lock().unwrap() = Some(r);
            self
        }
        fn with_archive(self, r: Reply<Vec<u8>>) -> Self {
            *self.archive.lock().unwrap() = Some(r);
            self
        }
        fn with_install(self, r: Reply<()>) -> Self {
            *self.install.lock().unwrap() = Some(r);
            self
        }
    }

    impl Updater for MockUpdater {
        fn current_version(&self) -> String {
            self.current_version.clone()
        }
        fn target(&self) -> String {
            self.target.clone()
        }
        fn get_release(&self, _timeout: Duration) -> anyhow::Result<UpdateRelease> {
            self.release
                .lock()
                .unwrap()
                .take()
                .expect("mock get_release called without a configured reply")
                .take()
        }
        fn fetch_sidecar(&self, _asset: &UpdateAsset) -> anyhow::Result<String> {
            self.sidecar
                .lock()
                .unwrap()
                .take()
                .expect("mock fetch_sidecar called without a configured reply")
                .take()
        }
        fn fetch_archive(&self, _asset: &UpdateAsset) -> anyhow::Result<Vec<u8>> {
            self.archive
                .lock()
                .unwrap()
                .take()
                .expect("mock fetch_archive called without a configured reply")
                .take()
        }
        fn install_archive(
            &self,
            _asset_name: &str,
            _archive_bytes: &[u8],
            _bin_name_in_archive: &str,
        ) -> anyhow::Result<()> {
            self.install
                .lock()
                .unwrap()
                .take()
                .expect("mock install_archive called without a configured reply")
                .take()
        }
    }

    fn sample_release(version: &str, target: &str) -> UpdateRelease {
        UpdateRelease {
            version: version.to_string(),
            assets: vec![UpdateAsset {
                name: format!("scorpio-{target}.tar.gz"),
                download_url: format!("https://example.invalid/scorpio-{target}.tar.gz"),
            }],
        }
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(bytes);
        hex::encode(h.finalize())
    }

    // ── run_upgrade_with ───────────────────────────────────────────

    #[tokio::test]
    async fn run_upgrade_up_to_date_short_circuits_without_sidecar_fetch() {
        let m = MockUpdater::new("0.2.1", "aarch64-apple-darwin")
            .with_release(Reply::Ok(sample_release("0.2.1", "aarch64-apple-darwin")));
        let res = run_upgrade_with(m).await;
        assert!(res.is_ok(), "expected Ok, got {res:?}");
    }

    #[tokio::test]
    async fn run_upgrade_happy_path_with_valid_sidecar_returns_ok() {
        let archive = b"fake-archive-bytes".to_vec();
        let sidecar = sha256_hex(&archive);
        let m = MockUpdater::new("0.2.1", "aarch64-apple-darwin")
            .with_release(Reply::Ok(sample_release("0.3.0", "aarch64-apple-darwin")))
            .with_sidecar(Reply::Ok(sidecar))
            .with_archive(Reply::Ok(archive))
            .with_install(Reply::Ok(()));
        let res = run_upgrade_with(m).await;
        assert!(res.is_ok(), "expected Ok, got {res:?}");
    }

    #[tokio::test]
    async fn run_upgrade_sidecar_404_returns_err() {
        let m = MockUpdater::new("0.2.1", "aarch64-apple-darwin")
            .with_release(Reply::Ok(sample_release("0.3.0", "aarch64-apple-darwin")))
            .with_sidecar(Reply::Err(
                "could not verify integrity: sidecar not available (HTTP 404)",
            ));
        let res = run_upgrade_with(m).await;
        let err = res.expect_err("expected Err on sidecar 404");
        assert!(
            format!("{err:#}").contains("sidecar not available"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn run_upgrade_checksum_mismatch_returns_err_and_skips_perform() {
        let archive = b"real-archive".to_vec();
        let fake_sidecar = sha256_hex(b"different-content");
        let m = MockUpdater::new("0.2.1", "aarch64-apple-darwin")
            .with_release(Reply::Ok(sample_release("0.3.0", "aarch64-apple-darwin")))
            .with_sidecar(Reply::Ok(fake_sidecar))
            .with_archive(Reply::Ok(archive));
        // Intentionally do NOT configure `install` — if it is called, the mock
        // panics with "called without a configured reply".
        let res = run_upgrade_with(m).await;
        let err = res.expect_err("expected checksum mismatch Err");
        assert!(
            format!("{err:#}").contains("integrity check failed"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn run_upgrade_no_matching_asset_returns_err() {
        // release has only an aarch64-apple-darwin asset; we ask for linux.
        let mut release = sample_release("0.3.0", "aarch64-apple-darwin");
        // Overwrite asset name so it doesn't match "x86_64-unknown-linux-gnu".
        release.assets[0].name = "scorpio-aarch64-apple-darwin.tar.gz".into();
        let m =
            MockUpdater::new("0.2.1", "x86_64-unknown-linux-gnu").with_release(Reply::Ok(release));
        let res = run_upgrade_with(m).await;
        let err = res.expect_err("expected no-asset Err");
        assert!(
            format!("{err:#}").contains("no release archive found"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn run_upgrade_install_panic_maps_to_err_not_unwind() {
        let archive = b"bytes".to_vec();
        let sidecar = sha256_hex(&archive);
        let m = MockUpdater::new("0.2.1", "aarch64-apple-darwin")
            .with_release(Reply::Ok(sample_release("0.3.0", "aarch64-apple-darwin")))
            .with_sidecar(Reply::Ok(sidecar))
            .with_archive(Reply::Ok(archive))
            .with_install(Reply::Panic);
        let res = run_upgrade_with(m).await;
        let err = res.expect_err("expected Err on install panic");
        assert!(
            format!("{err:#}").contains("upgrade task failed to join"),
            "got: {err:#}"
        );
    }

    // ── check_latest_version_with ──────────────────────────────────

    #[tokio::test]
    async fn check_returns_some_on_newer_release() {
        let m = MockUpdater::new("0.2.1", "aarch64-apple-darwin")
            .with_release(Reply::Ok(sample_release("0.3.0", "aarch64-apple-darwin")));
        let got = check_latest_version_with(m).await;
        assert_eq!(got.as_deref(), Some("0.3.0"));
    }

    #[tokio::test]
    async fn check_returns_none_on_equal_release() {
        let m = MockUpdater::new("0.3.0", "aarch64-apple-darwin")
            .with_release(Reply::Ok(sample_release("0.3.0", "aarch64-apple-darwin")));
        let got = check_latest_version_with(m).await;
        assert_eq!(got, None);
    }

    #[tokio::test]
    async fn check_returns_none_on_fetch_err() {
        let m = MockUpdater::new("0.2.1", "aarch64-apple-darwin")
            .with_release(Reply::Err("network unavailable"));
        let got = check_latest_version_with(m).await;
        assert_eq!(got, None);
    }

    #[tokio::test]
    async fn check_returns_none_on_get_release_panic() {
        // Panic inside spawn_blocking → JoinError → None (totality invariant).
        let m = MockUpdater::new("0.2.1", "aarch64-apple-darwin").with_release(Reply::Panic);
        let got = check_latest_version_with(m).await;
        assert_eq!(got, None);
    }

    #[tokio::test]
    async fn check_returns_none_on_unparseable_release_tag() {
        let m = MockUpdater::new("0.2.1", "aarch64-apple-darwin").with_release(Reply::Ok(
            UpdateRelease {
                version: "not-a-semver".into(),
                assets: vec![],
            },
        ));
        let got = check_latest_version_with(m).await;
        assert_eq!(got, None);
    }

    // ── try_show_update_notice_with_tty ────────────────────────────

    mod notice_dispatch {
        use super::*;

        #[tokio::test]
        async fn returns_formatted_notice_when_some() {
            let (tx, rx) = tokio::sync::oneshot::channel();
            tx.send(Some("0.3.0".to_string())).unwrap();
            let got = try_show_update_notice_with_tty(rx, "0.2.1", true);
            let s = got.expect("expected Some");
            assert!(s.contains("0.2.1") && s.contains("0.3.0"));
        }

        #[tokio::test]
        async fn returns_boxed_notice_for_terminal_stderr() {
            let (tx, rx) = tokio::sync::oneshot::channel();
            tx.send(Some("0.3.0".to_string())).unwrap();

            let got = try_show_update_notice_with_tty(rx, "0.2.1", true);
            let s = got.expect("expected Some");

            assert!(s.contains('╭') && s.contains('╰') && s.contains('│'));
        }

        #[tokio::test]
        async fn returns_plain_notice_for_non_terminal_stderr() {
            let (tx, rx) = tokio::sync::oneshot::channel();
            tx.send(Some("0.3.0".to_string())).unwrap();

            let got = try_show_update_notice_with_tty(rx, "0.2.1", false);
            let s = got.expect("expected Some");

            assert!(s.contains("Update available"));
            assert!(s.contains("0.2.1") && s.contains("0.3.0"));
            assert!(s.contains("scorpio upgrade"));
            assert!(!s.contains('╭') && !s.contains('╰') && !s.contains('│'));
            assert!(!s.contains("\u{1b}"));
        }

        #[tokio::test]
        async fn returns_none_when_sender_reports_up_to_date() {
            let (tx, rx) = tokio::sync::oneshot::channel();
            tx.send(None).unwrap();
            assert!(try_show_update_notice_with_tty(rx, "0.2.1", true).is_none());
        }

        #[tokio::test]
        async fn returns_none_when_sender_not_yet_sent_empty() {
            // Intentional best-effort: if the check hasn't finished when the
            // subcommand returns, we silently skip — documented in R3.
            let (_tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
            assert!(try_show_update_notice_with_tty(rx, "0.2.1", true).is_none());
        }

        #[tokio::test]
        async fn returns_none_when_sender_dropped_disconnected() {
            let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
            drop(tx); // background task panicked before sending
            assert!(try_show_update_notice_with_tty(rx, "0.2.1", true).is_none());
        }
    }

    // ── show_update_notice_with_tty (async, with grace window) ─────

    mod async_notice_dispatch {
        use super::*;

        #[tokio::test]
        async fn returns_ready_when_sender_beats_grace() {
            let (tx, rx) = tokio::sync::oneshot::channel();
            tx.send(Some("0.3.0".into())).unwrap();
            let outcome =
                show_update_notice_with_tty(rx, "0.2.1", true, Duration::from_millis(500)).await;
            match outcome {
                NoticeOutcome::Ready(s) => {
                    assert!(s.contains("0.2.1") && s.contains("0.3.0"));
                    assert!(s.contains('╭')); // boxed form on tty
                }
                _ => panic!("expected Ready"),
            }
        }

        #[tokio::test]
        async fn waits_for_late_sender_within_grace() {
            let (tx, rx) = tokio::sync::oneshot::channel();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                let _ = tx.send(Some("0.3.0".into()));
            });
            let outcome =
                show_update_notice_with_tty(rx, "0.2.1", false, Duration::from_millis(500)).await;
            match outcome {
                NoticeOutcome::Ready(s) => {
                    // Non-tty plain form.
                    assert!(s.contains("Update available"));
                    assert!(s.contains("0.2.1") && s.contains("0.3.0"));
                    assert!(!s.contains('╭'));
                }
                _ => panic!("expected Ready"),
            }
        }

        #[tokio::test]
        async fn returns_pending_on_grace_timeout_preserving_rx() {
            // Sender exists but never sends within the grace. We must not
            // hang past `grace`, AND we must return the rx for retry.
            let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
            let started = std::time::Instant::now();
            let outcome =
                show_update_notice_with_tty(rx, "0.2.1", true, Duration::from_millis(50)).await;
            let elapsed = started.elapsed();
            assert!(
                elapsed < Duration::from_millis(500),
                "timeout should fire promptly, elapsed={elapsed:?}"
            );
            let mut rx = match outcome {
                NoticeOutcome::Pending(rx) => rx,
                other => panic!("expected Pending, got {}", outcome_kind(&other)),
            };
            // Receiver is still usable — deliver late and confirm the caller
            // could have observed it.
            tx.send(Some("0.3.0".into())).unwrap();
            let late = rx.try_recv().unwrap();
            assert_eq!(late.as_deref(), Some("0.3.0"));
        }

        #[tokio::test]
        async fn returns_resolved_when_sender_reports_up_to_date() {
            let (tx, rx) = tokio::sync::oneshot::channel();
            tx.send(None).unwrap();
            let outcome =
                show_update_notice_with_tty(rx, "0.2.1", true, Duration::from_millis(500)).await;
            assert!(matches!(outcome, NoticeOutcome::Resolved));
        }

        #[tokio::test]
        async fn returns_resolved_when_sender_dropped() {
            let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
            drop(tx);
            let outcome =
                show_update_notice_with_tty(rx, "0.2.1", true, Duration::from_millis(500)).await;
            assert!(matches!(outcome, NoticeOutcome::Resolved));
        }

        fn outcome_kind(o: &NoticeOutcome) -> &'static str {
            match o {
                NoticeOutcome::Ready(_) => "Ready",
                NoticeOutcome::Resolved => "Resolved",
                NoticeOutcome::Pending(_) => "Pending",
            }
        }
    }

    // ── check_binary_writable ──────────────────────────────────────

    mod binary_writable {
        use super::*;
        use std::io::Write;

        #[test]
        fn writable_file_returns_ok() {
            let mut tmp = tempfile::NamedTempFile::new().unwrap();
            tmp.write_all(b"x").unwrap();
            assert!(check_binary_writable(tmp.path()).is_ok());
        }

        #[test]
        fn readonly_file_returns_err() {
            let tmp = tempfile::NamedTempFile::new().unwrap();
            let mut perms = std::fs::metadata(tmp.path()).unwrap().permissions();
            perms.set_readonly(true);
            std::fs::set_permissions(tmp.path(), perms).unwrap();

            let res = check_binary_writable(tmp.path());
            let err = res.expect_err("expected Err for readonly file");
            assert!(format!("{err:#}").contains("permission"), "got: {err:#}");

            // Restore so the tempfile can be cleaned up.
            let mut perms = std::fs::metadata(tmp.path()).unwrap().permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            let _ = std::fs::set_permissions(tmp.path(), perms);
        }

        #[test]
        fn missing_file_returns_err() {
            let err = check_binary_writable(Path::new("/definitely/does/not/exist"))
                .expect_err("expected Err for missing path");
            assert!(
                format!("{err:#}").contains("failed to stat"),
                "got: {err:#}"
            );
        }
    }
}
