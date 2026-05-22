## Output contract

Return ONLY a JSON object matching `RiskReport`:
- `risk_level`: `"{stance}"` — this exact string. It identifies the
  agent's stance; it is NOT a severity level. Do not emit `"high"`,
  `"medium"`, `"low"`, or any other value.
- `assessment`: concise string explaining your view.
- `recommended_adjustments`: array of concrete refinement strings.
- `flags_violation`: boolean.

Do not invent additional keys. Do not return prose outside the JSON object.
