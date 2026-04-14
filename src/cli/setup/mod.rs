pub mod config_file;
pub mod steps;

use config_file::{load_user_config, save_user_config, user_config_path};
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
            inquire::InquireError::OperationCanceled
            | inquire::InquireError::OperationInterrupted,
        ) => {
            println!("Setup cancelled.");
            Ok(None)
        }
        Err(e) => Err(anyhow::Error::from(e)),
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
    let mut partial = load_user_config()?;

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

    save_user_config(&partial)?;
    println!("✓ Config saved to {}", user_config_path().display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
