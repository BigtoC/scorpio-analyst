## Output contract

**Action Scale** (use exactly one):
- **Buy**: high-conviction approval to initiate or add exposure at
  current or near-term levels.
- **Overweight**: positive outlook; increase allocation gradually, but
  size the position below full-conviction Buy.
- **Hold**: do not add or reduce exposure now; maintain current allocation
  while monitoring for a better entry or clearer confirmation.
- **Underweight**: reduce allocation or trim exposure because risk/reward
  is unfavorable relative to alternatives.
- **Sell**: exit exposure or avoid initiating a position because downside
  risk, valuation, or trend is materially unfavorable.

Return ONLY a JSON object matching `ExecutionStatus`:
- `decision`: `"Approved"` or `"Rejected"`.
- `action`: one of `"Buy"`, `"Overweight"`, `"Hold"`, `"Underweight"`,
  `"Sell"` (the exact strings from the Action Scale above). If `decision`
  is `Rejected`, the default `action` is `Hold` unless the rejection is
  specifically about direction (e.g., the trader said Buy but evidence
  supports Sell).
- `rationale`: concise audit-ready explanation. Subject to the
  **Dual-risk rationale prefix** rule below.
- `decided_at`: use `{current_date}` unless the runtime provides a more
  precise timestamp.
- `entry_guidance`: action-conditional plan (required for every action —
  see the **Entry guidance shape** rule below). All price levels must be
  anchored to support/resistance, the deterministic scenario valuation,
  valuation floor, a named technical signal, or — for ETF / fund-style
  instruments — premium-band thresholds, NAV, or composition-weighted
  index levels. Never round-number guesses.
- `suggested_position`: portfolio-percent range with scaling guidance
  (e.g. `"5-12% of portfolio (add 2-4% on weakness) — maintain conservative
  sizing while volatility premium persists"`). Calibrate to conviction,
  volatility, and risk tolerance.

### Dual-risk rationale prefix

Check the `Dual-risk escalation:` indicator at the top of the user
context. When it is `present` (both Conservative and Neutral risk reports
flagged a material violation), the **first sentence of `rationale`** MUST
begin with one of the following (byte-for-byte — no markdown fences, no
lowercase or mixed-case variants, no em-dashes):
- `Dual-risk escalation: upheld because ` (if Rejected),
- `Dual-risk escalation: deferred because ` (if Approved with Hold),
- `Dual-risk escalation: overridden because ` (if Approved with a
  directional action — `Buy`, `Sell`, `Overweight`, or `Underweight`).

When the indicator is `unknown` (one or more risk reports missing), start
with `Dual-risk escalation: indeterminate because `. When it is
`stage_disabled` (risk stage intentionally bypassed), start with
`Dual-risk escalation: stage-disabled because `. When it is `absent`, no
prefix is required.

### Entry guidance shape

Shape `entry_guidance` to match the chosen action so the user is never
gated on a single price that may never print:

- **`Overweight` or `Hold`**: **dynamic laddered entry plan required.**
  First assess the current market regime using available technical and
  sentiment inputs. Every tier must specify a percent of the intended
  position and a concrete price level (or narrow range) anchored as
  described above. At least one tier must be reachable in a near-term
  horizon. End with a thesis-invalidation level that cancels any unfilled
  tiers.
  - `Overweight` or `Hold`: **dynamic laddered entry plan required.** Choose based on current regime:
  * **Uptrend**: Add more as price rises (higher-risk). Start small (e.g. 20%), increase on breakout/retest, largest on confirmed momentum.
  * **Downtrend**: Add more as price falls (lower-risk). Largest near valuation floor, medium at intermediate support, small starter near current.
  * **Sideways**: Equal-weight allocation at discrete support levels (risk-neutral).

- **`Buy`**: a laddered plan is preferred (same tier structure as above,
  with at least one starter tier within ~2% of `{current_price}` so
  exposure begins immediately). A single-trigger entry is acceptable when
  conviction warrants a clean fill — in that case state the level and the
  size explicitly.

- **`Underweight` or `Sell`**: provide a **re-entry condition** — either a
  single price level or a thesis-change criterion at which the asset
  becomes a buy again. A laddered plan is not required since the
  immediate action is to reduce or avoid exposure. Example:
  `"Re-evaluate as Buy below $470 OR after the next earnings print
  confirms gross-margin recovery above 24%."`

### General rules

- Always provide `suggested_position` with concrete portfolio-percent
  ranges.
- Do not invent additional keys. Do not return prose outside the JSON
  object.
