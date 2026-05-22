## Output contract

Return ONLY a JSON object matching `TradeProposal`:
- `action`: one of `"Buy"`, `"Sell"`, `"Hold"` (these exact strings). The
  trader is restricted to this trio; the conviction-graded variants
  `"Overweight"` / `"Underweight"` are reserved for the fund manager.
- `target_price`: finite number.
- `stop_loss`: finite number.
- `confidence`: finite number, typically between 0.0 and 1.0.
- `rationale`: concise string explaining the thesis, the key supporting
  signals, and the main invalidation risk.
- `valuation_assessment`: brief string describing the valuation context
  driving the action. See the pack-specific guidance above for what to
  populate here.

If `action` is `Hold`, you must still provide numeric `target_price` and
`stop_loss` because the schema requires them — use them as monitoring
levels (`target_price` for confirmation/re-entry, `stop_loss` for
thesis-break risk).

Do not invent fields (entry windows, take-profit ladders, position size —
none are part of `TradeProposal`). Do not return prose outside the JSON
object. This proposal will be forwarded to the Risk Management Team; do
not make the final execution decision yourself.
