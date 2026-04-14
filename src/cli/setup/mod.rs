pub mod config_file;
pub mod steps;

use std::path::{Path, PathBuf};

use anyhow::Context;

use config_file::{
    PartialConfig, UserConfigFileError, load_user_config_at, save_user_config_at, user_config_path,
};
use steps::{
    step1_finnhub_api_key, step2_fred_api_key, step3_llm_provider_keys, step4_provider_routing,
    step5_health_check,
};

/// Map an `InquireError` cancellation signal into an `Ok(None)` early-exit,
/// or propagate any other error as `Err`.
///
/// Called by the orchestrator around each interactive step (1–4) so that ESC
/// and Ctrl-C produce `"Setup cancelled."` + `Ok(())` without going through
/// the error-propagation path for genuine I/O failures.
pub(crate) fn handle_cancellation<T>(
    result: Result<T, inquire::InquireError>,
) -> anyhow::Result<Option<T>> {
    match result {
        Ok(v) => Ok(Some(v)),
        Err(
            inquire::InquireError::OperationCanceled | inquire::InquireError::OperationInterrupted,
        ) => {
            println!("Setup cancelled.");
            Ok(None)
        }
        Err(e) => Err(anyhow::Error::from(e)),
    }
}

fn is_prompt_cancellation(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<inquire::InquireError>()
        .is_some_and(|ie| {
            matches!(
                ie,
                inquire::InquireError::OperationCanceled
                    | inquire::InquireError::OperationInterrupted
            )
        })
}

fn backup_path_for(path: &Path, timestamp: &str) -> anyhow::Result<PathBuf> {
    let file_name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("config path has no file name: {}", path.display()))?
        .to_string_lossy();

    Ok(path.with_file_name(format!("{file_name}.bak.{timestamp}")))
}

fn prompt_to_recover_malformed_config(
    path: &Path,
    _error: &UserConfigFileError,
) -> anyhow::Result<bool> {
    eprintln!("✗ Failed to parse existing config at {}", path.display());
    eprintln!("The file is not valid TOML and can be backed up before continuing.");

    inquire::Confirm::new("Move malformed config aside and start fresh?")
        .with_default(false)
        .prompt()
        .map_err(anyhow::Error::from)
}

fn load_or_recover_user_config_at<F, G>(
    path: &Path,
    mut should_recover: F,
    timestamp: G,
) -> anyhow::Result<Option<PartialConfig>>
where
    F: FnMut(&Path, &UserConfigFileError) -> anyhow::Result<bool>,
    G: FnOnce() -> String,
{
    match load_user_config_at(path) {
        Ok(config) => Ok(Some(config)),
        Err(error) => {
            let Some(file_error) = error.downcast_ref::<UserConfigFileError>() else {
                return Err(error);
            };

            match file_error {
                UserConfigFileError::Parse { .. } => {
                    if !should_recover(path, file_error)? {
                        return Ok(None);
                    }

                    let backup_path = backup_path_for(path, &timestamp())?;
                    std::fs::rename(path, &backup_path).with_context(|| {
                        format!(
                            "failed to move malformed config from {} to {}",
                            path.display(),
                            backup_path.display()
                        )
                    })?;

                    Ok(Some(PartialConfig::default()))
                }
                UserConfigFileError::Read { .. } => Err(error),
            }
        }
    }
}

