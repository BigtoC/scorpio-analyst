//! Copilot OAuth scope validation and identity-binding record.
//!
//! rig-core 0.36.0 does not surface OAuth scopes from cached grants. This module
//! relies on a live `GET /user` call against `https://api.github.com` to confirm
//! identity and inspect the `X-OAuth-Scopes` header. rig's `api-key.json` is used
//! only for local cache inspection and runtime-base validation.

use std::{path::Path, time::Duration};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::error::TradingError;

/// Scorpio-owned identity binding written to `<token_dir>/scorpio-identity.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ScorpioIdentityBinding {
    /// Numeric GitHub account ID (mandatory; survives login renames).
    pub github_id: u64,
    /// GitHub login at time of authorization (display only).
    pub github_login: String,
    /// Unix timestamp (seconds) at which this binding was written.
    pub written_at: i64,
}

/// Read the identity binding from the token directory.
pub fn read_binding(token_dir: &Path) -> Result<ScorpioIdentityBinding> {
    let path = token_dir.join("scorpio-identity.json");
    verify_copilot_secret_file_secure(&path)?;
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("identity binding missing at {}", path.display()))?;
    let parsed: ScorpioIdentityBinding =
        serde_json::from_str(&raw).context("identity binding is malformed JSON")?;
    if parsed.github_id == 0 {
        return Err(anyhow::anyhow!(
            "identity binding missing github_id (must be a non-zero numeric account ID)"
        ));
    }
    Ok(parsed)
}

/// Write the identity binding atomically with `0o600` permissions on Unix.
pub fn write_binding(token_dir: &Path, binding: &ScorpioIdentityBinding) -> Result<()> {
    let path = token_dir.join("scorpio-identity.json");
    let json = serde_json::to_string_pretty(binding)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&tmp, perms)?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Verify that a Copilot secret/cache file is a regular non-symlink file.
pub fn verify_copilot_secret_file_secure(path: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path)
        .with_context(|| format!("secret file missing at {}", path.display()))?;
    if !meta.file_type().is_file() || meta.file_type().is_symlink() {
        return Err(anyhow::anyhow!(
            "secret file at {} must be a regular non-symlink file",
            path.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::PermissionsExt;
        let uid = unsafe { libc::geteuid() };
        if meta.uid() != uid {
            return Err(anyhow::anyhow!(
                "secret file at {} is not owned by the current user",
                path.display()
            ));
        }
        let mode = meta.permissions().mode() & 0o777;
        if mode != 0o600 {
            return Err(anyhow::anyhow!(
                "secret file at {} has insecure permissions {:o} (expected exactly 0o600)",
                path.display(),
                mode
            ));
        }
    }
    Ok(())
}

/// Required GitHub OAuth scope on the Copilot bootstrap token.
pub const REQUIRED_SCOPE: &str = "read:user";

/// Live identity returned by `GET https://api.github.com/user`.
#[derive(Debug)]
pub struct GitHubIdentity {
    pub id: u64,
    pub login: String,
    pub scopes: Vec<String>,
}

/// Parsed subset of rig's cached `api-key.json` record.
///
/// Scorpio reads this file only to validate the cached Copilot runtime base and
/// related local metadata before trusting runtime auth state.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ApiKeyRecord {
    pub token: Option<String>,
    pub expires_at: Option<i64>,
    pub endpoints: Option<ApiKeyEndpoints>,
    pub bootstrap_token_fingerprint: Option<String>,
}

/// Nested endpoint URLs from rig's cached `api-key.json` record.
///
/// Only the `api` field is consumed in this slice, as part of the local
/// Copilot runtime-base allowlist check.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ApiKeyEndpoints {
    pub api: Option<String>,
}

/// Read rig's cached Copilot API-key record from `api-key.json`.
pub fn read_api_key_record(token_dir: &Path) -> Result<ApiKeyRecord, TradingError> {
    let path = token_dir.join("api-key.json");
    verify_copilot_secret_file_secure(&path)
        .map_err(|e| TradingError::Config(anyhow::anyhow!(e.to_string())))?;
    let raw = std::fs::read_to_string(&path).map_err(|e| {
        TradingError::Config(anyhow::anyhow!("failed to read {}: {e}", path.display()))
    })?;

    let parsed: ApiKeyRecord = serde_json::from_str(&raw).map_err(|e| {
        TradingError::Config(anyhow::anyhow!("failed to parse {}: {e}", path.display()))
    })?;

    Ok(parsed)
}

