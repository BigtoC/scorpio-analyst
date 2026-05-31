//! SEC N-PORT-P parser.
//!
//! Parses the XBRL "primary document" of an N-PORT-P filing into the
//! [`NPortHoldings`] shape. Fail-soft: returns `None` for any
//! schema-mismatch or partial-data condition.

use std::collections::HashMap;

use chrono::NaiveDate;
use quick_xml::events::{BytesRef, Event};
use quick_xml::reader::Reader;

use crate::data::sec_edgar_nport::{NPortHoldingRow, NPortHoldings, NPortSectorRow};

/// Try to parse an N-PORT-P primary XBRL document.
///
/// The N-PORT-P schema groups holdings under `<invstOrSec>` (invested
/// securities). Returns `None` when the input is empty, structurally
/// invalid, or contains no holdings.
pub fn parse_nport_p(xml: &str, filing_date: NaiveDate) -> Option<NPortHoldings> {
    if xml.trim().is_empty() {
        return None;
    }
    // Note: text is NOT trimmed at the reader level. quick-xml 0.38+ splits
    // entity references (e.g. `&amp;`) out of text nodes into separate
    // `GeneralRef` events, so an element's text can span multiple events that
    // we accumulate; reader-level trimming would clip whitespace adjacent to an
    // entity (e.g. "Procter & Gamble"). Every field read trims instead.
    let mut reader = Reader::from_str(xml);

    let mut holdings: Vec<NPortHoldingRow> = Vec::new();
    let mut sector_totals: HashMap<String, f64> = HashMap::new();
    let mut stated_benchmark: Option<String> = None;
    // `repPdDate` is the date the holdings are reported as-of (anchors staleness);
    // `repPdEnd` is the fiscal reporting-period end, which can be a quarter later.
    // Track them separately and prefer repPdDate so a both-present filing does not
    // understate holdings age based on document order.
    let mut rep_pd_date: Option<NaiveDate> = None;
    let mut rep_pd_end: Option<NaiveDate> = None;

    let mut current: Option<PartialHolding> = None;
    let mut current_text: Vec<u8> = Vec::new();

    loop {
        match reader.read_event() {
            Err(_) => return None,
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                // Reset the per-element text buffer. Because entity references
                // now arrive as separate `GeneralRef` events, an element's text
                // is accumulated across events from this Start to its End.
                current_text.clear();
                let name = e.name().as_ref().to_vec();
                if name == b"invstOrSec" {
                    current = Some(PartialHolding::default());
                }
            }
            Ok(Event::Text(t)) => {
                // `decode()` replaces `unescape()` (removed in quick-xml 0.38);
                // it only performs charset decoding. XML entities are delivered
                // separately as `GeneralRef` events (handled below). Append so
                // text split across events reassembles. Fall back to raw bytes
                // on a charset-decode error.
                match t.decode() {
                    Ok(decoded) => current_text.extend_from_slice(decoded.as_bytes()),
                    Err(_) => current_text.extend_from_slice(t.into_inner().as_ref()),
                }
            }
            Ok(Event::GeneralRef(r)) => {
                // Splice resolved entity/character references (e.g. `&amp;` in
                // "S&P 500 Index") back into the current element's text.
                if let Some(ch) = resolve_general_ref(&r) {
                    let mut buf = [0u8; 4];
                    current_text.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
            }
            Ok(Event::End(e)) => {
                let name = e.name().as_ref().to_vec();
                if let Some(partial) = current.as_mut() {
                    fill_field(partial, &name, &current_text, &mut sector_totals);
                }
                if name == b"invstOrSec"
                    && let Some(p) = current.take()
                    && let Some(row) = p.into_row()
                {
                    holdings.push(row);
                }
                if name == b"repPdDate" || name == b"repPdEnd" {
                    let txt = String::from_utf8_lossy(&current_text).trim().to_owned();
                    if let Ok(parsed) = NaiveDate::parse_from_str(&txt, "%Y-%m-%d") {
                        if name == b"repPdDate" {
                            rep_pd_date = Some(parsed);
                        } else {
                            rep_pd_end = Some(parsed);
                        }
                    }
                }
                if name == b"benchmarkName" || name == b"indxName" {
                    let txt = String::from_utf8_lossy(&current_text).trim().to_owned();
                    if let Some(normalized) = normalize_optional_benchmark(&txt) {
                        stated_benchmark = Some(normalized);
                    }
                }
            }
            _ => {}
        }
    }

    if holdings.is_empty() {
        return None;
    }

    // Recompute weight_pct from value_usd when weight_pct wasn't reported directly.
    let total_value: f64 = holdings.iter().filter_map(|h| h.value_usd).sum();
    if total_value > 0.0 {
        for h in holdings.iter_mut() {
            if h.weight_pct == 0.0
                && let Some(v) = h.value_usd
            {
                h.weight_pct = v / total_value * 100.0;
            }
        }
    }

    let sector_breakdown: Vec<NPortSectorRow> = sector_totals
        .into_iter()
        .map(|(sector, weight_pct)| NPortSectorRow { sector, weight_pct })
        .collect();

    Some(NPortHoldings {
        filing_date,
        report_date: rep_pd_date.or(rep_pd_end),
        holdings,
        sector_breakdown,
        stated_benchmark,
    })
}

