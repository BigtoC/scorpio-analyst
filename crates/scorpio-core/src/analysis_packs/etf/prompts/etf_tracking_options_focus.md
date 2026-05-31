## ETF benchmark & dealer-positioning lens

In addition to standard technicals:

- **Official benchmark name** — when present, cite it as filed reference
  context. Do not infer a benchmark ticker from the textual name.

- **Dealer positioning (secondary baseline overlay)** — when `options_gex` is
  available in the prompt context, treat it as a **secondary overlay** on top
  of premium/discount and composition evidence. Do not cite
  `options_gex` fields from the technical prompt unless the implementation has
  explicitly threaded that derived payload into the prompt context. When only
  raw `options_context` / `options_summary` is available, discuss only the raw
  snapshot signals present there.

  When derived `options_gex` is available, cite present, decision-relevant
  signals. Do not force named absence callouts for every unavailable sub-signal:

  - **Near-term gamma exposure** — `options_gex.net_gex_usd_per_1pct_move`.
    Positive net means dealer hedging tends to dampen near-term moves;
    negative net means hedging tends to amplify them.
  - **Broad gamma exposure** — `options_gex.broad.net_gex_usd_per_1pct_move`
    when present. Explicitly label this as an all-expirations
    single-rate approximation when present.
    If `options_gex.broad.expirations_used <
    options_gex.broad.expirations_total_considered`, label the broad line as
    `Partial expirations` and mention both counts.
  - **Volatility sensitivity (VEX)** —
    `options_gex.vex_summary.net_vex_usd_per_volpt` when present, framed as a
    **conditional sensitivity to an absolute IV move**, not as a stand-alone
    stabilizing signal.
  - **Time-decay sensitivity (CEX)** —
    `options_gex.cex_summary.net_cex_usd_per_day` when present, framed as a
    **conditional sensitivity to one calendar day of decay**.
  - **Gamma walls** — `options_gex.strikes` (top dealer concentrations by
    `|net_gex|`) when present.
  - **Supporting evidence** — `options_gex.call_put_oi_ratio` and
    `options_gex.max_pain_strike` are **supporting**, not primary, evidence.
    Cite them only after the near-term GEX line.

- **Absence handling** — Stage 2 uses a single generic branch: if no usable
  derived dealer-positioning overlay is available in the prompt context, say
  dealer-positioning signals are unavailable for this run and keep the rest of
  the ETF analysis anchored on premium/discount and composition.
  Split no-snapshot vs unusable-snapshot copy only after adding an explicit
  derivation-status field.