/// Run the interactive setup wizard.
///
/// Loads any existing `~/.scorpio-analyst/config.toml` and walks the user
/// through five steps:
/// 1. Finnhub API key
/// 2. FRED API key
/// 3. LLM provider key(s)
/// 4. Provider routing (quick/deep model selection)
/// 5. LLM health check
///
/// ESC or Ctrl-C at any point cancels without saving.
pub fn run() -> anyhow::Result<()> {
    let config_path = user_config_path()?;
    let mut partial = match load_or_recover_user_config_at(
        &config_path,
        prompt_to_recover_malformed_config,
        || chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string(),
    ) {
        Ok(Some(config)) => config,
        Ok(None) => return Ok(()),
        Err(error) if is_prompt_cancellation(&error) => {
            println!("Setup cancelled.");
            return Ok(());
        }
        Err(error) => return Err(error),
    };

    // Steps 1–4: each is wrapped so ESC/Ctrl-C produce "Setup cancelled." + Ok(()).
    macro_rules! step {
        ($call:expr) => {
            if handle_cancellation($call)?.is_none() {
                return Ok(());
            }
        };
    }

    step!(step1_finnhub_api_key(&mut partial));
    step!(step2_fred_api_key(&mut partial));
    step!(step3_llm_provider_keys(&mut partial));
    step!(step4_provider_routing(&mut partial));

    // Step 5: health check — manages its own confirm prompt.
    let should_save = match step5_health_check(&partial) {
        Ok(v) => v,
        Err(e) => {
            // Ctrl-C on the "Save anyway?" confirm — treat as cancelled.
            if e.downcast_ref::<inquire::InquireError>().is_some_and(|ie| {
                matches!(
                    ie,
                    inquire::InquireError::OperationCanceled
                        | inquire::InquireError::OperationInterrupted
                )
            }) {
                println!("Setup cancelled.");
                return Ok(());
            }
            return Err(e);
        }
    };

    if !should_save {
        println!("Config not saved.");
        return Ok(());
    }

    save_user_config_at(&partial, &config_path)?;
    println!("✓ Config saved to {}", config_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn handle_cancellation_esc_prints_cancelled_and_returns_none() {
        let result: Result<(), _> = Err(inquire::InquireError::OperationCanceled);
        let outcome = handle_cancellation(result).expect("should not error");
        assert!(outcome.is_none(), "ESC should yield None (cancelled)");
    }

    #[test]
    fn handle_cancellation_ctrl_c_prints_cancelled_and_returns_none() {
        let result: Result<(), _> = Err(inquire::InquireError::OperationInterrupted);
        let outcome = handle_cancellation(result).expect("should not error");
        assert!(outcome.is_none(), "Ctrl-C should yield None (cancelled)");
    }

    #[test]
    fn handle_cancellation_success_returns_some_value() {
        let result: Result<i32, inquire::InquireError> = Ok(42);
        let outcome = handle_cancellation(result).expect("should not error");
        assert_eq!(outcome, Some(42));
    }

    #[test]
    fn handle_cancellation_unit_success_returns_some_unit() {
        let result: Result<(), inquire::InquireError> = Ok(());
        let outcome = handle_cancellation(result).expect("should not error");
        assert!(outcome.is_some());
    }

    #[test]
    fn load_or_recover_user_config_at_backs_up_invalid_file_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "not [ valid toml").unwrap();

        let loaded = load_or_recover_user_config_at(
            &path,
            |_path, _error| Ok(true),
            || "20260414T000000Z".to_owned(),
        )
        .expect("recovery should succeed")
        .expect("accepting recovery should continue with defaults");

        assert_eq!(loaded, PartialConfig::default());
        assert!(
            !path.exists(),
            "original malformed config should be moved aside"
        );
        assert!(
            dir.path().join("config.toml.bak.20260414T000000Z").exists(),
            "backup file should be created"
        );
    }

    #[test]
    fn load_or_recover_user_config_at_decline_keeps_invalid_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "not [ valid toml").unwrap();

        let loaded = load_or_recover_user_config_at(
            &path,
            |_path, _error| Ok(false),
            || "20260414T000000Z".to_owned(),
        )
        .expect("declining recovery should not error");

        assert!(loaded.is_none(), "declining recovery should stop setup");
        assert!(
            path.exists(),
            "original malformed config should remain in place"
        );
        assert!(
            !dir.path().join("config.toml.bak.20260414T000000Z").exists(),
            "no backup should be created when recovery is declined"
        );
    }

    #[test]
    fn prompt_to_recover_malformed_config_does_not_echo_secret_contents() {
        let path = Path::new("/tmp/config.toml");
        let parse_error = toml::from_str::<PartialConfig>("openai_api_key = \"sk-secret\n")
            .expect_err("fixture should be invalid toml");
        let error = UserConfigFileError::Parse {
            path: path.to_path_buf(),
            source: parse_error,
        };

        let rendered = format!("{error:#}");

        assert!(
            rendered.contains("/tmp/config.toml"),
            "parse error should still mention the path"
        );
        assert!(
            !rendered.contains("sk-secret"),
            "parse error rendering should not echo secret content: {rendered}"
        );
    }
}
