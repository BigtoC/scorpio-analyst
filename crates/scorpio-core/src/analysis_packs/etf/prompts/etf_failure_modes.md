## ETF failure modes to weigh

- **AP arbitrage breakdown** — when authorised-participant flow halts (high
  premiums/discounts persist), the wrapper decouples from NAV. Flag it
  whenever `premium_pct` magnitude exceeds the category band's "Extreme"
  threshold.
- **Composition staleness** — N-PORT-P filings have a 60-day legal lag.
  Phase 1 should qualify holdings with age bands: `fresh` (`<=45` days),
  `aging` (`46-90` days), `stale` (`>90` days). Even `aging` data must be
  qualified in plain language.
- **Tracking drift vs index** — non-trivial tracking error can come from
  sampling, securities-lending offsets, or fees. Treat persistent drift as
  a structural cost, not noise.
- **Leverage decay** — daily-reset leveraged/inverse products drift from
  their stated multiple over multi-day horizons. Holding-period risk.
- **Distribution mechanics** — large quarterly distributions reset NAV;
  reading the premium across a distribution date without adjustment is a
  false signal.
