use anyhow::{anyhow, Result};
use chrono::{Datelike, NaiveDate, Utc};
use chrono_tz::Europe::Berlin;
use moka::future::Cache;
use std::collections::HashMap;
use std::sync::Arc;

use crate::ecb;

/// A source that can return the EUR/X rate for a given currency and calendar day.
///
/// Returns `Ok(Some(rate))` for trading days, `Ok(None)` for non-trading days
/// (weekends, holidays), and `Err` if the underlying data could not be retrieved.
pub trait RateSource: Send + Sync {
    async fn rate_for_day(&self, date: NaiveDate, currency: &str) -> Result<Option<f64>>;
}

/// All trading-day rates for one (currency, year), keyed by date.
///
/// `Some(rate)` — ECB trading day.
/// `None`       — non-trading day (weekend / holiday) in the fetched range.
/// absent       — date not in the fetched range (only possible for today before
///               ECB publishes; the year entry is invalidated so the next request
///               re-fetches).
type YearRates = Arc<HashMap<NaiveDate, Option<f64>>>;

/// [`RateSource`] backed by the ECB data API with a lazy per-(currency, year)
/// in-memory cache.
///
/// `moka::future::Cache::try_get_with` ensures that concurrent requests for the
/// same (currency, year) issue exactly one HTTP fetch; all waiters share the result.
#[derive(Clone)]
pub struct EcbRateSource {
    cache: Cache<(String, i32), YearRates>,
    client: reqwest::Client,
}

impl Default for EcbRateSource {
    fn default() -> Self {
        Self {
            cache: Cache::new(u64::MAX),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("failed to build reqwest client"),
        }
    }
}

impl RateSource for EcbRateSource {
    /// Returns the EUR/X rate for `currency` on `date`, fetching the full
    /// calendar year on first access.  Concurrent calls for the same
    /// (currency, year) are deduplicated by `try_get_with` — only one HTTP
    /// request is issued regardless of how many tasks call simultaneously.
    async fn rate_for_day(&self, date: NaiveDate, currency: &str) -> Result<Option<f64>> {
        let currency = currency.to_uppercase();
        let year = date.year();
        let url = ecb::ecb_currency_url(&currency);
        let client = self.client.clone();
        let init_currency = currency.clone();

        let year_data = self
            .cache
            .try_get_with((currency.clone(), year), async move {
                let mut fetched = HashMap::new();
                ecb::fetch_year_into(year, &init_currency, &mut fetched, &url, &client).await?;
                Ok::<YearRates, anyhow::Error>(Arc::new(fetched))
            })
            .await
            .map_err(|e| anyhow!("{e}"))?;

        // If today's rate is absent (ECB hasn't published yet, ~15:00 CET),
        // invalidate the cached year so the next request re-fetches and picks
        // up the rate once it's available.
        let today = Utc::now().with_timezone(&Berlin).date_naive();
        if year == today.year() && !year_data.contains_key(&today) {
            self.cache.invalidate(&(currency, year)).await;
        }

        Ok(year_data.get(&date).copied().flatten())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::server::LOOKBACK_DAYS;

    /// Walk back up to LOOKBACK_DAYS to find the most recent trading day rate.
    async fn rate_for(source: &EcbRateSource, date: &str, currency: &str) -> (NaiveDate, f64) {
        let d = NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap();
        for offset in 0..=LOOKBACK_DAYS {
            let candidate = d - chrono::Duration::days(offset);
            if let Some(rate) = source.rate_for_day(candidate, currency).await.unwrap() {
                return (candidate, rate);
            }
        }
        panic!("no rate found for {date}");
    }

    #[tokio::test]
    async fn trading_day_jan_2() {
        let src = EcbRateSource::default();
        let (date, rate) = rate_for(&src, "2025-01-02", "USD").await;
        assert_eq!(date, NaiveDate::from_ymd_opt(2025, 1, 2).unwrap());
        assert_eq!(rate, 1.0321);
    }

    #[tokio::test]
    async fn trading_day_jan_3() {
        let src = EcbRateSource::default();
        let (date, rate) = rate_for(&src, "2025-01-03", "USD").await;
        assert_eq!(date, NaiveDate::from_ymd_opt(2025, 1, 3).unwrap());
        assert_eq!(rate, 1.0299);
    }

    #[tokio::test]
    async fn new_years_day_falls_back_to_dec_31() {
        let src = EcbRateSource::default();
        // Jan 1 is a holiday — should fall back to Dec 31 2024
        let (date, rate) = rate_for(&src, "2025-01-01", "USD").await;
        assert_eq!(date, NaiveDate::from_ymd_opt(2024, 12, 31).unwrap());
        assert_eq!(rate, 1.0389);
    }

    #[tokio::test]
    async fn saturday_falls_back_to_friday() {
        let src = EcbRateSource::default();
        // Jan 4 is Saturday — should fall back to Friday Jan 3
        let (date, rate) = rate_for(&src, "2025-01-04", "USD").await;
        assert_eq!(date, NaiveDate::from_ymd_opt(2025, 1, 3).unwrap());
        assert_eq!(rate, 1.0299);
    }

    #[tokio::test]
    async fn sunday_falls_back_to_friday() {
        let src = EcbRateSource::default();
        // Jan 5 is Sunday — should fall back to Friday Jan 3
        let (date, rate) = rate_for(&src, "2025-01-05", "USD").await;
        assert_eq!(date, NaiveDate::from_ymd_opt(2025, 1, 3).unwrap());
        assert_eq!(rate, 1.0299);
    }

    #[tokio::test]
    async fn non_trading_day_returns_none() {
        let src = EcbRateSource::default();
        // Jan 1 2025 is a holiday — rate_for_day should return None directly.
        let result = src
            .rate_for_day(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(), "USD")
            .await
            .unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn gbp_rate_is_fetched_independently() {
        let src = EcbRateSource::default();
        // GBP is a different currency — must be fetched and cached separately from USD.
        let (date, rate) = rate_for(&src, "2025-01-02", "GBP").await;
        assert_eq!(date, NaiveDate::from_ymd_opt(2025, 1, 2).unwrap());
        // ECB EUR/GBP rate on 2025-01-02 — just check it's a plausible value.
        assert!(rate > 0.5 && rate < 1.5, "unexpected GBP rate: {rate}");
    }
}