/// Validate the cached Copilot runtime base from `api-key.json.endpoints.api`.
pub fn validate_copilot_runtime_base(record: &ApiKeyRecord) -> Result<(), TradingError> {
    let Some(raw) = record
        .endpoints
        .as_ref()
        .and_then(|endpoints| endpoints.api.as_deref())
    else {
        return Ok(());
    };

    let parsed = url::Url::parse(raw).map_err(|e| {
        TradingError::Config(anyhow::anyhow!(
            "Copilot runtime base in api-key.json is not a valid URL: {e}"
        ))
    })?;

    if parsed.scheme() != "https" {
        return Err(TradingError::Config(anyhow::anyhow!(
            "Copilot runtime base must use https"
        )));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(TradingError::Config(anyhow::anyhow!(
            "Copilot runtime base must not contain user/password info"
        )));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(TradingError::Config(anyhow::anyhow!(
            "Copilot runtime base must not contain query or fragment components"
        )));
    }

    let host = parsed.host_str().ok_or_else(|| {
        TradingError::Config(anyhow::anyhow!("Copilot runtime base is missing a host"))
    })?;
    if parsed.port().is_some() {
        return Err(TradingError::Config(anyhow::anyhow!(
            "Copilot runtime base must not include an explicit port"
        )));
    }

    match host {
        "api.githubcopilot.com" | "api.individual.githubcopilot.com" => Ok(()),
        other => Err(TradingError::Config(anyhow::anyhow!(
            "Copilot runtime base host {other:?} is not allowed in this slice"
        ))),
    }
}

/// Call `GET https://api.github.com/user` with the given access token.
pub async fn fetch_github_identity(access_token: &str) -> Result<GitHubIdentity, TradingError> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .user_agent("scorpio-analyst")
        .build()
        .map_err(|e| TradingError::Config(anyhow::anyhow!("reqwest client build failed: {e}")))?;
    let resp = client
        .get("https://api.github.com/user")
        .bearer_auth(access_token)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| TradingError::Config(anyhow::anyhow!("GET /user failed: {e}")))?;

    let scopes_header = resp
        .headers()
        .get("X-OAuth-Scopes")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();

    if !resp.status().is_success() {
        return Err(TradingError::Config(anyhow::anyhow!(
            "GET /user returned status {}",
            resp.status()
        )));
    }

    #[derive(Deserialize)]
    struct UserResponse {
        id: u64,
        login: String,
    }
    let body: UserResponse = resp
        .json()
        .await
        .map_err(|e| TradingError::Config(anyhow::anyhow!("GET /user body parse: {e}")))?;

    let scopes: Vec<String> = scopes_header
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();

    Ok(GitHubIdentity {
        id: body.id,
        login: body.login,
        scopes,
    })
}

/// Reject the grant unless it contains exactly the expected scope: `read:user`.
pub fn validate_scope(scopes: &[String]) -> Result<(), TradingError> {
    if scopes.is_empty() {
        return Err(TradingError::Config(anyhow::anyhow!(
            "X-OAuth-Scopes header was empty; refusing to trust this grant"
        )));
    }
    if !scopes.iter().any(|scope| scope == REQUIRED_SCOPE) {
        return Err(TradingError::Config(anyhow::anyhow!(
            "Copilot bootstrap is missing required scope {REQUIRED_SCOPE:?}"
        )));
    }
    for scope in scopes {
        if scope != REQUIRED_SCOPE {
            return Err(TradingError::Config(anyhow::anyhow!(
                "Copilot bootstrap has unexpected scope {scope:?}; required scope is exactly {REQUIRED_SCOPE:?} in this slice"
            )));
        }
    }
    Ok(())
}

