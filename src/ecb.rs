use anyhow::{anyhow, bail, Result};
use chrono::{NaiveDate, Utc};
use serde::Deserialize;
use std::collections::HashMap;

const ECB_BASE: &str =
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
pub async fn fetch_year_into(year: i32, rates: &mut HashMap<NaiveDate, Option<f64>>) -> Result<()> {
    let today = Utc::now().date_naive();

    let jan1 = NaiveDate::from_ymd_opt(year, 1, 1)
        .ok_or_else(|| anyhow!("Invalid year {year}"))?;

    if jan1 > today {
        bail!("No exchange rate data available for future year {year}");
    }

    let dec31 = NaiveDate::from_ymd_opt(year, 12, 31).expect("valid year-end date");
    let end = dec31.min(today);

    let url = format!("{ECB_BASE}?startPeriod={jan1}&endPeriod={end}&format=csvdata");

    let text = reqwest::get(&url)
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
        rates.insert(date, Some(record.obs_value));
        trading_days += 1;
    }

    if trading_days == 0 {
        bail!("No exchange rate data available for year {year}");
    }

    // Backfill every non-trading day in [jan1, end] with None.
    for day in jan1.iter_days().take_while(|d| *d <= end) {
        rates.entry(day).or_insert(None);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn non_trading_days_marked_none() {
        let mut rates = HashMap::new();
        fetch_year_into(2025, &mut rates).await.unwrap();

        // Jan 1 (holiday) and Jan 4–5 (weekend) must be explicitly None.
        assert_eq!(rates[&NaiveDate::from_ymd_opt(2025, 1, 1).unwrap()], None);
        assert_eq!(rates[&NaiveDate::from_ymd_opt(2025, 1, 4).unwrap()], None);
        assert_eq!(rates[&NaiveDate::from_ymd_opt(2025, 1, 5).unwrap()], None);

        // Jan 2 and Jan 3 are trading days — must be Some.
        assert!(rates[&NaiveDate::from_ymd_opt(2025, 1, 2).unwrap()].is_some());
        assert!(rates[&NaiveDate::from_ymd_opt(2025, 1, 3).unwrap()].is_some());
    }
}
