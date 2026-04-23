use std::path::{Path, PathBuf};

use anyhow::Context as _;

use crate::error::TradingError;

/// Resolve the SQLite database path.
///
/// If `db_path` is `Some`, basic validation is applied to reject clearly unsafe
/// or malformed inputs (empty paths, embedded null bytes, bare path-traversal
/// sequences). Otherwise the default `$HOME/.scorpio-analyst/phase_snapshots.db`
/// is returned.
pub(super) fn resolve_db_path(db_path: Option<&Path>) -> Result<PathBuf, TradingError> {
    if let Some(p) = db_path {
        let s = p.to_string_lossy();

        if s.is_empty() {
            return Err(TradingError::Config(anyhow::anyhow!(
                "snapshot db_path must not be empty"
            )));
        }

        if s.contains('\0') {
            return Err(TradingError::Config(anyhow::anyhow!(
                "snapshot db_path must not contain null bytes"
            )));
        }

        let all_traversal = p.components().all(|c| {
            matches!(
                c,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        });
        if all_traversal {
            return Err(TradingError::Config(anyhow::anyhow!(
                "snapshot db_path must not be a bare traversal path: {s}"
            )));
        }

        return Ok(p.to_path_buf());
    }

    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .with_context(|| {
            "HOME/USERPROFILE environment variable not set; cannot resolve default snapshot path"
        })
        .map_err(TradingError::Config)?;

    Ok(PathBuf::from(home)
        .join(".scorpio-analyst")
        .join("phase_snapshots.db"))
}
