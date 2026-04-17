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

/// Wall-clock ceiling on the background update check so a stalled DNS or
/// captive portal can never delay user-visible output past this.
pub const UPDATE_CHECK_TIMEOUT_SECS: u64 = 5;

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
    fn get_release(&self) -> anyhow::Result<UpdateRelease>;
    fn fetch_sidecar(&self, asset: &UpdateAsset) -> anyhow::Result<String>;
    fn fetch_archive(&self, asset: &UpdateAsset) -> anyhow::Result<Vec<u8>>;
    fn perform_update(&self) -> anyhow::Result<UpgradeOutcome>;
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

    fn build_http_client() -> anyhow::Result<reqwest::blocking::Client> {
        reqwest::blocking::Client::builder()
            .user_agent(concat!("scorpio-analyst/", env!("CARGO_PKG_VERSION")))
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

    fn get_release(&self) -> anyhow::Result<UpdateRelease> {
        let update = self_update::backends::github::Update::configure()
            .repo_owner(REPO_OWNER)
            .repo_name(REPO_NAME)
            .bin_name(BIN_NAME)
            .current_version(&self.current_version)
            .show_output(false)
            .show_download_progress(false)
            .no_confirm(true)
            .build()
            .map_err(|e| anyhow!("failed to configure updater: {e}"))?;

        // Pre-release tags are already excluded by GitHub's "latest release"
        // API endpoint, so no application-side filtering is needed here.
        let release = update
            .get_latest_release()
            .map_err(|e| anyhow!("failed to fetch latest release: {e}"))?;

        Ok(UpdateRelease {
            version: release.version,
            assets: release
                .assets
                .into_iter()
                .map(|a| UpdateAsset {
                    name: a.name,
                    download_url: a.download_url,
                })
                .collect(),
        })
    }

    fn fetch_sidecar(&self, asset: &UpdateAsset) -> anyhow::Result<String> {
        let client = Self::build_http_client()?;
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
        let client = Self::build_http_client()?;
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

    fn perform_update(&self) -> anyhow::Result<UpgradeOutcome> {
        let update = self_update::backends::github::Update::configure()
            .repo_owner(REPO_OWNER)
            .repo_name(REPO_NAME)
            .bin_name(BIN_NAME)
            .current_version(&self.current_version)
            .show_output(true)
            .show_download_progress(true)
            .no_confirm(true)
            .build()
            .map_err(|e| anyhow!("failed to configure updater: {e}"))?;

        let status = update
            .update()
            .map_err(|e| anyhow!("upgrade failed: {e}"))?;

        Ok(match status {
            self_update::Status::UpToDate(v) => UpgradeOutcome::UpToDate(v),
            self_update::Status::Updated(v) => UpgradeOutcome::Updated {
                from: self.current_version.clone(),
                to: v,
            },
        })
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
/// failure path — network error, join panic, timeout, version parse error —
/// returns `None`.
pub async fn check_latest_version() -> Option<String> {
    check_latest_version_with(GithubUpdater::new(env!("CARGO_PKG_VERSION"))).await
}

async fn check_latest_version_with<U: Updater>(updater: U) -> Option<String> {
    let current = updater.current_version();

    let fetch = task::spawn_blocking(move || updater.get_release());

    let release =
        match tokio::time::timeout(Duration::from_secs(UPDATE_CHECK_TIMEOUT_SECS), fetch).await {
            Ok(Ok(Ok(r))) => r,
            Ok(Ok(Err(e))) => {
                tracing::debug!("update check skipped: {e:#}");
                return None;
            }
            Ok(Err(e)) => {
                // JoinError — the blocking task panicked. Absorb silently; the
                // background task is fire-and-forget.
                tracing::debug!("update check skipped (task join error): {e}");
                return None;
            }
            Err(_) => {
                tracing::debug!("update check skipped: timeout after {UPDATE_CHECK_TIMEOUT_SECS}s");
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

// ── try_show_update_notice ─────────────────────────────────────────────

/// Best-effort non-blocking drain of the background update-check result.
///
/// Returns `Some(formatted_notice)` when the channel has already delivered a
/// `Some(latest)` payload; returns `None` otherwise (channel empty, sender
/// dropped, or the task reported "up to date"). This is intentional: the
/// subcommand should not wait on the check.
pub fn try_show_update_notice(
    rx: tokio::sync::oneshot::Receiver<Option<String>>,
    current: &str,
) -> Option<String> {
    // Consume the Receiver by value so callers can't reuse it.
    let mut rx = rx;
    match rx.try_recv() {
        Ok(Some(latest)) => Some(format_update_notice(current, &latest)),
        Ok(None)
        | Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        | Err(tokio::sync::oneshot::error::TryRecvError::Closed) => None,
    }
}

// ── run_upgrade ────────────────────────────────────────────────────────

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
            .get_release()
            .context("failed to fetch latest release")?;

        // Short-circuit if no newer release exists. Skipping the sidecar +
        // archive fetches also spares an unnecessary network round-trip.
        if !should_notify(&current, &release.version) {
            return Ok(UpgradeOutcome::UpToDate(current.clone()));
        }

        // Pick the asset whose name contains the current target triple.
        let target = updater.target();
        let asset = release
            .assets
            .iter()
            .find(|a| a.name.contains(&target))
            .cloned()
            .ok_or_else(|| anyhow!("no release asset found for target '{target}'"))?;

        let sidecar = updater.fetch_sidecar(&asset)?;
        let archive = updater.fetch_archive(&asset)?;
        verify_checksum(&archive, &sidecar)
            .with_context(|| format!("integrity check failed for {}", asset.name))?;

        updater.perform_update()
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
        perform: std::sync::Arc<std::sync::Mutex<Option<Reply<UpgradeOutcome>>>>,
    }

    impl MockUpdater {
        fn new(current: &str, target: &str) -> Self {
            Self {
                current_version: current.to_string(),
                target: target.to_string(),
                release: std::sync::Arc::new(std::sync::Mutex::new(None)),
                sidecar: std::sync::Arc::new(std::sync::Mutex::new(None)),
                archive: std::sync::Arc::new(std::sync::Mutex::new(None)),
                perform: std::sync::Arc::new(std::sync::Mutex::new(None)),
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
        fn with_perform(self, r: Reply<UpgradeOutcome>) -> Self {
            *self.perform.lock().unwrap() = Some(r);
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
        fn get_release(&self) -> anyhow::Result<UpdateRelease> {
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
        fn perform_update(&self) -> anyhow::Result<UpgradeOutcome> {
            self.perform
                .lock()
                .unwrap()
                .take()
                .expect("mock perform_update called without a configured reply")
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
            .with_perform(Reply::Ok(UpgradeOutcome::Updated {
                from: "0.2.1".into(),
                to: "0.3.0".into(),
            }));
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
        // Intentionally do NOT configure `perform` — if it is called, the mock
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
            format!("{err:#}").contains("no release asset found"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn run_upgrade_perform_update_panic_maps_to_err_not_unwind() {
        let archive = b"bytes".to_vec();
        let sidecar = sha256_hex(&archive);
        let m = MockUpdater::new("0.2.1", "aarch64-apple-darwin")
            .with_release(Reply::Ok(sample_release("0.3.0", "aarch64-apple-darwin")))
            .with_sidecar(Reply::Ok(sidecar))
            .with_archive(Reply::Ok(archive))
            .with_perform(Reply::Panic);
        let res = run_upgrade_with(m).await;
        let err = res.expect_err("expected Err on perform panic");
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

    // ── try_show_update_notice ─────────────────────────────────────

    mod notice_dispatch {
        use super::*;

        #[tokio::test]
        async fn returns_formatted_notice_when_some() {
            let (tx, rx) = tokio::sync::oneshot::channel();
            tx.send(Some("0.3.0".to_string())).unwrap();
            let got = try_show_update_notice(rx, "0.2.1");
            let s = got.expect("expected Some");
            assert!(s.contains("0.2.1") && s.contains("0.3.0"));
        }

        #[tokio::test]
        async fn returns_none_when_sender_reports_up_to_date() {
            let (tx, rx) = tokio::sync::oneshot::channel();
            tx.send(None).unwrap();
            assert!(try_show_update_notice(rx, "0.2.1").is_none());
        }

        #[tokio::test]
        async fn returns_none_when_sender_not_yet_sent_empty() {
            // Intentional best-effort: if the check hasn't finished when the
            // subcommand returns, we silently skip — documented in R3.
            let (_tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
            assert!(try_show_update_notice(rx, "0.2.1").is_none());
        }

        #[tokio::test]
        async fn returns_none_when_sender_dropped_disconnected() {
            let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
            drop(tx); // background task panicked before sending
            assert!(try_show_update_notice(rx, "0.2.1").is_none());
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
