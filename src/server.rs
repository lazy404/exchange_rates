use anyhow::bail;
use chrono::{NaiveDate, Utc};
use chrono_tz::Europe::Berlin;
use rmcp::{
    Error as McpError, ServerHandler,
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    schemars, tool,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::ecb;
use crate::rates::{EcbRateSource, RateSource};

pub(crate) const LOOKBACK_DAYS: i64 = 10;

const SUPPORTED_CURRENCIES: &[&str] = &[
    "EUR",
    "AUD", "BGN", "BRL", "CAD", "CHF", "CNY", "CZK", "DKK",
    "GBP", "HKD", "HUF", "IDR", "ILS", "INR", "ISK", "JPY",
    "KRW", "MXN", "MYR", "NOK", "NZD", "PHP", "PLN", "RON",
    "SEK", "SGD", "THB", "TRY", "USD", "ZAR",
];

fn validate_currency(c: &str) -> Result<String, McpError> {
    let upper = c.to_uppercase();
    if SUPPORTED_CURRENCIES.contains(&upper.as_str()) {
        Ok(upper)
    } else {
        Err(McpError::invalid_params(
            format!("Unsupported currency '{c}'. Supported: {}", SUPPORTED_CURRENCIES.join(", ")),
            None,
        ))
    }
}

#[derive(Clone, Default)]
pub struct ExchangeRateServer {
    rates: EcbRateSource,
}

impl ExchangeRateServer {
    /// Find the most recent available EUR/X rate on or before `date`, looking
    /// back up to [`LOOKBACK_DAYS`] days to skip weekends and holidays.
    async fn get_rate(&self, date: &str, currency: &str) -> anyhow::Result<(NaiveDate, f64)> {
        let d = NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .map_err(|e| anyhow::anyhow!("Invalid date format '{date}': {e}"))?;

        if d > Utc::now().with_timezone(&Berlin).date_naive() {
            bail!("Exchange rates are not available for future date {date}");
        }

        for offset in 0..=LOOKBACK_DAYS {
            let candidate = d - chrono::Duration::days(offset);
            if let Some(rate) = RateSource::rate_for_day(&self.rates, candidate, currency).await? {
                return Ok((candidate, rate));
            }
        }

        bail!(
            "No exchange rate data available for {date} or the {LOOKBACK_DAYS} preceding days"
        );
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetRateParams {
    /// Date in YYYY-MM-DD format (e.g. "2025-01-15")
    pub date: String,
    /// ISO 4217 currency code to get the EUR rate for (e.g. "USD", "GBP", "JPY").
    /// Returns: 1 EUR = N {currency}. Supported currencies: AUD, BGN, BRL, CAD,
    /// CHF, CNY, CZK, DKK, GBP, HKD, HUF, IDR, ILS, INR, ISK, JPY, KRW, MXN,
    /// MYR, NOK, NZD, PHP, PLN, RON, SEK, SGD, THB, TRY, USD, ZAR.
    pub currency: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConvertParams {
    /// Amount to convert
    pub amount: f64,
    /// Source currency: "EUR" or any supported non-EUR currency
    pub from: String,
    /// Target currency: "EUR" or any supported non-EUR currency
    pub to: String,
    /// Date in YYYY-MM-DD format (e.g. "2025-01-15")
    pub date: String,
}

#[tool(tool_box)]
impl ExchangeRateServer {
    #[tool(description = "Get the EUR exchange rate for a specific currency and date from the European Central Bank. Returns '1 EUR = N {currency}'. Only use this when the rate itself is what is being asked for. To convert an amount, use convert_currency instead.")]
    async fn get_exchange_rate(
        &self,
        #[tool(aggr)] GetRateParams { date, currency }: GetRateParams,
    ) -> Result<CallToolResult, McpError> {
        let currency = validate_currency(&currency)?;
        if currency == "EUR" {
            return Err(McpError::invalid_params(
                "EUR is the base currency — get_exchange_rate requires a non-EUR currency. \
                 Use convert_currency if you want to convert amounts.",
                None,
            ));
        }
        let (actual_date, rate) = self.get_rate(&date, &currency).await.map_err(|e| {
            McpError::internal_error(format!("Failed to fetch ECB rate: {e}"), None)
        })?;

        let url = ecb::ecb_csv_url(&currency, actual_date);
        Ok(CallToolResult::success(vec![Content::text(format!(
            "EUR/{currency} rate on {actual_date}: 1 EUR = {rate} {currency} (source: {url})"
        ))]))
    }

    #[tool(description = "Convert an amount between EUR and another currency using the ECB exchange rate for a specific date. Exactly one of 'from' or 'to' must be EUR — EUR→EUR is not supported. Always use this tool when the user wants to convert a sum of money — never call get_exchange_rate and compute the result yourself.")]
    async fn convert_currency(
        &self,
        #[tool(aggr)] ConvertParams { amount, from, to, date }: ConvertParams,
    ) -> Result<CallToolResult, McpError> {
        let from = validate_currency(&from)?;
        let to = validate_currency(&to)?;

        // Same currency — no fetch needed.
        if from == to {
            if from == "EUR" {
                return Err(McpError::invalid_params(
                    "EUR is the base currency — converting EUR to EUR is not meaningful. \
                     Specify a non-EUR currency on one side.",
                    None,
                ));
            }
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "{amount:.2} {from} = {amount:.2} {to}"
            ))]));
        }

        // Determine the foreign (non-EUR) currency and the conversion direction.
        let (foreign, to_eur) = match (from.as_str(), to.as_str()) {
            ("EUR", _) => (&to, false),
            (_, "EUR") => (&from, true),
            _ => {
                return Err(McpError::invalid_params(
                    "At least one of 'from' or 'to' must be EUR",
                    None,
                ));
            }
        };

        let (actual_date, rate) = self.get_rate(&date, foreign).await.map_err(|e| {
            McpError::internal_error(format!("Failed to fetch ECB rate: {e}"), None)
        })?;

        let result = if to_eur { amount / rate } else { amount * rate };

        let url = ecb::ecb_csv_url(foreign, actual_date);
        Ok(CallToolResult::success(vec![Content::text(format!(
            "{amount:.2} {from} = {result:.2} {to} (rate: 1 EUR = {rate:.4} {foreign}, date: {actual_date}, source: {url})"
        ))]))
    }
}

