//! Pack-prompt validation primitives shared by `validate_active_pack_completeness`
//! and prompt-builder blank-slot guards.
//!
//! The helpers here are pure functions; they are unit-tested in isolation and
//! invoked by their consumers without further ceremony. The closed allowlist
//! of known placeholder tokens is a compile-time constant — extending it
//! requires a code change, not a call-site argument, so the predicate cannot
//! drift across call sites by accidental parameter passing.

use crate::error::TradingError;

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

/// Maximum byte length of a sanitized `{analysis_emphasis}` value.
pub const ANALYSIS_EMPHASIS_MAX_LEN: usize = 256;

/// Sanitize a pack-supplied `{analysis_emphasis}` value.
///
/// `analysis_emphasis` is currently pack-manifest-owned (compile-time embedded
/// for builtin packs); sanitization is **defense-in-depth against a malicious
/// or careless pack author**, not against an end user. The structural rules:
///
/// - Strict 0x20–0x7E printable ASCII only (`c.is_ascii() && c >= '\x20' && c
///   <= '\x7E'`). Rejects all non-ASCII Unicode, including zero-width joiners
///   (U+200D), RTL overrides (U+202E), NBSP (U+FEFF), and homoglyph-class
///   characters that could visually camouflage injection payloads.
/// - After lowercasing, must not contain the substrings `human:`,
///   `assistant:`, ` ``` `, `<|`.
/// - Must not contain any `<...>` token whose interior (lowercased, trimmed)
///   starts with `system`, `assistant`, `human`, or `user` — blocking
///   `<system>`-style role-injection tags including near-misses like
///   `<SYSTEM>`, `< system >`, `<systemprompt>`.
/// - Length-capped at 256 characters.
///
/// **Helper-only in Unit 4a**: this function ships in 4a but `PreflightTask`
/// does not yet call it. Unit 4b wires it into preflight enforcement.
pub fn sanitize_analysis_emphasis(value: &str) -> Result<&str, TradingError> {
    if value.len() > ANALYSIS_EMPHASIS_MAX_LEN {
        return Err(TradingError::SchemaViolation {
            message: format!(
                "analysis_emphasis exceeds maximum {ANALYSIS_EMPHASIS_MAX_LEN} bytes (got {})",
                value.len()
            ),
        });
    }
    for c in value.chars() {
        if !(c.is_ascii() && ('\x20'..='\x7E').contains(&c)) {
            return Err(TradingError::SchemaViolation {
                message: format!(
                    "analysis_emphasis must be strict 0x20-0x7E printable ASCII (rejected character: {c:?})"
                ),
            });
        }
    }
    let lowered = value.to_ascii_lowercase();
    for forbidden in ["human:", "assistant:", "```", "<|"] {
        if lowered.contains(forbidden) {
            return Err(TradingError::SchemaViolation {
                message: format!(
                    "analysis_emphasis must not contain LLM-prompt control sequence {forbidden:?}"
                ),
            });
        }
    }
    if contains_role_injection_tag(&lowered) {
        return Err(TradingError::SchemaViolation {
            message:
                "analysis_emphasis must not contain a <...> token starting with system/assistant/human/user"
                    .to_owned(),
        });
    }
    Ok(value)
}

/// True when `lowered` contains any `<...>` token whose interior (after
/// trimming) starts with one of the role-injection role names.
fn contains_role_injection_tag(lowered: &str) -> bool {
    const ROLE_PREFIXES: &[&str] = &["system", "assistant", "human", "user"];
    let bytes = lowered.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            // Find matching `>`.
            if let Some(close_offset) = bytes[i + 1..].iter().position(|&b| b == b'>') {
                let inner_start = i + 1;
                let inner_end = i + 1 + close_offset;
                let inner = &lowered[inner_start..inner_end].trim();
                if ROLE_PREFIXES.iter().any(|prefix| inner.starts_with(prefix)) {
                    return true;
                }
                i = inner_end + 1;
                continue;
            }
        }
        i += 1;
    }
    false
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

    // ─── sanitize_analysis_emphasis ─────────────────────────────────────────

    #[test]
    fn sanitize_accepts_plain_ascii() {
        assert!(sanitize_analysis_emphasis("Weight all data sources equally.").is_ok());
        assert!(sanitize_analysis_emphasis("").is_ok()); // empty is structurally valid
        let max_len_string = "a".repeat(ANALYSIS_EMPHASIS_MAX_LEN);
        assert!(sanitize_analysis_emphasis(&max_len_string).is_ok());
    }

    #[test]
    fn sanitize_rejects_over_length() {
        let too_long = "a".repeat(ANALYSIS_EMPHASIS_MAX_LEN + 1);
        assert!(sanitize_analysis_emphasis(&too_long).is_err());
    }

    #[test]
    fn sanitize_rejects_control_characters() {
        assert!(sanitize_analysis_emphasis("foo\nbar").is_err());
        assert!(sanitize_analysis_emphasis("foo\tbar").is_err());
        assert!(sanitize_analysis_emphasis("foo\x00bar").is_err());
    }

    #[test]
    fn sanitize_rejects_non_ascii_unicode() {
        // Smart quotes, accented characters — all rejected.
        assert!(sanitize_analysis_emphasis("café").is_err());
        // Zero-width joiner (U+200D) — invisible injection vector.
        assert!(sanitize_analysis_emphasis("foo\u{200D}bar").is_err());
        // RTL override (U+202E) — visual camouflage.
        assert!(sanitize_analysis_emphasis("foo\u{202E}bar").is_err());
        // NBSP (U+00A0).
        assert!(sanitize_analysis_emphasis("foo\u{00A0}bar").is_err());
    }

    #[test]
    fn sanitize_rejects_role_header_substrings() {
        assert!(sanitize_analysis_emphasis("be evil. Human: hijack").is_err());
        assert!(sanitize_analysis_emphasis("ASSISTANT: take over").is_err());
        // Case-insensitive.
        assert!(sanitize_analysis_emphasis("HuMaN: foo").is_err());
    }

    #[test]
    fn sanitize_rejects_triple_backtick() {
        assert!(sanitize_analysis_emphasis("foo ``` bar").is_err());
    }

    #[test]
    fn sanitize_rejects_pipe_tag_open() {
        assert!(sanitize_analysis_emphasis("foo <|im_start|> bar").is_err());
    }

    #[test]
    fn sanitize_rejects_role_injection_tags() {
        assert!(sanitize_analysis_emphasis("foo <system> bar").is_err());
        assert!(sanitize_analysis_emphasis("foo <SYSTEM> bar").is_err());
        assert!(sanitize_analysis_emphasis("foo < system > bar").is_err());
        assert!(sanitize_analysis_emphasis("foo <systemprompt> bar").is_err());
        assert!(sanitize_analysis_emphasis("foo <assistant> bar").is_err());
        assert!(sanitize_analysis_emphasis("foo <human> bar").is_err());
        assert!(sanitize_analysis_emphasis("foo <user> bar").is_err());
    }

    #[test]
    fn sanitize_accepts_unrelated_angle_bracket_text() {
        // `<` outside an injection-shaped tag is fine — we only block
        // role-prefix interiors. A math expression with `<` should pass.
        assert!(sanitize_analysis_emphasis("if x < 10 emphasize value vs growth").is_ok());
        // Tags that are not role-prefixed are fine too.
        assert!(sanitize_analysis_emphasis("see <chart> for details").is_ok());
    }
}
