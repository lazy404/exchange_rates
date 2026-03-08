use anyhow::{anyhow, bail, Result};
use chrono::{Datelike, NaiveDate, Utc};
use chrono_tz::Europe::Berlin;
use serde::Deserialize;
use std::collections::HashMap;

pub(crate) fn ecb_currency_url(currency: &str) -> String {
    format!("https://data-api.ecb.europa.eu/service/data/EXR/D.{currency}.EUR.SP00.A")
}

pub(crate) fn ecb_csv_url(currency: &str, date: NaiveDate) -> String {
    format!(
        "https://data-api.ecb.europa.eu/service/data/EXR/D.{currency}.EUR.SP00.A\
         ?startPeriod={date}&endPeriod={date}&format=csvdata"
    )
}

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

/// Fetch all trading-day rates for the given calendar year and currency from ECB
/// and merge them into `rates`.
///
/// Every calendar day in `[Jan 1, min(Dec 31, today)]` is written into the map:
/// - `Some(rate)` for days the ECB published a rate (trading days).
/// - `None` for all other days in that range (weekends, holidays).
///
/// Today is excluded from the backfill so it stays absent and triggers a fresh
/// fetch once ECB publishes the rate (~15:00 CET).
///
/// Returns an error immediately — without making any HTTP request — if `year`
/// is entirely in the future.
pub async fn fetch_year_into(
    year: i32,
    currency: &str,
    rates: &mut HashMap<(String, NaiveDate), Option<f64>>,
    base_url: &str,
    client: &reqwest::Client,
) -> Result<()> {
    let today = Utc::now().with_timezone(&Berlin).date_naive();

    let jan1 = NaiveDate::from_ymd_opt(year, 1, 1)
        .ok_or_else(|| anyhow!("Invalid year {year}"))?;

    if jan1 > today {
        bail!("No exchange rate data available for future year {year}");
    }

    let dec31 = NaiveDate::from_ymd_opt(year, 12, 31).expect("valid year-end date");
    let end = dec31.min(today);

    let url = format!("{base_url}?startPeriod={jan1}&endPeriod={end}&format=csvdata");

    let text = client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    if text.is_empty() {
        bail!("No exchange rate data available for {currency} in year {year}");
    }

    // Insert Some(rate) for every trading day first. If parsing fails partway
    // through, only correct Some values have been written — no None poisoning.
    let currency_upper = currency.to_uppercase();
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
        rates.insert((currency_upper.clone(), date), Some(record.obs_value));
    }

    // Backfill non-trading days in [jan1, end] with None, excluding only
    // today. Today stays absent so subsequent requests re-fetch once ECB
    // publishes the rate (~15:00 CET). All other days — including Dec 31 of
    // past years — are backfilled normally so they don't cause repeated fetches.
    for day in jan1.iter_days().take_while(|d| *d <= end && *d != today) {
        rates.entry((currency_upper.clone(), day)).or_insert(None);
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
        let client = reqwest::Client::new();
        fetch_year_into(today.year(), "USD", &mut rates, &server.uri(), &client)
            .await
            .unwrap();

        assert_ne!(
            rates.get(&("USD".to_string(), today)),
            Some(&None),
            "today ({today}) must not be cached as None — \
             a pre-publication fetch must leave today absent so the \
             next request re-fetches and picks up the published rate"
        );
    }

    #[tokio::test]
    async fn header_only_response_does_not_error() {
        use wiremock::matchers::any;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // A CSV with just the header and no data rows — exactly what the ECB
        // returns when the requested range contains only non-trading days
        // (e.g. fetching year N on Jan 1, when the only day in the range is a holiday).
        let csv = "KEY,FREQ,CURRENCY,CURRENCY_DENOM,EXR_TYPE,EXR_SUFFIX,TIME_PERIOD,OBS_VALUE\n";

        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_string(csv))
            .mount(&server)
            .await;

        let mut rates = HashMap::new();
        let client = reqwest::Client::new();
        // Must succeed, not bail with "no exchange rate data".
        fetch_year_into(2025, "USD", &mut rates, &server.uri(), &client)
            .await
            .unwrap();
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
        let client = reqwest::Client::new();
        let err = fetch_year_into(2025, "USD", &mut rates, &server.uri(), &client)
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
        let client = reqwest::Client::new();
        fetch_year_into(2023, "USD", &mut rates, &ecb_currency_url("USD"), &client)
            .await
            .unwrap();
        let dec31 = NaiveDate::from_ymd_opt(2023, 12, 31).unwrap();
        assert_eq!(
            rates.get(&("USD".to_string(), dec31)),
            Some(&None),
            "Dec 31 2023 (Sunday) must be cached as None, not left absent"
        );
    }

    #[tokio::test]
    async fn non_trading_days_marked_none() {
        let mut rates = HashMap::new();
        let client = reqwest::Client::new();
        fetch_year_into(2025, "USD", &mut rates, &ecb_currency_url("USD"), &client)
            .await
            .unwrap();

        let key = |m, d| ("USD".to_string(), NaiveDate::from_ymd_opt(2025, m, d).unwrap());

        // Jan 1 (holiday) and Jan 4–5 (weekend) must be explicitly None.
        assert_eq!(rates[&key(1, 1)], None);
        assert_eq!(rates[&key(1, 4)], None);
        assert_eq!(rates[&key(1, 5)], None);

        // Jan 2 and Jan 3 are trading days — must be Some.
        assert!(rates[&key(1, 2)].is_some());
        assert!(rates[&key(1, 3)].is_some());
    }
}
