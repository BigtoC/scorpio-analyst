## ETF failure modes to weigh

- **AP arbitrage breakdown** — when authorised-participant flow halts (high
  premiums/discounts persist), the wrapper decouples from NAV. Flag it
  whenever `premium_pct` magnitude exceeds the category band's "Extreme"
  threshold.
- **Composition staleness** — SEC N-PORT fallback holdings can have a legal
  filing lag; Alpha Vantage ETF_PROFILE snapshots do not carry regulatory report
  dates. Qualify holdings with source and age-band metadata when present.
- **Tracking drift vs index** — current runs leave tracking error unavailable
  unless verified benchmark daily history exists. Treat textual benchmark names
  as reference context only.
- **Leverage decay** — daily-reset leveraged/inverse products drift from
  their stated multiple over multi-day horizons. Holding-period risk.
- **Distribution mechanics** — large quarterly distributions reset NAV;
  reading the premium across a distribution date without adjustment is a
  false signal.
