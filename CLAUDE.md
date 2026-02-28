# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
# Build
cargo build

# Build release
cargo build --release

# Run (starts the MCP server on stdio)
cargo run

# Check (fast compile check without producing a binary)
cargo check

# Run tests (most hit the live ECB API — requires internet access)
cargo test

# Run a single test
cargo test <test_name>

# Lint
cargo clippy

# Format
cargo fmt
```

## Architecture

This is an **MCP (Model Context Protocol) server** that exposes EUR exchange rates from the European Central Bank (ECB) as tools. It communicates over **stdio** using JSON-RPC — stdout is exclusively for MCP protocol messages, tracing logs go to stderr.

### Module structure

- **`src/main.rs`** — Entry point. Initializes tracing (stderr only), creates `ExchangeRateServer`, and serves it over stdio using the `rmcp` crate.
- **`src/server.rs`** — Defines `ExchangeRateServer` and its two MCP tools via the `#[tool]` macro from `rmcp`. Tool parameter structs derive `JsonSchema` (via `schemars`) so the MCP framework can auto-generate schemas. Contains `SUPPORTED_CURRENCIES` (31 entries including EUR) and the lookback logic (`LOOKBACK_DAYS = 10`) that walks backward from the requested date to skip weekends and holidays.
- **`src/rates.rs`** — Defines the `RateSource` trait and `EcbRateSource`, which implements a lazy per-(currency, year) in-memory cache (`HashMap<(String, NaiveDate), Option<f64>>`). `Some(rate)` = trading day, `None` = non-trading day in a fetched (currency, year) pair, absent = not yet loaded.
- **`src/ecb.rs`** — HTTP client. Fetches CSV data from the ECB API via `ecb_currency_url(currency)`, parses it with the `csv` crate, and writes results into the cache via `fetch_year_into`. Backfills every non-trading day in the fetched year range with `None`, but excludes today so a pre-publication fetch doesn't permanently cache a missing rate. `ecb_csv_url(currency, date)` builds the single-day URL returned in tool responses.

### MCP tools exposed

| Tool | Description |
|------|-------------|
| `get_exchange_rate` | Returns the EUR/X rate for a given currency and date. EUR itself is rejected (it's the base). |
| `convert_currency` | Converts an amount between EUR and any supported currency for a given date. One of `from`/`to` must be EUR. |

### Key constraints

- **Supported currencies:** EUR plus 30 ECB non-EUR currencies (AUD, BGN, BRL, CAD, CHF, CNY, CZK, DKK, GBP, HKD, HUF, IDR, ILS, INR, ISK, JPY, KRW, MXN, MYR, NOK, NZD, PHP, PLN, RON, SEK, SGD, THB, TRY, USD, ZAR). Cross-rate conversions (e.g. GBP→JPY) are not supported — one side must be EUR.
- Rates are only available on **ECB business days**. When a date has no rate, `server.rs` walks back up to `LOOKBACK_DAYS` (10) days to find the most recent available rate.
- "Today" is always evaluated in **CET (Europe/Berlin)** timezone, in both the future-date check and the cache backfill exclusion.
- The ECB API URL is built per-currency by `ecb_currency_url()` in `ecb.rs`: `https://data-api.ecb.europa.eu/service/data/EXR/D.{CURRENCY}.EUR.SP00.A` — a full calendar year is fetched at a time with `?startPeriod=&endPeriod=&format=csvdata`.
- Both tools include a direct **ECB CSV URL** for the specific date used in the response (the actual date after any lookback), so callers can verify the raw source data.
- **Most tests hit the live ECB API** — `ecb.rs` has three wiremock-based tests that don't require internet; everything else does.
