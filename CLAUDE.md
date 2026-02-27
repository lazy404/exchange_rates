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

# Run tests (hit the live ECB API — requires internet access)
cargo test

# Run a single test
cargo test <test_name>

# Lint
cargo clippy

# Format
cargo fmt
```

## Architecture

This is an **MCP (Model Context Protocol) server** that exposes EUR/USD exchange rates from the European Central Bank (ECB) as tools. It communicates over **stdio** using JSON-RPC — stdout is exclusively for MCP protocol messages, tracing logs go to stderr.

### Module structure

- **`src/main.rs`** — Entry point. Initializes tracing (stderr only), creates `ExchangeRateServer`, and serves it over stdio using the `rmcp` crate.
- **`src/server.rs`** — Defines `ExchangeRateServer` and its two MCP tools via the `#[tool]` macro from `rmcp`. Tool parameter structs derive `JsonSchema` (via `schemars`) so the MCP framework can auto-generate schemas. Contains the lookback logic (`LOOKBACK_DAYS = 10`) that walks backward from the requested date to skip weekends and holidays.
- **`src/rates.rs`** — Defines the `RateSource` trait and `EcbRateSource`, which implements a lazy per-year in-memory cache (`HashMap<NaiveDate, Option<f64>>`). `Some(rate)` = trading day, `None` = non-trading day in a fetched year, absent = year not yet loaded.
- **`src/ecb.rs`** — HTTP client. Fetches CSV data from the ECB API, parses it with the `csv` crate, and writes results into the cache via `fetch_year_into`. Backfills every non-trading day in the fetched year range with `None`.

### MCP tools exposed

| Tool | Description |
|------|-------------|
| `get_exchange_rate` | Returns the EUR/USD rate for a given date (YYYY-MM-DD) |
| `convert_currency` | Converts an amount between EUR and USD for a given date |

### Key constraints

- Rates are only available on **ECB business days**. When a date has no rate, `server.rs` walks back up to `LOOKBACK_DAYS` (10) days to find the most recent available rate.
- Only EUR↔USD conversions are supported.
- The ECB API endpoint is hardcoded in `ecb.rs`: `https://data-api.ecb.europa.eu/service/data/EXR/D.USD.EUR.SP00.A` — it fetches a full calendar year at a time with `?startPeriod=&endPeriod=&format=csvdata`.
- **Tests hit the live ECB API** — there are no mocks. All tests in `rates.rs` and `ecb.rs` require internet access.