/// Normalize an optional textual benchmark/index name, rejecting the common
/// "no value" placeholders SEC filings use. Returns `None` for empty or
/// `n/a`/`na`/`none`/`null` content; otherwise the trimmed name verbatim.
pub(crate) fn normalize_optional_benchmark(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "n/a" | "na" | "none" | "null" => None,
        _ => Some(trimmed.to_owned()),
    }
}

#[derive(Default)]
struct PartialHolding {
    name: Option<String>,
    cusip: Option<String>,
    ticker: Option<String>,
    weight_pct: f64,
    value_usd: Option<f64>,
}

impl PartialHolding {
    fn into_row(self) -> Option<NPortHoldingRow> {
        let name = self.name?;
        Some(NPortHoldingRow {
            cusip: self.cusip,
            ticker: self.ticker,
            name,
            weight_pct: self.weight_pct,
            value_usd: self.value_usd,
        })
    }
}

/// Resolve a quick-xml [`GeneralRef`](Event::GeneralRef) to its replacement
/// character. Handles numeric character references (`&#38;`, `&#x26;`) and the
/// five XML predefined entities. Unknown entities yield `None` and are dropped,
/// matching the prior fail-soft behavior.
pub(super) fn resolve_general_ref(r: &BytesRef) -> Option<char> {
    if let Ok(Some(ch)) = r.resolve_char_ref() {
        return Some(ch);
    }
    match r.decode().ok()?.as_ref() {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => None,
    }
}

