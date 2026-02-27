use anyhow::bail;
use chrono::{NaiveDate, Utc};
use rmcp::{
    Error as McpError, ServerHandler,
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    schemars, tool,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::rates::{EcbRateSource, RateSource};

const LOOKBACK_DAYS: i64 = 10;

#[derive(Clone, Default)]
pub struct ExchangeRateServer {
    rates: EcbRateSource,
}

impl ExchangeRateServer {
    /// Find the most recent available EUR/USD rate on or before `date`, looking
    /// back up to [`LOOKBACK_DAYS`] days to skip weekends and holidays.
    async fn get_rate(&self, date: &str) -> anyhow::Result<(NaiveDate, f64)> {
        let d = NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .map_err(|e| anyhow::anyhow!("Invalid date format '{date}': {e}"))?;

        if d > Utc::now().date_naive() {
            bail!("Exchange rates are not available for future date {date}");
        }

        for offset in 0..=LOOKBACK_DAYS {
            let candidate = d - chrono::Duration::days(offset);
            if let Some(rate) = RateSource::rate_for_day(&self.rates, candidate).await? {
                return Ok((candidate, rate));
            }
        }

        bail!(
            "No exchange rate data available for {date} or the preceding {LOOKBACK_DAYS} days"
        );
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetRateParams {
    /// Date in YYYY-MM-DD format (e.g. "2025-01-15")
    pub date: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConvertParams {
    /// Amount to convert
    pub amount: f64,
    /// Source currency: "EUR" or "USD"
    pub from: String,
    /// Target currency: "EUR" or "USD"
    pub to: String,
    /// Date in YYYY-MM-DD format (e.g. "2025-01-15")
    pub date: String,
}

#[tool(tool_box)]
impl ExchangeRateServer {
    #[tool(description = "Get the EUR/USD exchange rate from the European Central Bank for a specific date. Only use this when the rate itself is what is being asked for. To convert an amount between EUR and USD, use convert_currency instead — do not fetch the rate and compute the conversion yourself.")]
    async fn get_exchange_rate(
        &self,
        #[tool(aggr)] GetRateParams { date }: GetRateParams,
    ) -> Result<CallToolResult, McpError> {
        let (actual_date, rate) = self.get_rate(&date).await.map_err(|e| {
            McpError::internal_error(format!("Failed to fetch ECB rate: {e}"), None)
        })?;

        Ok(CallToolResult::success(vec![Content::text(format!(
            "EUR/USD rate on {actual_date}: 1 EUR = {rate} USD (source: ECB)"
        ))]))
    }

    #[tool(description = "Convert an amount between EUR and USD using the ECB exchange rate for a specific date. Always use this tool when the user wants to convert a sum of money — never call get_exchange_rate and compute the result yourself.")]
    async fn convert_currency(
        &self,
        #[tool(aggr)] ConvertParams { amount, from, to, date }: ConvertParams,
    ) -> Result<CallToolResult, McpError> {
        let (actual_date, rate) = self.get_rate(&date).await.map_err(|e| {
            McpError::internal_error(format!("Failed to fetch ECB rate: {e}"), None)
        })?;

        let from = from.to_uppercase();
        let to = to.to_uppercase();

        let result = match (from.as_str(), to.as_str()) {
            ("EUR", "USD") => amount * rate,
            ("USD", "EUR") => amount / rate,
            ("EUR", "EUR") | ("USD", "USD") => amount,
            _ => {
                return Err(McpError::invalid_params(
                    "Only EUR and USD are supported",
                    None,
                ));
            }
        };

        Ok(CallToolResult::success(vec![Content::text(format!(
            "{amount:.2} {from} = {result:.2} {to} (rate: 1 EUR = {rate:.4} USD, date: {actual_date})"
        ))]))
    }
}

#[tool(tool_box)]
impl ServerHandler for ExchangeRateServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "Provides EUR/USD exchange rates from the European Central Bank (ECB). \
                 If the requested date is a weekend or holiday, the most recent available \
                 rate is returned automatically. Use YYYY-MM-DD date format. \
                 When converting an amount between EUR and USD, always call convert_currency \
                 directly — never call get_exchange_rate and perform the arithmetic yourself."
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
        let err = server.get_rate("2099-01-01").await.unwrap_err();
        assert!(
            err.to_string().contains("future"),
            "unexpected error: {err}"
        );
    }
}
