use anyhow::Result;
use async_trait::async_trait;
use chrono::{Datelike, NaiveDate};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::ecb;

/// A source that can return the EUR/USD rate for a given calendar day.
///
/// Returns `Ok(Some(rate))` for trading days, `Ok(None)` for non-trading days
/// (weekends, holidays, future dates with no published rate), and `Err` if the
/// underlying data could not be retrieved.
#[async_trait]
pub trait RateSource: Send + Sync {
    async fn rate_for_day(&self, date: NaiveDate) -> Result<Option<f64>>;
}

/// Flat date → `Option<f64>` cache.
///
/// `Some(rate)` — ECB trading day with a known rate.
/// `None`       — day in a fetched year with no rate (weekend / holiday).
/// absent       — year containing this date has not been loaded yet.
type Cache = HashMap<NaiveDate, Option<f64>>;

/// [`RateSource`] implementation backed by the ECB data API with a lazy
/// per-year in-memory cache.
#[derive(Debug, Clone, Default)]
pub struct EcbRateSource {
    cache: Arc<Mutex<Cache>>,
}

#[async_trait]
impl RateSource for EcbRateSource {
    /// Returns the EUR/USD rate for `date`, or `None` if the ECB published no
    /// rate for that day.  Fetches the full calendar year on first access and
    /// caches it for subsequent lookups.
    async fn rate_for_day(&self, date: NaiveDate) -> Result<Option<f64>> {
        let mut cache = self.cache.lock().await;
        if !cache.contains_key(&date) {
            ecb::fetch_year_into(date.year(), &mut cache).await?;
        }
        Ok(*cache.get(&date).unwrap_or(&None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOOKBACK_DAYS: i64 = 10;

    /// Walk back up to LOOKBACK_DAYS to find the most recent trading day rate.
    async fn rate_for(source: &EcbRateSource, date: &str) -> (NaiveDate, f64) {
        let d = NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap();
        for offset in 0..=LOOKBACK_DAYS {
            let candidate = d - chrono::Duration::days(offset);
            if let Some(rate) = source.rate_for_day(candidate).await.unwrap() {
                return (candidate, rate);
            }
        }
        panic!("no rate found for {date}");
    }

    #[tokio::test]
    async fn trading_day_jan_2() {
        let src = EcbRateSource::default();
        let (date, rate) = rate_for(&src, "2025-01-02").await;
        assert_eq!(date, NaiveDate::from_ymd_opt(2025, 1, 2).unwrap());
        assert_eq!(rate, 1.0321);
    }

    #[tokio::test]
    async fn trading_day_jan_3() {
        let src = EcbRateSource::default();
        let (date, rate) = rate_for(&src, "2025-01-03").await;
        assert_eq!(date, NaiveDate::from_ymd_opt(2025, 1, 3).unwrap());
        assert_eq!(rate, 1.0299);
    }

    #[tokio::test]
    async fn new_years_day_falls_back_to_dec_31() {
        let src = EcbRateSource::default();
        // Jan 1 is a holiday — should fall back to Dec 31 2024
        let (date, rate) = rate_for(&src, "2025-01-01").await;
        assert_eq!(date, NaiveDate::from_ymd_opt(2024, 12, 31).unwrap());
        assert_eq!(rate, 1.0389);
    }

    #[tokio::test]
    async fn saturday_falls_back_to_friday() {
        let src = EcbRateSource::default();
        // Jan 4 is Saturday — should fall back to Friday Jan 3
        let (date, rate) = rate_for(&src, "2025-01-04").await;
        assert_eq!(date, NaiveDate::from_ymd_opt(2025, 1, 3).unwrap());
        assert_eq!(rate, 1.0299);
    }

    #[tokio::test]
    async fn sunday_falls_back_to_friday() {
        let src = EcbRateSource::default();
        // Jan 5 is Sunday — should fall back to Friday Jan 3
        let (date, rate) = rate_for(&src, "2025-01-05").await;
        assert_eq!(date, NaiveDate::from_ymd_opt(2025, 1, 3).unwrap());
        assert_eq!(rate, 1.0299);
    }

    #[tokio::test]
    async fn non_trading_day_returns_none() {
        let src = EcbRateSource::default();
        // Jan 1 2025 is a holiday — rate_for_day should return None directly.
        let result = src
            .rate_for_day(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap())
            .await
            .unwrap();
        assert_eq!(result, None);
    }
}
