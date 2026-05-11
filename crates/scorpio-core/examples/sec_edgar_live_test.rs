//! Live SEC EDGAR API smoke test.
//!
//! **NOT run automatically in CI** - `cargo nextest` does not execute `examples/`.
//! Run manually with:
//!
//! ```sh
//! cargo run -p scorpio-core --example sec_edgar_live_test
//! ```
//!
//! Uses the hardcoded Scorpio User-Agent. SEC EDGAR is unauthenticated — no API key.
//!
//! Covers:
//! - CIK lookup for a well-known ticker (AAPL)
//! - Filings fetch: happy path (AAPL → non-empty in a 24-month window)
//! - Fail-soft contract: bogus CIK → `Ok(empty)`, NOT an error
//! - Fail-soft contract: unknown ticker CIK lookup → `Ok(None)`

use scorpio_core::{data::SecEdgarClient, rate_limit::SharedRateLimiter};

struct Results {
    pass: usize,
    fail: usize,
}

impl Results {
    fn new() -> Self {
        Self { pass: 0, fail: 0 }
    }

    fn check(&mut self, label: &str, ok: bool) {
        if ok {
            println!("  PASS  {label}");
            self.pass += 1;
        } else {
            eprintln!("  FAIL  {label}");
            self.fail += 1;
        }
    }

    fn check_result(&mut self, label: &str, result: Result<(), String>) {
        match result {
            Ok(()) => self.check(label, true),
            Err(msg) => {
                eprintln!("  FAIL  {label}: {msg}");
                self.fail += 1;
            }
        }
    }
}

fn section(n: usize, title: &str) {
    println!("[{n}] {title}");
}

fn info(msg: &str) {
    println!("        {msg}");
}

#[tokio::main]
async fn main() {
    println!("─────────────────────────────────────────────────────────────────");
    println!("  SEC EDGAR live API smoke test");
    println!("─────────────────────────────────────────────────────────────────");
    println!("  User-Agent : Scorpio Analyst scorpio@ledgerlylab.com");
    println!("  No API key required — SEC EDGAR is public");
    println!("─────────────────────────────────────────────────────────────────");
    println!();

    // 5 req/sec: well within SEC EDGAR's 10 req/sec fair-use ceiling.
    let client = SecEdgarClient::new(SharedRateLimiter::new("sec-edgar-test", 5))
        .expect("hardcoded User-Agent should always construct successfully");

    let mut r = Results::new();

    // ── [1] CIK lookup — well-known ticker ────────────────────────────────────
    section(1, "lookup_cik — AAPL (well-known ticker)");
    match client.lookup_cik("AAPL").await {
        Err(e) => {
            eprintln!("  FAIL  lookup_cik returned error: {e}");
            r.fail += 1;
        }
        Ok(None) => {
            eprintln!("  FAIL  lookup_cik returned None for AAPL — CIK map fetch failed?");
            r.fail += 1;
        }
        Ok(Some(cik)) => {
            info(&format!("AAPL CIK = {cik}"));
            r.check_result(
                "AAPL CIK matches known value 320193",
                if cik == 320193 {
                    Ok(())
                } else {
                    Err(format!("expected 320193, got {cik}"))
                },
            );
        }
    }
    println!();

    // ── [2] CIK lookup — unknown ticker ──────────────────────────────────────
    section(2, "lookup_cik — ZZZNOTAREALTICKERZZZ (unknown ticker)");
    match client.lookup_cik("ZZZNOTAREALTICKERZZZ").await {
        Err(e) => {
            eprintln!("  FAIL  lookup_cik returned error for unknown ticker: {e}");
            r.fail += 1;
        }
        Ok(None) => {
            r.check("unknown ticker returns Ok(None)", true);
        }
        Ok(Some(cik)) => {
            eprintln!("  FAIL  unknown ticker unexpectedly returned CIK {cik}");
            r.fail += 1;
        }
    }
    println!();

    // ── [3] Filings fetch — happy path ────────────────────────────────────────
    section(3, "fetch_recent_filings — AAPL 8-K (2025–2026)");
    let filings = client
        .fetch_recent_filings(320193, &["8-K", "SC 13D", "SC 13G"], "2025-01-01", "2026-12-31")
        .await
        .expect("fetch_recent_filings must return Ok regardless of network outcome");

    info(&format!("{} filing(s) returned", filings.len()));
    for f in filings.iter().take(3) {
        info(&format!(
            "  {} | {} | items={:?}",
            f.filing_date, f.form_type, f.item_codes
        ));
    }
    r.check(
        "AAPL has 8-K filings in the 2025-2026 window",
        !filings.is_empty(),
    );
    if !filings.is_empty() {
        r.check_result(
            "all returned filings have a non-empty primary_doc_url",
            if filings.iter().all(|f| !f.primary_doc_url.is_empty()) {
                Ok(())
            } else {
                Err("one or more filings had an empty primary_doc_url".to_owned())
            },
        );
        r.check_result(
            "all returned filings have a non-empty filing_date",
            if filings.iter().all(|f| !f.filing_date.is_empty()) {
                Ok(())
            } else {
                Err("one or more filings had an empty filing_date".to_owned())
            },
        );
    }
    println!();

    // ── [4] Fail-soft: bogus CIK → Ok(empty) ─────────────────────────────────
    section(4, "fetch_recent_filings — bogus CIK (fail-soft contract)");
    let bogus_result = client
        .fetch_recent_filings(99_999_999, &["8-K"], "2025-01-01", "2026-12-31")
        .await;
    r.check_result(
        "bogus CIK returns Ok(empty), not Err",
        match bogus_result {
            Ok(ref v) if v.is_empty() => Ok(()),
            Ok(ref v) => Err(format!("expected empty, got {} filings", v.len())),
            Err(e) => Err(format!("expected Ok(empty), got Err: {e}")),
        },
    );
    println!();

    println!("─────────────────────────────────────────────────────────────────");
    println!("  Results: {} passed, {} failed", r.pass, r.fail);
    println!("─────────────────────────────────────────────────────────────────");

    if r.fail > 0 {
        std::process::exit(1);
    }
}
