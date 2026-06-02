//! Pure account selection + snapshot assembly. No I/O.

use sha1::{Digest, Sha1};

use super::messages::{AccListItem, PositionListItem};
use super::{TRD_ENV_REAL, TRD_MARKET_US};
use crate::domain::Symbol;
use crate::state::{
    AccountPosition, AccountSnapshot, PositionSide, normalize_code, sanitize_label,
};

/// Map an analyzed symbol to its OpenD `TrdMarket`. v1: every equity → US.
/// HK/CN/futures are a clean extension here.
pub(crate) fn market_for_symbol(symbol: &Symbol) -> Result<i32, String> {
    match symbol {
        Symbol::Equity(_) => Ok(TRD_MARKET_US),
        Symbol::Crypto(_) => Err("account positions: crypto is not supported in v1".to_owned()),
    }
}

/// Resolve the `acc_id` to query. With `account` set, the chosen account must be
/// Real, authorized for `market`, and match `account` against its `uni_card_num`
/// (universal account number shown in the Futu app), `card_num`, or raw `acc_id`
/// (`uni_card_num` is sparse — only universal-system accounts carry it — so
/// `card_num`/`acc_id` keep securities accounts selectable). Otherwise the first
/// Real account authorized for `market` is used.
pub(crate) fn select_account(
    accounts: &[AccListItem],
    market: i32,
    account: Option<&str>,
) -> Result<u64, String> {
    if let Some(wanted) = account {
        let wanted = wanted.trim();
        return accounts
            .iter()
            .find(|a| {
                a.trd_env == TRD_ENV_REAL
                    && a.trd_market_auth_list.contains(&market)
                    && account_matches(a, wanted)
            })
            .map(|a| a.acc_id)
            .ok_or_else(|| "configured account is not available for this market".to_owned());
    }
    accounts
        .iter()
        .find(|a| a.trd_env == TRD_ENV_REAL && a.trd_market_auth_list.contains(&market))
        .map(|a| a.acc_id)
        .ok_or_else(|| format!("no real account for {}", trd_market_label(market)))
}

/// Whether the user-supplied selector matches this account by any of its
/// identifiers: universal account number, card number, or raw `acc_id`.
fn account_matches(acc: &AccListItem, wanted: &str) -> bool {
    acc.uni_card_num.as_deref() == Some(wanted)
        || acc.card_num.as_deref() == Some(wanted)
        || acc.acc_id.to_string() == wanted
}

/// Build the single-currency snapshot from raw position rows. `total` is
/// `Σ val`; currency is taken from the first row (uniform per market) or the
/// market default when there are no rows. The raw `acc_id` is redacted to a
/// hashed label and never stored.
pub(crate) fn assemble_snapshot(
    acc_id: u64,
    rows: Vec<PositionListItem>,
    market: i32,
) -> AccountSnapshot {
    let currency = rows
        .first()
        .map(|r| currency_label(r.currency))
        .unwrap_or_else(|| market_default_currency(market).to_owned());

    let mut total = 0.0;
    // Summing market value only makes sense within a single currency. The account
    // is market-matched, so rows are expected to share one currency; if OpenD ever
    // returns a mixed-currency list (e.g. a cross-listed name), drop the total
    // rather than report a meaningless sum under one currency label.
    let mut uniform_currency = true;
    let positions: Vec<AccountPosition> = rows
        .into_iter()
        .map(|r| {
            let row_currency = currency_label(r.currency);
            if row_currency != currency {
                uniform_currency = false;
            }
            total += r.val.unwrap_or(0.0);
            AccountPosition {
                code: normalize_code(&r.code),
                name: sanitize_label(&r.name),
                qty: r.qty,
                can_sell_qty: r.can_sell_qty,
                cost_price: r.cost_price,
                current_price: r.price,
                market_value: r.val,
                // OpenD returns plRatio as a percentage ("plRatio 8.8 ==> +8.8%",
                // confirmed against Trd_Common.proto + the Task 0 spike). The
                // domain stores pl_ratio as a fraction (0.088), so divide by 100.
                pl_ratio: r.pl_ratio.map(|p| p / 100.0),
                pl_val: r.pl_val,
                currency: row_currency,
                side: position_side(r.position_side),
            }
        })
        .collect();

    AccountSnapshot {
        account_label: Some(redact_account_id(acc_id)),
        market: trd_market_label(market).to_owned(),
        currency,
        total_market_value: uniform_currency.then_some(total),
        positions,
    }
}

fn position_side(value: i32) -> PositionSide {
    match value {
        1 => PositionSide::Short,
        _ => PositionSide::Long,
    }
}

