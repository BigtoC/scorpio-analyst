//! Pack-prompt validation primitives shared by `validate_active_pack_completeness`
//! and prompt-builder blank-slot guards.
//!
//! The helpers here are pure functions; they are unit-tested in isolation and
//! invoked by their consumers without further ceremony. The closed allowlist
//! of known placeholder tokens is a compile-time constant — extending it
//! requires a code change, not a call-site argument, so the predicate cannot
//! drift across call sites by accidental parameter passing.

/// Closed allowlist of placeholder tokens that may appear in pack-owned prompt
/// templates. `templating::render` expands exactly these three; any other
/// `{...}`-shaped token in a pack asset is treated as a developer typo
/// (e.g. `{ticker_symbol}`) and would render verbatim into the LLM prompt,
/// so `is_effectively_empty` deliberately does not strip unknown tokens.
const KNOWN_PLACEHOLDERS: &[&str] = &["{ticker}", "{current_date}", "{analysis_emphasis}"];

/// True when the slot has no meaningful content for the LLM prompt.
///
/// A slot is "effectively empty" when:
/// - it is `trim().is_empty()` after raw input — the obvious case; or
/// - after stripping every occurrence of the closed allowlist of known
///   placeholder tokens (`{ticker}`, `{current_date}`, `{analysis_emphasis}`),
///   the remainder is `trim().is_empty()` — which catches placeholder-only
///   slots like `"{ticker} {current_date}"` that would render to a degenerate
///   prompt (whitespace + concrete substitutions only, no instructional text).
///
/// Unknown placeholder tokens like `{ticker_symbol}` (typo) are *not* stripped
/// and therefore mark the slot as non-empty — surfacing typo-class developer
/// errors before they render verbatim to the LLM.
#[must_use]
pub fn is_effectively_empty(slot: &str) -> bool {
    if slot.trim().is_empty() {
        return true;
    }
    let mut stripped = slot.to_string();
    for token in KNOWN_PLACEHOLDERS {
        stripped = stripped.replace(token, "");
    }
    stripped.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_is_effectively_empty() {
        assert!(is_effectively_empty(""));
    }

    #[test]
    fn whitespace_only_is_effectively_empty() {
        assert!(is_effectively_empty("   "));
        assert!(is_effectively_empty("\t\n  "));
    }

    #[test]
    fn known_placeholder_only_is_effectively_empty() {
        assert!(is_effectively_empty("{ticker}"));
        assert!(is_effectively_empty("{current_date}"));
        assert!(is_effectively_empty("{analysis_emphasis}"));
    }

    #[test]
    fn multiple_known_placeholders_with_whitespace_is_effectively_empty() {
        assert!(is_effectively_empty("{ticker} {current_date}"));
        assert!(is_effectively_empty(
            "  {ticker}\n{current_date}\t{analysis_emphasis}  "
        ));
    }

    #[test]
    fn unknown_placeholder_is_not_effectively_empty() {
        // {ticker_symbol} is a typo, not a known token; it would render
        // verbatim into the LLM prompt, so it must mark the slot as non-empty.
        assert!(!is_effectively_empty("{ticker_symbol}"));
        assert!(!is_effectively_empty("{stock} {date}"));
    }

    #[test]
    fn slot_with_real_content_is_not_effectively_empty() {
        assert!(!is_effectively_empty("you are a fundamental analyst"));
    }

    #[test]
    fn slot_with_known_placeholder_and_real_content_is_not_effectively_empty() {
        assert!(!is_effectively_empty(
            "Analyze {ticker} on {current_date} carefully."
        ));
    }

    #[test]
    fn known_placeholders_are_exactly_three() {
        // Locking the closed allowlist size so any future addition forces
        // a deliberate code change to this constant + tests.
        assert_eq!(KNOWN_PLACEHOLDERS.len(), 3);
    }
}
