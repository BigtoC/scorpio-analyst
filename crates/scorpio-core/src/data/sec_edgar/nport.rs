//! SEC N-PORT-P parser.
//!
//! Parses the XBRL "primary document" of an N-PORT-P filing into the
//! [`NPortHoldings`] shape. Fail-soft: returns `None` for any
//! schema-mismatch or partial-data condition.

use std::collections::HashMap;

use chrono::NaiveDate;
use quick_xml::events::Event;
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
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut holdings: Vec<NPortHoldingRow> = Vec::new();
    let mut sector_totals: HashMap<String, f64> = HashMap::new();
    let mut stated_benchmark: Option<String> = None;

    let mut current: Option<PartialHolding> = None;
    let mut current_text: Vec<u8> = Vec::new();

    loop {
        match reader.read_event() {
            Err(_) => return None,
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                let name = e.name().as_ref().to_vec();
                if name == b"invstOrSec" {
                    current = Some(PartialHolding::default());
                }
            }
            Ok(Event::Text(t)) => {
                // unescape() resolves XML entities (e.g. `&amp;` → `&`). Fall
                // back to the raw bytes on failure so partial data still
                // contributes (typically the entity won't appear in numeric
                // fields like pctVal/valUSD anyway).
                match t.unescape() {
                    Ok(decoded) => current_text = decoded.as_bytes().to_vec(),
                    Err(_) => current_text = t.into_inner().to_vec(),
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
                if name == b"benchmarkName" || name == b"indxName" {
                    let txt = String::from_utf8_lossy(&current_text).trim().to_owned();
                    if !txt.is_empty() {
                        stated_benchmark = Some(txt);
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
        holdings,
        sector_breakdown,
        stated_benchmark,
    })
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
