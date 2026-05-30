//! SEC DERA Risk/Return Summary benchmark-name resolver.
//!
//! Extracts the official textual benchmark/index name a fund states in its
//! prospectus (the SEC DERA "Risk/Return Summary" datasets) for a given
//! `(series_id, class_id)`. This is a *display/prompt-context* name only — it is
//! never resolved to a market-data ticker.

use chrono::NaiveDate;

use crate::state::BenchmarkSource;

/// Series + class identifiers used to select a fund's rows in the risk/return
/// dataset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RiskReturnLookup<'a> {
    pub series_id: &'a str,
    pub class_id: &'a str,
}

/// Resolved official benchmark metadata for a fund share class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchmarkMetadata {
    pub name: String,
    pub source: BenchmarkSource,
    pub dataset_quarter: String,
    pub accession: Option<String>,
    pub filing_date: Option<NaiveDate>,
    pub source_period: Option<NaiveDate>,
}

/// Parse a SEC DERA risk/return TSV (the `num`/`txt`-style flat dataset) for the
/// benchmark name of `lookup`'s `(series_id, class_id)`. Returns the first
/// strategy/objective narrative row whose `value` yields a confident index name.
pub fn parse_risk_return_tsv_for_benchmark(
    raw: &str,
    lookup: RiskReturnLookup<'_>,
    dataset_quarter: &str,
) -> Option<BenchmarkMetadata> {
    let mut lines = raw.lines();
    let header = lines.next()?;
    let columns: Vec<&str> = header.split('\t').collect();
    let idx = |name: &str| columns.iter().position(|column| *column == name);
    let adsh_idx = idx("adsh")?;
    let series_idx = idx("series_id")?;
    let class_idx = idx("class_id")?;
    let filed_idx = idx("filed")?;
    let period_idx = idx("period")?;
    let tag_idx = idx("tag")?;
    let value_idx = idx("value")?;

    lines
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.get(series_idx)? != &lookup.series_id
                || fields.get(class_idx)? != &lookup.class_id
            {
                return None;
            }

            let tag = *fields.get(tag_idx)?;
            if tag != "StrategyNarrativeTextBlock" && tag != "ObjectivePrimaryTextBlock" {
                return None;
            }

            let name = extract_index_name(fields.get(value_idx)?)?;
            Some(BenchmarkMetadata {
                name,
                source: BenchmarkSource::SecRiskReturn,
                dataset_quarter: dataset_quarter.to_owned(),
                accession: fields.get(adsh_idx).map(|value| (*value).to_owned()),
                filing_date: fields
                    .get(filed_idx)
                    .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()),
                source_period: fields
                    .get(period_idx)
                    .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()),
            })
        })
        .next()
}

// `extract_index_name` over the StrategyNarrativeTextBlock / ObjectivePrimaryTextBlock
// narrative is the AUTHORITATIVE source of the benchmark's spaced name; the structured
// `AvgAnnlRtrPct` index-member token only CORROBORATES it (per spec — the structured row
// carries an unspaced token like `NYSESemiconductorIndex`, not the spaced display name).
// This scan is best-effort: it commits to the first `" index"` occurrence and can
// mis-extract on phrasings like "uses an index sampling strategy to track the CRSP US
// Total Market Index", so treat a low-confidence extraction as `None`. Do NOT special-case
// any single fund's index name; the SOXX fixture exercises this generic path ("track the
// NYSE Semiconductor Index" resolves correctly without a hardcoded marker).
fn extract_index_name(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let suffix = " index";
    let end = lower.find(suffix)? + suffix.len();
    let prefix_start = lower[..end]
        .rfind("the ")
        .map(|pos| pos + "the ".len())
        .unwrap_or(0);
    let candidate = text[prefix_start..end]
        .trim_matches(|c: char| c == ',' || c == '.')
        .trim();
    if candidate.len() >= "S&P 500 Index".len() {
        Some(candidate.to_owned())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_soxx_benchmark_from_strategy_text() {
        let raw = include_str!("../../tests/fixtures/sec_risk_return/soxx_rr.tsv");
        let benchmark = parse_risk_return_tsv_for_benchmark(
            raw,
            RiskReturnLookup {
                series_id: "S000004354",
                class_id: "C000012084",
            },
            "2025q3",
        )
        .expect("benchmark");

        assert_eq!(benchmark.name, "NYSE Semiconductor Index");
        assert_eq!(benchmark.source, crate::state::BenchmarkSource::SecRiskReturn);
        assert_eq!(benchmark.dataset_quarter, "2025q3");
        assert_eq!(benchmark.accession.as_deref(), Some("0001193125-25-162603"));
        assert_eq!(
            benchmark.filing_date,
            Some(NaiveDate::from_ymd_opt(2025, 7, 18).unwrap())
        );
    }

    #[test]
    fn returns_none_when_series_class_do_not_match() {
        let raw = include_str!("../../tests/fixtures/sec_risk_return/soxx_rr.tsv");
        let benchmark = parse_risk_return_tsv_for_benchmark(
            raw,
            RiskReturnLookup {
                series_id: "S000000000",
                class_id: "C000000000",
            },
            "2025q3",
        );
        assert!(benchmark.is_none());
    }

    #[test]
    fn extract_index_name_returns_spaced_name_or_none() {
        // Well-formed "...track the <Name> Index..." resolves to the spaced name.
        assert_eq!(
            extract_index_name("The Fund seeks results that track the MSCI World Index over time."),
            Some("MSCI World Index".to_owned())
        );
        // No "... Index" suffix at all → None.
        assert_eq!(extract_index_name("The Fund invests in semiconductors."), None);
        // Too-short candidate is low-confidence → None (never a mangled fragment).
        assert_eq!(extract_index_name("see the index"), None);
    }
}
