//! Local SQLite cache for Alpha Vantage earnings-call transcripts.
//!
//! [`TranscriptCacheStore`] persists stable `TranscriptFetch::Found` results
//! so repeated analyses for the same symbol and quarter skip the external API
//! call entirely. Negative outcomes (`NotPublished`, `Throttled`,
//! `Unavailable`) bypass the cache and stay re-fetchable.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use sqlx::SqlitePool;
use tracing::warn;

use crate::data::adapters::transcripts::TranscriptFetch;
use crate::error::TradingError;

/// Local SQLite cache for transcript fetch results.
#[derive(Clone, Debug)]
pub struct TranscriptCacheStore {
    pool: SqlitePool,
}

impl TranscriptCacheStore {
    /// Open (or create) the transcript cache at the given path.
    ///
    /// If `db_path` is `None`, the default path
    /// `$HOME/.scorpio-analyst/transcript_cache.db` is used. The parent
    /// directory is created automatically if absent.
    pub async fn new(db_path: Option<&Path>) -> Result<Self, TradingError> {
        let resolved = resolve_db_path(db_path)?;

        if let Some(parent) = resolved.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create directory: {}", parent.display()))
                .map_err(TradingError::Config)?;
        }

        let db_url = format!("sqlite://{}?mode=rwc", resolved.display());
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    sqlx::query("PRAGMA journal_mode=WAL")
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query("PRAGMA busy_timeout = 5000")
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(&db_url)
            .await
            .with_context(|| format!("failed to open SQLite pool at {}", resolved.display()))
            .map_err(TradingError::Config)?;

        sqlx::migrate!("migrations/transcript_cache")
            .run(&pool)
            .await
            .map_err(|e| {
                TradingError::Config(anyhow::anyhow!("transcript cache migration: {e}"))
            })?;

        Ok(Self { pool })
    }

    /// Store a transcript fetch result in the cache.
    ///
    /// Only `TranscriptFetch::Found` results are cached. Negative outcomes
    /// (`NotPublished`, `Throttled`, `Unavailable`) are silently skipped.
    pub async fn put(
        &self,
        symbol: &str,
        quarter: &str,
        fetch: &TranscriptFetch,
    ) -> Result<(), TradingError> {
        validate_quarter_for_storage(quarter)?;

        if !matches!(fetch, TranscriptFetch::Found(_)) {
            return Ok(());
        }

        let normalized_symbol = symbol.to_ascii_uppercase();
        let payload_json = serde_json::to_string(fetch)
            .with_context(|| "failed to serialize transcript cache payload")
            .map_err(TradingError::Storage)?;

        sqlx::query(
            "INSERT INTO transcript_cache (symbol, quarter, payload_json)
             VALUES (?, ?, ?)
             ON CONFLICT(symbol, quarter) DO UPDATE SET
                 payload_json = excluded.payload_json,
                 cached_at = datetime('now')",
        )
        .bind(&normalized_symbol)
        .bind(quarter)
        .bind(&payload_json)
        .execute(&self.pool)
        .await
        .with_context(|| format!("failed to write transcript cache entry {symbol} {quarter}"))
        .map_err(TradingError::Storage)?;

        Ok(())
    }

    /// Retrieve a cached transcript fetch result.
    ///
    /// Returns `None` on cache miss, query error, or deserialization failure.
    /// Read-side failures emit sanitized `warn!` logs but do not propagate
    /// errors to the caller.
    pub async fn get(&self, symbol: &str, quarter: &str) -> Option<TranscriptFetch> {
        let normalized_symbol = symbol.to_ascii_uppercase();
        let row = match sqlx::query_as::<_, (String,)>(
            "SELECT payload_json FROM transcript_cache
             WHERE symbol = ? AND quarter = ?",
        )
        .bind(&normalized_symbol)
        .bind(quarter)
        .fetch_optional(&self.pool)
        .await
        {
            Ok(row) => row,
            Err(_err) => {
                warn!(
                    symbol,
                    quarter,
                    error.kind = "query",
                    "transcript cache read failed"
                );
                return None;
            }
        };

        let (payload_json,) = row?;

        match serde_json::from_str::<TranscriptFetch>(&payload_json) {
            Ok(fetch) => Some(fetch),
            Err(_err) => {
                warn!(
                    symbol,
                    quarter,
                    error.kind = "deserialize",
                    "transcript cache deserialize failed"
                );
                None
            }
        }
    }

    /// Close the underlying pool. Test-only seam for forcing cache failures.
    #[cfg(test)]
    #[expect(dead_code)]
    pub(crate) async fn close_for_test(&self) {
        self.pool.close().await;
    }
}

