//! Minimal placeholder renderer.
//!
//! Matches the semantics of the existing
//! `template.replace("{ticker}", symbol).replace("{current_date}",
//! target_date)` pattern used across agent prompt builders today. Unknown
//! placeholders pass through unmodified so extracted baseline prompts stay
//! byte-identical when fed into this renderer with the same inputs.
use std::collections::HashMap;

/// Render `template` by substituting `{key}` occurrences with `vars[key]`.
///
/// Multiple occurrences of the same key are all substituted, consistent
/// with `str::replace`. Unknown placeholders pass through unchanged — the
/// current prompt pipeline relies on this behaviour for intra-prompt
/// curly-brace fragments that aren't meant to be substituted (e.g. JSON
/// schema examples).
#[must_use]
pub fn render(template: &str, vars: &HashMap<&str, &str>) -> String {
    let mut out = template.to_owned();
    for (key, value) in vars {
        // Allocate the `{key}` marker once per substitution; prompts are
        // kilobyte-scale so this isn't on any hot path.
        let marker = format!("{{{key}}}");
        out = out.replace(&marker, value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_ticker_placeholder() {
        let mut vars = HashMap::new();
        vars.insert("ticker", "AAPL");
        assert_eq!(render("Analyst for {ticker}", &vars), "Analyst for AAPL");
    }

    #[test]
    fn renders_current_date_placeholder() {
        let mut vars = HashMap::new();
        vars.insert("current_date", "2026-04-24");
        assert_eq!(render("as of {current_date}", &vars), "as of 2026-04-24");
    }

    #[test]
    fn unknown_placeholder_passes_through_unchanged() {
        let vars = HashMap::new();
        let template = "See {unexpected} marker";
        assert_eq!(render(template, &vars), "See {unexpected} marker");
    }

    #[test]
    fn multiple_occurrences_of_same_key_all_substituted() {
        let mut vars = HashMap::new();
        vars.insert("ticker", "NVDA");
        assert_eq!(
            render("{ticker} report for {ticker}", &vars),
            "NVDA report for NVDA"
        );
    }

    #[test]
    fn renders_analysis_emphasis_placeholder() {
        let mut vars = HashMap::new();
        vars.insert("analysis_emphasis", "focus on growth");
        assert_eq!(
            render("Emphasis: {analysis_emphasis}", &vars),
            "Emphasis: focus on growth"
        );
    }

    #[test]
    fn matches_legacy_chain_of_replace_calls() {
        // Byte-for-byte identity check: the new renderer with `{ticker}`
        // and `{current_date}` substitutions must produce the same output
        // as the inline `str::replace` chain currently used by
        // `build_fundamental_system_prompt`. If the renderer ever learns
        // fancier semantics (e.g. regex-style escaping) this guard fails.
        let template = "Analyst for {ticker} as of {current_date} — {unknown}";
        let legacy = template
            .replace("{ticker}", "AAPL")
            .replace("{current_date}", "2026-04-24");
        let mut vars = HashMap::new();
        vars.insert("ticker", "AAPL");
        vars.insert("current_date", "2026-04-24");
        assert_eq!(render(template, &vars), legacy);
    }
}
