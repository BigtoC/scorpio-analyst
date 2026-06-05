use chrono::{DateTime, NaiveDate, TimeZone as _, Utc};
use chrono_tz::US::Eastern;

pub fn market_local_date_eastern_at(now: DateTime<Utc>) -> NaiveDate {
    Eastern.from_utc_datetime(&now.naive_utc()).date_naive()
}

pub(crate) fn target_is_market_local_date_at(target_date: &str, now: DateTime<Utc>) -> bool {
    target_date == market_local_date_eastern_at(now).to_string()
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone as _, Utc};

    #[test]
    fn market_local_date_uses_eastern_calendar_day() {
        let utc_evening = Utc.with_ymd_and_hms(2026, 5, 28, 3, 30, 0).unwrap();

        assert_eq!(
            super::market_local_date_eastern_at(utc_evening).to_string(),
            "2026-05-27"
        );
        assert!(super::target_is_market_local_date_at(
            "2026-05-27",
            utc_evening
        ));
        assert!(!super::target_is_market_local_date_at(
            "2026-05-28",
            utc_evening
        ));
    }
}