#[tool(tool_box)]
impl ServerHandler for ExchangeRateServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "Provides EUR exchange rates from the European Central Bank (ECB) for 30 \
                 currencies: AUD, BGN, BRL, CAD, CHF, CNY, CZK, DKK, GBP, HKD, HUF, IDR, \
                 ILS, INR, ISK, JPY, KRW, MXN, MYR, NOK, NZD, PHP, PLN, RON, SEK, SGD, \
                 THB, TRY, USD, ZAR. If the requested date is a weekend or holiday, the \
                 most recent available rate is returned automatically. Use YYYY-MM-DD date \
                 format. Conversions must involve EUR on exactly one side (EUR→EUR is not \
                 supported). When converting an amount, \
                 always call convert_currency directly — never call get_exchange_rate and \
                 perform the arithmetic yourself."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .build(),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn future_date_is_rejected() {
        let server = ExchangeRateServer::default();
        let err = server.get_rate("2099-01-01", "USD").await.unwrap_err();
        assert!(
            err.to_string().contains("future"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn unsupported_currency_is_rejected() {
        let err = validate_currency("XYZ").unwrap_err();
        assert!(err.to_string().contains("XYZ"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn eur_is_rejected_in_get_exchange_rate() {
        let server = ExchangeRateServer::default();
        // EUR is the base currency — asking for "EUR/EUR" is meaningless.
        let params = GetRateParams {
            date: "2025-01-02".to_string(),
            currency: "EUR".to_string(),
        };
        let err = server.get_exchange_rate(params).await.unwrap_err();
        assert!(
            err.to_string().contains("base currency"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn convert_eur_to_usd_works() {
        let server = ExchangeRateServer::default();
        let params = ConvertParams {
            amount: 100.0,
            from: "EUR".to_string(),
            to: "USD".to_string(),
            date: "2025-01-02".to_string(),
        };
        let result = server.convert_currency(params).await.unwrap();
        let text = &result.content[0].as_text().unwrap().text;
        // EUR/USD on 2025-01-02 is 1.0321 → 100 EUR = 103.21 USD
        assert!(text.contains("100.00 EUR"), "unexpected result: {text}");
        assert!(text.contains("USD"), "unexpected result: {text}");
    }

    #[tokio::test]
    async fn convert_usd_to_eur_works() {
        let server = ExchangeRateServer::default();
        let params = ConvertParams {
            amount: 103.21,
            from: "USD".to_string(),
            to: "EUR".to_string(),
            date: "2025-01-02".to_string(),
        };
        let result = server.convert_currency(params).await.unwrap();
        let text = &result.content[0].as_text().unwrap().text;
        assert!(text.contains("103.21 USD"), "unexpected result: {text}");
        assert!(text.contains("EUR"), "unexpected result: {text}");
    }

    #[test]
    fn supported_currency_is_accepted() {
        assert_eq!(validate_currency("usd").unwrap(), "USD");
        assert_eq!(validate_currency("GBP").unwrap(), "GBP");
        assert_eq!(validate_currency("jpy").unwrap(), "JPY");
    }

    #[tokio::test]
    async fn all_supported_currencies_have_ecb_rates() {
        // Jan 2 2025 is a known ECB trading day. Verify every currency in
        // SUPPORTED_CURRENCIES returns a positive rate for that date.
        // A single EcbRateSource is shared so each year is fetched only once
        // per currency.
        let server = ExchangeRateServer::default();
        let mut failed = Vec::new();

        for &currency in SUPPORTED_CURRENCIES.iter().filter(|&&c| c != "EUR") {
            match server.get_rate("2025-01-02", currency).await {
                Ok((_, rate)) if rate > 0.0 => {}
                Ok((_, rate)) => failed.push(format!("{currency}: non-positive rate {rate}")),
                Err(e) => failed.push(format!("{currency}: {e}")),
            }
        }

        assert!(
            failed.is_empty(),
            "the following currencies failed:\n{}",
            failed.join("\n")
        );
    }
}
