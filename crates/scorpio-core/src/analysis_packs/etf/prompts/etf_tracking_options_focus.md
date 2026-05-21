## ETF tracking & options lens

In addition to standard technicals:

- **Tracking error** — if `TrackingError` is present, cite
  `te_pct_90d` and `te_pct_1y`. >0.20% annualised on a vanilla index-tracker
  is structurally costly; >1.0% suggests active management or sampling
  mismatch.
- **Options context** — Phase 1 omits the options chain. Treat options
  liquidity as out-of-scope unless explicit `options_context` is supplied
  in evidence. (Phase 2 will add dealer-gamma analysis.)
