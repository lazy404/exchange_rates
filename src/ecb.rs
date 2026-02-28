use anyhow::{anyhow, bail, Result};
use chrono::{Datelike, NaiveDate, Utc};
use chrono_tz::Europe::Berlin;
use serde::Deserialize;
use std::collections::HashMap;

pub(crate) const ECB_BASE: &str =
    "https://data-api.ecb.europa.eu/service/data/EXR/D.USD.EUR.SP00.A";

// ECB CSV response format (one row per trading day, non-trading days omitted):
//
// KEY,FREQ,CURRENCY,CURRENCY_DENOM,EXR_TYPE,EXR_SUFFIX,TIME_PERIOD,OBS_VALUE,...
// EXR.D.USD.EUR.SP00.A,D,USD,EUR,SP00,A,2024-12-30,1.0444,...
// EXR.D.USD.EUR.SP00.A,D,USD,EUR,SP00,A,2024-12-31,1.0389,...
// EXR.D.USD.EUR.SP00.A,D,USD,EUR,SP00,A,2025-01-02,1.0321,...
#[derive(Debug, Deserialize)]
struct EcbRecord {
    #[serde(rename = "TIME_PERIOD")]
    time_period: String,
    #[serde(rename = "OBS_VALUE")]
    obs_value: f64,
}

/// Fetch all trading-day rates for the given calendar year from ECB and merge
/// them into `rates`.
///
/// Every calendar day in `[Jan 1, min(Dec 31, today)]` is written into the map:
/// - `Some(rate)` for days the ECB published a rate (trading days).
/// - `None` for all other days in that range (weekends, holidays).
///
/// Future dates are never written, so they remain absent and will trigger a
/// fresh fetch once they are no longer in the future.
///
/// Returns an error immediately — without making any HTTP request — if `year`
/// is entirely in the future.
pub async fn fetch_year_into(year: i32, rates: &mut HashMap<NaiveDate, Option<f64>>, base_url: &str) -> Result<()> {
    let today = Utc::now().with_timezone(&Berlin).date_naive();

    let jan1 = NaiveDate::from_ymd_opt(year, 1, 1)
        .ok_or_else(|| anyhow!("Invalid year {year}"))?;

    if jan1 > today {
        bail!("No exchange rate data available for future year {year}");
    }

    let dec31 = NaiveDate::from_ymd_opt(year, 12, 31).expect("valid year-end date");
    let end = dec31.min(today);

    let url = format!("{base_url}?startPeriod={jan1}&endPeriod={end}&format=csvdata");

    let text = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    if text.is_empty() {
        bail!("No exchange rate data available for year {year}");
    }

    // Insert Some(rate) for every trading day first. If parsing fails partway
    // through, only correct Some values have been written — no None poisoning.
    let mut trading_days = 0usize;
    let mut reader = csv::Reader::from_reader(text.as_bytes());
    for result in reader.deserialize::<EcbRecord>() {
        let record = result?;
        let date = NaiveDate::parse_from_str(&record.time_period, "%Y-%m-%d")
            .map_err(|e| anyhow!("Invalid date '{}' from ECB: {e}", record.time_period))?;
        if date.year() != year {
            bail!(
                "ECB returned date '{}' outside requested year {year}",
                record.time_period
            );
        }
        if !record.obs_value.is_finite() || record.obs_value <= 0.0 {
            bail!("ECB returned invalid rate {} for {}", record.obs_value, record.time_period);
        }
        rates.insert(date, Some(record.obs_value));
        trading_days += 1;
    }

    if trading_days == 0 {
        bail!("No exchange rate data available for year {year}");
    }

    // Backfill non-trading days in [jan1, end] with None, excluding only
    // today. Today stays absent so subsequent requests re-fetch once ECB
    // publishes the rate (~15:00 CET). All other days — including Dec 31 of
    // past years — are backfilled normally so they don't cause repeated fetches.
    for day in jan1.iter_days().take_while(|d| *d <= end && *d != today) {
        rates.entry(day).or_insert(None);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn today_not_cached_as_none_before_ecb_publishes() {
        use chrono_tz::Europe::Berlin;
        use wiremock::matchers::any;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let today = Utc::now().with_timezone(&Berlin).date_naive();

        // Simulate an ECB response that contains no entry for today —
        // exactly as the real API behaves before ~15:00 CET on a trading day.
        // Use Jan 2-3 of the current year so the year-validation check passes.
        let year = today.year();
        let csv = format!(
            "KEY,FREQ,CURRENCY,CURRENCY_DENOM,EXR_TYPE,EXR_SUFFIX,TIME_PERIOD,OBS_VALUE\n\
             EXR.D.USD.EUR.SP00.A,D,USD,EUR,SP00,A,{year}-01-02,1.0321\n\
             EXR.D.USD.EUR.SP00.A,D,USD,EUR,SP00,A,{year}-01-03,1.0299\n"
        );

        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_string(csv))
            .mount(&server)
            .await;
        let mut rates = HashMap::new();
        fetch_year_into(today.year(), &mut rates, &server.uri())
            .await
            .unwrap();

        assert_ne!(
            rates.get(&today),
            Some(&None),
            "today ({today}) must not be cached as None — \
             a pre-publication fetch must leave today absent so the \
             next request re-fetches and picks up the published rate"
        );
    }

    #[tokio::test]
    async fn wrong_year_in_csv_is_rejected() {
        use wiremock::matchers::any;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // ECB response containing a date from the wrong year.
        let csv = "KEY,FREQ,CURRENCY,CURRENCY_DENOM,EXR_TYPE,EXR_SUFFIX,TIME_PERIOD,OBS_VALUE\n\
                   EXR.D.USD.EUR.SP00.A,D,USD,EUR,SP00,A,2024-12-31,1.0389\n";

        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_string(csv))
            .mount(&server)
            .await;

        let mut rates = HashMap::new();
        let err = fetch_year_into(2025, &mut rates, &server.uri())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("outside requested year"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn dec31_non_trading_day_is_backfilled() {
        // Dec 31 2023 was a Sunday — ECB published no rate.
        // It must be explicitly cached as None so requests for that date
        // don't trigger repeated full-year re-fetches.
        let mut rates = HashMap::new();
        fetch_year_into(2023, &mut rates, ECB_BASE).await.unwrap();
        let dec31 = NaiveDate::from_ymd_opt(2023, 12, 31).unwrap();
        assert_eq!(
            rates.get(&dec31),
            Some(&None),
            "Dec 31 2023 (Sunday) must be cached as None, not left absent"
        );
    }

    #[tokio::test]
    async fn non_trading_days_marked_none() {
        let mut rates = HashMap::new();
        fetch_year_into(2025, &mut rates, ECB_BASE).await.unwrap();

        // Jan 1 (holiday) and Jan 4–5 (weekend) must be explicitly None.
        assert_eq!(rates[&NaiveDate::from_ymd_opt(2025, 1, 1).unwrap()], None);
        assert_eq!(rates[&NaiveDate::from_ymd_opt(2025, 1, 4).unwrap()], None);
        assert_eq!(rates[&NaiveDate::from_ymd_opt(2025, 1, 5).unwrap()], None);

        // Jan 2 and Jan 3 are trading days — must be Some.
        assert!(rates[&NaiveDate::from_ymd_opt(2025, 1, 2).unwrap()].is_some());
        assert!(rates[&NaiveDate::from_ymd_opt(2025, 1, 3).unwrap()].is_some());
    }
}
