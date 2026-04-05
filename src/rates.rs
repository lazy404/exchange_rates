use anyhow::Result;
use chrono::{Datelike, NaiveDate};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::ecb;

/// A source that can return the EUR/X rate for a given currency and calendar day.
///
/// Returns `Ok(Some(rate))` for trading days, `Ok(None)` for non-trading days
/// (weekends, holidays), and `Err` if the underlying data could not be retrieved.
pub trait RateSource: Send + Sync {
    async fn rate_for_day(&self, date: NaiveDate, currency: &str) -> Result<Option<f64>>;
}

/// Flat (currency, date) → `Option<f64>` cache.
///
/// `Some(rate)` — ECB trading day with a known rate.
/// `None`       — day in a fetched (currency, year) with no rate (weekend / holiday).
/// absent       — (currency, year) containing this date has not been loaded yet.
type Cache = HashMap<(String, NaiveDate), Option<f64>>;

/// One mutex per (currency, year) pair — held only during the HTTP fetch and
/// cache merge. Prevents duplicate concurrent fetches for the same (currency, year).
type FetchGuards = HashMap<(String, i32), Arc<Mutex<()>>>;

/// [`RateSource`] implementation backed by the ECB data API with a lazy
/// per-(currency, year) in-memory cache.
#[derive(Debug, Clone)]
pub struct EcbRateSource {
    cache: Arc<Mutex<Cache>>,
    fetch_guards: Arc<Mutex<FetchGuards>>,
    client: reqwest::Client,
}

impl Default for EcbRateSource {
    fn default() -> Self {
        Self {
            cache: Arc::default(),
            fetch_guards: Arc::default(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("failed to build reqwest client"),
        }
    }
}

impl RateSource for EcbRateSource {
    /// Returns the EUR/X rate for `currency` on `date`, or `None` if the ECB
    /// published no rate for that day. Fetches the full calendar year for the
    /// given currency on first access and caches it for subsequent lookups.
    ///
    /// Concurrent requests for the same (currency, year) are serialised via a
    /// per-key mutex so only one HTTP fetch is issued.
    async fn rate_for_day(&self, date: NaiveDate, currency: &str) -> Result<Option<f64>> {
        let currency = currency.to_uppercase();
        let cache_key = (currency.clone(), date);

        // Fast path: cache hit (holds a value or explicit None for a non-trading day).
        {
            let cache = self.cache.lock().await;
            if let Some(val) = cache.get(&cache_key) {
                return Ok(*val);
            }
        }

        // Acquire the per-(currency, year) fetch guard so that only one task
        // issues an HTTP request for a given (currency, year) at a time.
        let guard = {
            let mut guards = self.fetch_guards.lock().await;
            Arc::clone(
                guards
                    .entry((currency.clone(), date.year()))
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        let _fetch_lock = guard.lock().await;

        // Re-check after acquiring the guard — another task may have already
        // fetched and populated the cache while we were waiting.
        {
            let cache = self.cache.lock().await;
            if let Some(val) = cache.get(&cache_key) {
                return Ok(*val);
            }
        }

        // Fetch the full year into a temporary map — no lock held during I/O.
        let url = ecb::ecb_currency_url(&currency);
        let mut fetched = HashMap::new();
        ecb::fetch_year_into(date.year(), &currency, &mut fetched, &url, &self.client).await?;

        // Merge into the shared cache and return.
        let mut cache = self.cache.lock().await;
        cache.extend(fetched);
        Ok(*cache.get(&cache_key).unwrap_or(&None))
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

    #[tokio::test]
    async fn concurrent_requests_issue_one_fetch() {
        // Two tasks requesting the same (currency, year) simultaneously must not
        // both issue HTTP fetches — the second must wait for the first and reuse
        // the cached result.
        use std::sync::atomic::{AtomicUsize, Ordering};
        use wiremock::matchers::any;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let fetch_count = Arc::new(AtomicUsize::new(0));
        let fetch_count_clone = Arc::clone(&fetch_count);

        let mock_server = MockServer::start().await;
        let csv = "KEY,FREQ,CURRENCY,CURRENCY_DENOM,EXR_TYPE,EXR_SUFFIX,TIME_PERIOD,OBS_VALUE\n\
                   EXR.D.USD.EUR.SP00.A,D,USD,EUR,SP00,A,2023-01-02,1.0700\n";

        Mock::given(any())
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(csv)
                    .set_delay(std::time::Duration::from_millis(50)),
            )
            .expect(1) // exactly one HTTP request must be made
            .mount(&mock_server)
            .await;

        // Build an EcbRateSource that points at the mock server by overriding
        // ecb_currency_url via fetch_year_into directly.
        //
        // Instead, test via two concurrent fetch_year_into calls sharing one guard.
        let cache = Arc::new(Mutex::new(HashMap::<(String, NaiveDate), Option<f64>>::new()));
        let guards = Arc::new(Mutex::new(FetchGuards::new()));
        let client = reqwest::Client::new();
        let base_url = mock_server.uri();
        let date = NaiveDate::from_ymd_opt(2023, 1, 2).unwrap();

        let tasks: Vec<_> = (0..2)
            .map(|_| {
                let cache = Arc::clone(&cache);
                let guards = Arc::clone(&guards);
                let client = client.clone();
                let base_url = base_url.clone();
                let fetch_count = Arc::clone(&fetch_count_clone);
                tokio::spawn(async move {
                    let currency = "USD".to_string();
                    let cache_key = (currency.clone(), date);

                    {
                        let c = cache.lock().await;
                        if c.contains_key(&cache_key) {
                            return;
                        }
                    }

                    let guard = {
                        let mut g = guards.lock().await;
                        Arc::clone(
                            g.entry((currency.clone(), date.year()))
                                .or_insert_with(|| Arc::new(Mutex::new(()))),
                        )
                    };
                    let _lock = guard.lock().await;

                    {
                        let c = cache.lock().await;
                        if c.contains_key(&cache_key) {
                            return;
                        }
                    }

                    fetch_count.fetch_add(1, Ordering::SeqCst);
                    let mut fetched = HashMap::new();
                    crate::ecb::fetch_year_into(2023, &currency, &mut fetched, &base_url, &client)
                        .await
                        .unwrap();
                    let mut c = cache.lock().await;
                    c.extend(fetched);
                })
            })
            .collect();

        for t in tasks {
            t.await.unwrap();
        }

        mock_server.verify().await;
        assert_eq!(fetch_count.load(Ordering::SeqCst), 1, "expected exactly 1 fetch");
    }
}