/// Validate the quarter format (`"YYYYQN"` where N is 1-4) at the storage
/// boundary. This makes `put("AAPL", "2025q1", ...)` fail fast even for
/// negative outcomes, instead of being silently skipped before the SQL
/// `CHECK` constraint is reached.
fn validate_quarter_for_storage(quarter: &str) -> Result<(), TradingError> {
    let b = quarter.as_bytes();
    let ok = b.len() == 6
        && b[0..4].iter().all(|c| c.is_ascii_digit())
        && b[4] == b'Q'
        && matches!(b[5], b'1'..=b'4');
    if !ok {
        return Err(TradingError::SchemaViolation {
            message: format!("invalid quarter format (expected YYYYQN, N=1..4): {quarter:?}"),
        });
    }
    Ok(())
}

/// Resolve the SQLite database path for the transcript cache.
///
/// Mirrors `SnapshotStore` path validation rules: non-empty, no null bytes,
/// no bare traversal, no dot-only paths.
fn resolve_db_path(db_path: Option<&Path>) -> Result<PathBuf, TradingError> {
    if let Some(p) = db_path {
        let s = p.to_string_lossy();

        if s.is_empty() {
            return Err(TradingError::Config(anyhow::anyhow!(
                "transcript cache db_path must not be empty"
            )));
        }

        if s.contains('\0') {
            return Err(TradingError::Config(anyhow::anyhow!(
                "transcript cache db_path must not contain null bytes"
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
                "transcript cache db_path must not be a bare traversal path: {s}"
            )));
        }

        return Ok(p.to_path_buf());
    }

    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .with_context(|| {
            "HOME/USERPROFILE environment variable not set; cannot resolve default transcript cache path"
        })
        .map_err(TradingError::Config)?;

    Ok(PathBuf::from(home)
        .join(".scorpio-analyst")
        .join("transcript_cache.db"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::adapters::transcripts::{TranscriptEvidence, TranscriptSegment};
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    // ── SharedLogBuffer for asserting on warn! output ─────────────────

    #[derive(Clone, Default)]
    struct SharedLogBuffer(Arc<Mutex<Vec<u8>>>);

    impl SharedLogBuffer {
        fn contents(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).expect("valid utf8 logs")
        }
    }

    struct SharedLogWriter(Arc<Mutex<Vec<u8>>>);

    impl<'a> MakeWriter<'a> for SharedLogBuffer {
        type Writer = SharedLogWriter;

        fn make_writer(&'a self) -> Self::Writer {
            SharedLogWriter(Arc::clone(&self.0))
        }
    }

    impl Write for SharedLogWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    // ── Test helpers ──────────────────────────────────────────────────

    async fn test_store() -> TranscriptCacheStore {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("transcript-cache.db");
        // Leak the TempDir for the lifetime of the test pool so the file
        // is not deleted while the connection is open. Tests are short-lived;
        // tempfile cleans up on process exit.
        std::mem::forget(dir);
        TranscriptCacheStore::new(Some(&path))
            .await
            .expect("store should open")
    }

    fn sample_found(symbol: &str, quarter: &str) -> TranscriptFetch {
        TranscriptFetch::Found(TranscriptEvidence {
            symbol: symbol.to_owned(),
            call_date: quarter.to_owned(),
            segments: vec![TranscriptSegment {
                speaker: "Tim Cook".to_owned(),
                title: "CEO".to_owned(),
                content: "Hello everyone.".to_owned(),
                sentiment: Some(0.5),
            }],
        })
    }

    fn sample_found_with_speaker(symbol: &str, quarter: &str, speaker: &str) -> TranscriptFetch {
        TranscriptFetch::Found(TranscriptEvidence {
            symbol: symbol.to_owned(),
            call_date: quarter.to_owned(),
            segments: vec![TranscriptSegment {
                speaker: speaker.to_owned(),
                title: "CFO".to_owned(),
                content: "Revenue grew.".to_owned(),
                sentiment: None,
            }],
        })
    }

    // ── Bootstrap ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn new_creates_transcript_cache_table() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("transcript-cache.db");

        let store = TranscriptCacheStore::new(Some(&path))
            .await
            .expect("store should open");

        assert!(store.get("AAPL", "2025Q1").await.is_none());
    }

    // ── Read/write ────────────────────────────────────────────────────

    #[tokio::test]
    async fn put_and_get_round_trip_found() {
        let store = test_store().await;
        let fetch = sample_found("AAPL", "2025Q1");

        store.put("AAPL", "2025Q1", &fetch).await.expect("put");

        assert_eq!(store.get("AAPL", "2025Q1").await, Some(fetch));
    }

    #[tokio::test]
    async fn put_skips_negative_outcomes() {
        let store = test_store().await;

        store
            .put("AAPL", "2025Q1", &TranscriptFetch::NotPublished)
            .await
            .expect("put");
        store
            .put("AAPL", "2025Q2", &TranscriptFetch::Throttled)
            .await
            .expect("put");
        store
            .put("AAPL", "2025Q3", &TranscriptFetch::Unavailable)
            .await
            .expect("put");

        assert!(store.get("AAPL", "2025Q1").await.is_none());
        assert!(store.get("AAPL", "2025Q2").await.is_none());
        assert!(store.get("AAPL", "2025Q3").await.is_none());
    }

    // ── Resilience ────────────────────────────────────────────────────

    #[tokio::test]
    async fn put_overwrites_existing_entry() {
        let store = test_store().await;

        let first = sample_found("AAPL", "2025Q1");
        store
            .put("AAPL", "2025Q1", &first)
            .await
            .expect("first put");

        let second = sample_found_with_speaker("AAPL", "2025Q1", "Luca Maestri");
        store
            .put("AAPL", "2025Q1", &second)
            .await
            .expect("second put");

        assert_eq!(store.get("AAPL", "2025Q1").await, Some(second));
    }

    #[test]
    fn get_logs_sanitized_warn_for_corrupt_payload() {
        let logs = SharedLogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .without_time()
            .with_writer(logs.clone())
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");

            runtime.block_on(async {
                let store = test_store().await;

                sqlx::query(
                    "INSERT INTO transcript_cache (symbol, quarter, payload_json)
                     VALUES (?, ?, ?)",
                )
                .bind("AAPL")
                .bind("2025Q1")
                .bind("not valid json")
                .execute(&store.pool)
                .await
                .expect("seed row");

                assert!(store.get("AAPL", "2025Q1").await.is_none());
            });
        });

        let output = logs.contents();
        assert!(output.contains("transcript cache deserialize failed"));
        assert!(!output.contains("not valid json"));
    }

    #[tokio::test]
    async fn put_rejects_invalid_quarter_at_storage_boundary() {
        let store = test_store().await;
        let err = store
            .put("AAPL", "2025q1", &sample_found("AAPL", "2025q1"))
            .await
            .expect_err("invalid quarter should fail");

        assert!(format!("{err:#}").contains("invalid quarter format"));
    }

    #[tokio::test]
    async fn two_store_instances_can_open_the_same_db_path() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("transcript-cache.db");

        let first = TranscriptCacheStore::new(Some(&path))
            .await
            .expect("first store");
        let second = TranscriptCacheStore::new(Some(&path))
            .await
            .expect("second store");

        first
            .put("AAPL", "2025Q1", &sample_found("AAPL", "2025Q1"))
            .await
            .expect("write");
        assert!(second.get("AAPL", "2025Q1").await.is_some());
    }
}
