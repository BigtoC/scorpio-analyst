use crate::error::TradingError;
/// Extract a JSON object from a raw LLM response that may contain markdown
/// code fences or explanatory prose around the JSON.
///
/// Tries three strategies in order:
/// 1. **Fast path** – the trimmed string already starts with `{` and ends with `}`.
/// 2. **Code fence** – the JSON is wrapped in `` ```json … ``` `` or `` ``` … ``` ``.
/// 3. **Brace fallback** – extract from the first `{` to the last `}`.
pub(crate) fn extract_json_object(context: &str, raw: &str) -> Result<String, TradingError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(TradingError::SchemaViolation {
            message: format!("{context}: LLM returned empty response"),
        });
    }
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Ok(trimmed.to_owned());
    }
    if let Some(extracted) = extract_from_code_fence(trimmed) {
        return Ok(extracted);
    }
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}'))
        && start < end
    {
        return Ok(trimmed[start..=end].to_owned());
    }
    Err(TradingError::SchemaViolation {
        message: format!("{context}: no JSON object found in LLM response"),
    })
}
/// Extract content from a markdown code fence (`` ```json `` or plain `` ``` ``).
pub(crate) fn extract_from_code_fence(text: &str) -> Option<String> {
    let mut inside_fence = false;
    let mut content_lines = Vec::new();
    for line in text.lines() {
        let stripped = line.trim();
        if !inside_fence {
            if let Some(after_ticks) = stripped.strip_prefix("```") {
                let after_ticks = after_ticks.trim();
                if after_ticks.is_empty() || after_ticks.eq_ignore_ascii_case("json") {
                    inside_fence = true;
                }
            }
        } else if stripped == "```" {
            let joined = content_lines.join("\n");
            let candidate = joined.trim();
            if candidate.starts_with('{') && candidate.ends_with('}') {
                return Some(candidate.to_owned());
            }
            inside_fence = false;
            content_lines.clear();
        } else {
            content_lines.push(line);
        }
    }
    None
}
