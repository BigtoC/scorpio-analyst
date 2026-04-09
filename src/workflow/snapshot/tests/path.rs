use std::path::Path;

use super::in_memory_store;
use crate::workflow::snapshot::SnapshotStore;
use crate::workflow::snapshot::path::resolve_db_path;

#[test]
fn default_path_resolves_to_expected_location() {
    let path = resolve_db_path(None).expect("should resolve");
    let path_str = path.to_string_lossy();
    assert!(
        path_str.contains(".scorpio-analyst"),
        "expected .scorpio-analyst in path, got: {path_str}"
    );
    assert!(
        path_str.ends_with("phase_snapshots.db"),
        "expected phase_snapshots.db at end, got: {path_str}"
    );
}

#[test]
fn custom_path_overrides_default() {
    let custom = Path::new("/tmp/custom_test.db");
    let resolved = resolve_db_path(Some(custom)).expect("should resolve");
    assert_eq!(resolved, custom);
}

#[test]
fn empty_path_is_rejected() {
    let result = resolve_db_path(Some(Path::new("")));
    assert!(result.is_err(), "empty path should be rejected");
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("must not be empty"),
        "error should mention empty: {msg}"
    );
}

#[test]
fn null_byte_path_is_rejected() {
    let result = resolve_db_path(Some(Path::new("/tmp/bad\0.db")));
    assert!(result.is_err(), "null-byte path should be rejected");
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("null bytes"),
        "error should mention null bytes: {msg}"
    );
}

#[test]
fn bare_traversal_path_is_rejected() {
    let result = resolve_db_path(Some(Path::new("../../..")));
    assert!(result.is_err(), "bare traversal path should be rejected");
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("bare traversal"),
        "error should mention traversal: {msg}"
    );
}

#[test]
fn dot_only_path_is_rejected() {
    let result = resolve_db_path(Some(Path::new(".")));
    assert!(result.is_err(), "bare '.' path should be rejected");
}

#[test]
fn legitimate_path_with_parent_ref_is_accepted() {
    let p = Path::new("/tmp/foo/../bar.db");
    let resolved = resolve_db_path(Some(p)).expect("should resolve");
    assert_eq!(resolved, p);
}

#[tokio::test]
async fn parent_directory_is_created() {
    let dir = tempfile::tempdir().expect("temp dir");
    let nested = dir.path().join("nested").join("deep").join("snap.db");
    assert!(!nested.parent().unwrap().exists());

    SnapshotStore::new(Some(&nested))
        .await
        .expect("store should be created with auto-mkdir");

    assert!(nested.parent().unwrap().exists());
}

#[tokio::test]
async fn in_memory_store_smoke_test() {
    let _store = in_memory_store().await;
}
