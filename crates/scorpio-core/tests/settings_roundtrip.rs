//! Integration test for the `scorpio_core::settings` cross-crate seam.
//!
//! Asserts that `PartialConfig`, `save_user_config_at`, `load_user_config_at`,
//! and `UserConfigFileError` remain reachable as public items and that the
//! save/load round-trip produces an equal value when consumed from outside the
//! crate (the CLI wires against this exact surface).

use scorpio_core::settings::{
    PartialConfig, UserConfigFileError, load_user_config_at, save_user_config_at,
};

#[test]
fn save_then_load_roundtrip_across_crate_boundary() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");

    let original = PartialConfig {
        finnhub_api_key: Some("fh-test".into()),
        openai_api_key: Some("sk-test".into()),
        quick_thinking_provider: Some("openai".into()),
        quick_thinking_model: Some("gpt-4o-mini".into()),
        ..Default::default()
    };

    save_user_config_at(&original, &path).expect("save");
    let loaded = load_user_config_at(&path).expect("load");

    assert_eq!(loaded, original);
}

#[test]
fn malformed_file_surface_user_config_file_error_parse_variant() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "not [ valid toml").expect("write fixture");

    let err = load_user_config_at(&path).expect_err("malformed TOML should error");
    let parse = err
        .downcast_ref::<UserConfigFileError>()
        .expect("CLI recovery downcasts through the re-exported type");
    assert!(
        matches!(parse, UserConfigFileError::Parse { .. }),
        "malformed TOML should map to UserConfigFileError::Parse; got {parse:?}"
    );
}