/// `Currency` enum → label (`Trd_Common`).
fn currency_label(code: i32) -> String {
    match code {
        1 => "HKD",
        2 => "USD",
        3 => "CNH",
        4 => "JPY",
        5 => "SGD",
        6 => "AUD",
        _ => "UNKNOWN",
    }
    .to_owned()
}

fn market_default_currency(market: i32) -> &'static str {
    match market {
        TRD_MARKET_US => "USD",
        _ => "UNKNOWN",
    }
}

fn trd_market_label(market: i32) -> &'static str {
    match market {
        1 => "HK",
        TRD_MARKET_US => "US",
        3 => "CN",
        5 => "Futures",
        _ => "Unknown",
    }
}

/// Redacted short label for an account id (first 6 hex of SHA-1). Its purpose is
/// to keep the raw `accID` out of persisted state and reports — not to be a
/// cryptographic privacy boundary: account ids are a small enumerable space, so
/// the label is recoverable by brute force and must not be treated as anonymized.
fn redact_account_id(acc_id: u64) -> String {
    let mut hasher = Sha1::new();
    hasher.update(acc_id.to_le_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().take(3).map(|b| format!("{b:02x}")).collect();
    format!("acct-{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::futu::messages::{AccListItem, PositionListItem};
    use crate::data::futu::{TRD_ENV_REAL, TRD_MARKET_US};
    use crate::domain::Symbol;

    fn acc(acc_id: u64, trd_env: i32, markets: &[i32]) -> AccListItem {
        AccListItem {
            trd_env,
            acc_id,
            trd_market_auth_list: markets.to_vec(),
            uni_card_num: None,
            card_num: None,
        }
    }

    fn acc_with_nums(
        acc_id: u64,
        trd_env: i32,
        markets: &[i32],
        uni_card_num: Option<&str>,
        card_num: Option<&str>,
    ) -> AccListItem {
        AccListItem {
            trd_env,
            acc_id,
            trd_market_auth_list: markets.to_vec(),
            uni_card_num: uni_card_num.map(str::to_owned),
            card_num: card_num.map(str::to_owned),
        }
    }

    fn position(code: &str, val: f64, currency: i32) -> PositionListItem {
        PositionListItem {
            position_side: 0,
            code: code.to_owned(),
            name: code.to_owned(),
            qty: 100.0,
            can_sell_qty: 100.0,
            price: Some(185.42),
            cost_price: Some(150.0),
            val: Some(val),
            pl_val: Some(3542.0),
            pl_ratio: Some(23.6), // wire percentage
            currency,
        }
    }

    #[test]
    fn market_for_us_equity_is_trd_market_us() {
        let symbol = Symbol::parse("AAPL").unwrap();
        assert_eq!(market_for_symbol(&symbol).unwrap(), TRD_MARKET_US);
    }

    #[test]
    fn selects_first_real_account_authorized_for_market() {
        let accounts = vec![
            acc(1, 0, &[TRD_MARKET_US]),            // paper — skipped
            acc(2, TRD_ENV_REAL, &[1]),             // real but HK only — skipped
            acc(3, TRD_ENV_REAL, &[TRD_MARKET_US]), // match
            acc(4, TRD_ENV_REAL, &[TRD_MARKET_US]), // also matches; not chosen
        ];
        let chosen = select_account(&accounts, TRD_MARKET_US, None).unwrap();
        assert_eq!(chosen, 3);
    }

    #[test]
    fn account_override_selects_by_raw_acc_id() {
        let accounts = vec![
            acc(3, TRD_ENV_REAL, &[TRD_MARKET_US]),
            acc(9, TRD_ENV_REAL, &[TRD_MARKET_US]),
        ];
        let chosen = select_account(&accounts, TRD_MARKET_US, Some("9")).unwrap();
        assert_eq!(chosen, 9);
    }

    #[test]
    fn account_override_selects_by_uni_card_num() {
        // The US securities account has no uni_card_num; the futures-style account
        // does. Selecting by uni_card_num resolves to that account's acc_id.
        let accounts = vec![
            acc_with_nums(
                3,
                TRD_ENV_REAL,
                &[TRD_MARKET_US],
                None,
                Some("1001100580092142"),
            ),
            acc_with_nums(
                9,
                TRD_ENV_REAL,
                &[TRD_MARKET_US],
                Some("1001237387290123"),
                None,
            ),
        ];
        let chosen = select_account(&accounts, TRD_MARKET_US, Some("1001237387290123")).unwrap();
        assert_eq!(chosen, 9);
    }

    #[test]
    fn account_override_selects_by_card_num_for_account_without_uni_card_num() {
        // The real-world US case: no uni_card_num, only a card_num.
        let accounts = vec![acc_with_nums(
            7,
            TRD_ENV_REAL,
            &[TRD_MARKET_US],
            None,
            Some("1001100580092142"),
        )];
        let chosen = select_account(&accounts, TRD_MARKET_US, Some("1001100580092142")).unwrap();
        assert_eq!(chosen, 7);
    }

    #[test]
    fn account_override_trims_whitespace_before_matching() {
        let accounts = vec![acc_with_nums(
            7,
            TRD_ENV_REAL,
            &[TRD_MARKET_US],
            None,
            Some("123"),
        )];
        assert_eq!(
            select_account(&accounts, TRD_MARKET_US, Some("  123 ")).unwrap(),
            7
        );
    }

    #[test]
    fn account_override_that_is_not_real_is_unavailable() {
        let accounts = vec![acc(9, 0, &[TRD_MARKET_US])]; // paper
        assert!(select_account(&accounts, TRD_MARKET_US, Some("9")).is_err());
    }

    #[test]
    fn account_override_real_but_unauthorized_for_market_is_unavailable() {
        // Real account, but only authorized for HK (1), not the requested US (2).
        let accounts = vec![acc(9, TRD_ENV_REAL, &[1])];
        assert!(select_account(&accounts, TRD_MARKET_US, Some("9")).is_err());
    }

    #[test]
    fn account_override_no_identifier_matches_is_unavailable() {
        let accounts = vec![acc_with_nums(
            9,
            TRD_ENV_REAL,
            &[TRD_MARKET_US],
            Some("uni-1"),
            Some("card-1"),
        )];
        assert!(select_account(&accounts, TRD_MARKET_US, Some("nonexistent")).is_err());
    }

    #[test]
    fn no_matching_real_account_is_unavailable() {
        let accounts = vec![acc(1, 0, &[TRD_MARKET_US])]; // only paper
        let err = select_account(&accounts, TRD_MARKET_US, None).unwrap_err();
        assert!(err.contains("no real account"));
    }

    #[test]
    fn assemble_snapshot_computes_total_currency_and_redacts_account() {
        let rows = vec![
            position("US.AAPL", 18_542.0, 2),
            position("US.MSFT", 12_000.0, 2),
        ];
        let snap = assemble_snapshot(987654321, rows, TRD_MARKET_US);
        assert_eq!(snap.market, "US");
        assert_eq!(snap.currency, "USD");
        assert_eq!(snap.total_market_value, Some(30_542.0));
        assert_eq!(snap.positions.len(), 2);
        // account_label is a redacted hash, never the raw id.
        let label = snap.account_label.unwrap();
        assert!(
            !label.contains("987654321"),
            "label must be redacted: {label}"
        );
    }

    #[test]
    fn assemble_snapshot_with_zero_positions_is_empty_available() {
        let snap = assemble_snapshot(1, vec![], TRD_MARKET_US);
        assert!(snap.positions.is_empty());
        assert_eq!(snap.total_market_value, Some(0.0));
        assert_eq!(snap.currency, "USD"); // market default when no rows
    }

    #[test]
    fn assemble_maps_position_side_and_currency_labels() {
        let mut row = position("AAPL", 100.0, 2);
        row.position_side = 1; // Short
        let snap = assemble_snapshot(1, vec![row], TRD_MARKET_US);
        assert_eq!(snap.positions[0].side, crate::state::PositionSide::Short);
        assert_eq!(snap.positions[0].currency, "USD");
    }

    #[test]
    fn assemble_snapshot_drops_total_when_currencies_are_mixed() {
        // USD (2) + HKD (1) in one list — summing under a single label is wrong,
        // so total_market_value is dropped while per-row currencies are preserved.
        let rows = vec![
            position("US.AAPL", 18_542.0, 2),
            position("HK.0700", 12_000.0, 1),
        ];
        let snap = assemble_snapshot(1, rows, TRD_MARKET_US);
        assert_eq!(snap.total_market_value, None);
        assert_eq!(snap.positions[0].currency, "USD");
        assert_eq!(snap.positions[1].currency, "HKD");
    }

    #[test]
    fn assemble_snapshot_converts_plratio_percentage_to_fraction() {
        // Wire plRatio is a percentage (23.6 == +23.6%); the domain stores it as
        // a fraction (0.236) so the prompt/report ×100 render is correct.
        let mut row = position("AAPL", 1_000.0, 2);
        row.pl_ratio = Some(23.6);
        let snap = assemble_snapshot(1, vec![row], TRD_MARKET_US);
        let stored = snap.positions[0].pl_ratio.unwrap();
        assert!(
            (stored - 0.236).abs() < 1e-9,
            "expected fraction 0.236, got {stored}"
        );
    }
}