fn fill_field(
    partial: &mut PartialHolding,
    tag: &[u8],
    text: &[u8],
    sector_totals: &mut HashMap<String, f64>,
) {
    let txt = String::from_utf8_lossy(text).trim().to_owned();
    match tag {
        b"name" => partial.name = Some(txt),
        b"cusip" => partial.cusip = Some(txt),
        b"pctVal" => {
            if let Ok(v) = txt.parse::<f64>() {
                partial.weight_pct = v;
            }
        }
        b"valUSD" => partial.value_usd = txt.parse::<f64>().ok(),
        b"issuerType" | b"industryGroup" => {
            let weight = partial.weight_pct;
            *sector_totals.entry(txt).or_insert(0.0) += weight;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_nport_p_returns_none_for_empty_input() {
        let result = parse_nport_p("", NaiveDate::from_ymd_opt(2026, 4, 30).unwrap());
        assert!(result.is_none());
    }

    #[test]
    fn parse_nport_p_extracts_report_date_from_rep_pd_date() {
        let xml = r#"
        <edgarSubmission>
          <formData><genInfo><repPdDate>2026-03-31</repPdDate></genInfo></formData>
          <invstOrSec><name>Apple Inc</name><pctVal>5.0</pctVal><issuerType>Technology</issuerType></invstOrSec>
        </edgarSubmission>
        "#;
        let result = parse_nport_p(xml, NaiveDate::from_ymd_opt(2026, 5, 28).unwrap())
            .expect("fixture should parse");
        assert_eq!(
            result.report_date,
            Some(NaiveDate::from_ymd_opt(2026, 3, 31).unwrap())
        );
    }

    #[test]
    fn parse_nport_p_normalizes_na_benchmark_name_to_none() {
        // N/A lives in a *read* tag (indxName) so this actually drives
        // normalize_optional_benchmark's placeholder rejection.
        let xml = r#"
        <edgarSubmission>
          <formData><genInfo><repPdEnd>2026-03-31</repPdEnd></genInfo></formData>
          <indxName>N/A</indxName>
          <invstOrSec><name>Apple Inc</name><pctVal>5.0</pctVal><issuerType>Technology</issuerType></invstOrSec>
        </edgarSubmission>
        "#;
        let result = parse_nport_p(xml, NaiveDate::from_ymd_opt(2026, 5, 28).unwrap())
            .expect("fixture should parse");
        assert!(
            result.stated_benchmark.is_none(),
            "an N/A index name must normalize to None"
        );
    }

    #[test]
    fn parse_nport_p_keeps_real_benchmark_name() {
        let xml = r#"
        <edgarSubmission>
          <benchmarkName>NYSE Semiconductor Index</benchmarkName>
          <invstOrSec><name>Apple Inc</name><pctVal>5.0</pctVal><issuerType>Technology</issuerType></invstOrSec>
        </edgarSubmission>
        "#;
        let result = parse_nport_p(xml, NaiveDate::from_ymd_opt(2026, 5, 28).unwrap())
            .expect("fixture should parse");
        assert_eq!(
            result.stated_benchmark.as_deref(),
            Some("NYSE Semiconductor Index")
        );
    }

    #[test]
    fn parse_nport_p_prefers_rep_pd_date_over_rep_pd_end() {
        // Both present, repPdEnd later in document order; repPdDate (the holdings
        // as-of date) must win so staleness is not understated by ordering.
        let xml = r#"
        <edgarSubmission>
          <formData><genInfo>
            <repPdDate>2026-03-31</repPdDate>
            <repPdEnd>2026-06-30</repPdEnd>
          </genInfo></formData>
          <invstOrSec><name>Apple Inc</name><pctVal>5.0</pctVal><issuerType>Technology</issuerType></invstOrSec>
        </edgarSubmission>
        "#;
        let result = parse_nport_p(xml, NaiveDate::from_ymd_opt(2026, 8, 1).unwrap())
            .expect("fixture should parse");
        assert_eq!(
            result.report_date,
            Some(NaiveDate::from_ymd_opt(2026, 3, 31).unwrap()),
            "repPdDate (holdings as-of) must be preferred over the later repPdEnd"
        );
    }

    #[test]
    fn parse_nport_p_returns_none_for_garbage_xml() {
        let result = parse_nport_p("not xml", NaiveDate::from_ymd_opt(2026, 4, 30).unwrap());
        assert!(result.is_none());
    }

    #[test]
    fn parse_nport_p_extracts_three_holdings_from_fixture() {
        let xml = include_str!("../../../tests/fixtures/nport/spy_2026_04_30_excerpt.xml");
        let result = parse_nport_p(xml, NaiveDate::from_ymd_opt(2026, 4, 30).unwrap())
            .expect("fixture should parse");
        assert_eq!(result.holdings.len(), 3);
        assert!(result.holdings.iter().any(|h| h.name.contains("APPLE")));
    }

    #[test]
    fn parse_nport_p_captures_stated_benchmark_from_fixture() {
        let xml = include_str!("../../../tests/fixtures/nport/spy_2026_04_30_excerpt.xml");
        let result = parse_nport_p(xml, NaiveDate::from_ymd_opt(2026, 4, 30).unwrap())
            .expect("fixture should parse");
        assert_eq!(result.stated_benchmark.as_deref(), Some("S&P 500 Index"));
    }

    #[test]
    fn parse_nport_p_aggregates_sector_breakdown() {
        let xml = include_str!("../../../tests/fixtures/nport/spy_2026_04_30_excerpt.xml");
        let result = parse_nport_p(xml, NaiveDate::from_ymd_opt(2026, 4, 30).unwrap())
            .expect("fixture should parse");
        // All three holdings share `Information Technology` issuerType.
        assert_eq!(result.sector_breakdown.len(), 1);
        let row = &result.sector_breakdown[0];
        assert_eq!(row.sector, "Information Technology");
        let expected = 7.21 + 6.85 + 5.92;
        assert!((row.weight_pct - expected).abs() < 1e-9);
    }
}