/// Read the access token from rig's managed cache file.
pub fn read_access_token(token_dir: &Path) -> Result<String, TradingError> {
    let path = token_dir.join("access-token");
    verify_copilot_secret_file_secure(&path)
        .map_err(|e| TradingError::Config(anyhow::anyhow!(e.to_string())))?;
    let raw = std::fs::read_to_string(&path).map_err(|e| {
        TradingError::Config(anyhow::anyhow!(
            "failed to read access token at {}: {e}",
            path.display()
        ))
    })?;
    let trimmed = raw.trim().to_owned();
    if trimmed.is_empty() {
        return Err(TradingError::Config(anyhow::anyhow!(
            "access token file at {} is empty",
            path.display()
        )));
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_scope_accepts_read_user_only() {
        assert!(validate_scope(&["read:user".to_owned()]).is_ok());
    }

    #[test]
    fn validate_scope_rejects_empty() {
        assert!(validate_scope(&[]).is_err());
    }

    #[test]
    fn validate_scope_rejects_missing_read_user() {
        assert!(validate_scope(&["other".to_owned()]).is_err());
    }

    #[test]
    fn validate_scope_rejects_repo_scope() {
        assert!(validate_scope(&["read:user".to_owned(), "repo".to_owned()]).is_err());
    }

    #[test]
    fn validate_scope_rejects_unexpected_scope_even_when_not_in_old_denylist() {
        assert!(validate_scope(&["read:user".to_owned(), "read:org".to_owned()]).is_err());
    }

    #[test]
    fn validate_copilot_runtime_base_accepts_allowed_hosts() {
        for raw in [
            "https://api.githubcopilot.com",
            "https://api.individual.githubcopilot.com",
        ] {
            let record = ApiKeyRecord {
                endpoints: Some(ApiKeyEndpoints {
                    api: Some(raw.to_owned()),
                }),
                ..Default::default()
            };
            assert!(
                validate_copilot_runtime_base(&record).is_ok(),
                "unexpected rejection for {raw}"
            );
        }
    }

    #[test]
    fn validate_copilot_runtime_base_rejects_untrusted_host() {
        let record = ApiKeyRecord {
            endpoints: Some(ApiKeyEndpoints {
                api: Some("https://evil.example.com".to_owned()),
            }),
            ..Default::default()
        };
        assert!(validate_copilot_runtime_base(&record).is_err());
    }

    #[test]
    fn validate_copilot_runtime_base_rejects_explicit_port() {
        let record = ApiKeyRecord {
            endpoints: Some(ApiKeyEndpoints {
                api: Some("https://api.githubcopilot.com:444".to_owned()),
            }),
            ..Default::default()
        };
        assert!(validate_copilot_runtime_base(&record).is_err());
    }

    #[test]
    fn binding_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let binding = ScorpioIdentityBinding {
            github_id: 42,
            github_login: "octocat".to_owned(),
            written_at: 1234567890,
        };
        write_binding(dir.path(), &binding).unwrap();
        let loaded = read_binding(dir.path()).unwrap();
        assert_eq!(loaded, binding);
    }

    #[test]
    fn binding_with_zero_id_rejected_on_read() {
        let dir = tempfile::tempdir().unwrap();
        let binding = ScorpioIdentityBinding {
            github_id: 0,
            github_login: "x".to_owned(),
            written_at: 0,
        };
        std::fs::write(
            dir.path().join("scorpio-identity.json"),
            serde_json::to_string(&binding).unwrap(),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                dir.path().join("scorpio-identity.json"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        let err = read_binding(dir.path()).unwrap_err();
        assert!(err.to_string().contains("github_id"));
    }

    #[test]
    fn verify_copilot_secret_file_secure_rejects_group_or_world_readable_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access-token");
        std::fs::write(&path, "ghu_test_token").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            let err = verify_copilot_secret_file_secure(&path).unwrap_err();
            assert!(
                err.to_string().contains("permissions"),
                "expected permission rejection, got: {err}"
            );
        }
    }

    #[test]
    fn verify_copilot_secret_file_secure_rejects_owner_executable_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access-token");
        std::fs::write(&path, "ghu_test_token").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            let err = verify_copilot_secret_file_secure(&path).unwrap_err();
            assert!(
                err.to_string().contains("permissions"),
                "expected permission rejection, got: {err}"
            );
        }
    }
}
