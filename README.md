# exchange_rates

[![Rust](https://img.shields.io/badge/rust-2024_edition-orange?logo=rust)](https://www.rust-lang.org)
[![MCP](https://img.shields.io/badge/MCP-compatible-blue?logo=anthropic)](https://modelcontextprotocol.io)
[![Data source](https://img.shields.io/badge/data-ECB_Data_Portal-003591?logo=europeanunion&logoColor=white)](https://data-api.ecb.europa.eu)
[![Build](https://img.shields.io/badge/build-cargo-green?logo=rust)](https://doc.rust-lang.org/cargo/)
[![Release](https://img.shields.io/github/v/release/lazy404/exchange_rates)](https://github.com/lazy404/exchange_rates/releases)

An [MCP (Model Context Protocol)](https://modelcontextprotocol.io) server that provides EUR exchange rates sourced directly from the **European Central Bank (ECB)**. Connect it to Claude to look up historical rates and convert amounts between EUR and 30 other currencies by date.

## Tools

| Tool | Description |
|------|-------------|
| `get_exchange_rate` | Returns the ECB rate for a given EUR/X currency pair and date |
| `convert_currency` | Converts an amount between EUR and another currency using the ECB rate for a given date |

If the requested date falls on a weekend or public holiday, the server automatically returns the most recent available rate (looking back up to 10 days). Every response includes a direct link to the ECB CSV data for the date used, so the source can be verified.

## Supported currencies

AUD, BGN, BRL, CAD, CHF, CNY, CZK, DKK, GBP, HKD, HUF, IDR, ILS, INR, ISK, JPY, KRW, MXN, MYR, NOK, NZD, PHP, PLN, RON, SEK, SGD, THB, TRY, USD, ZAR

Conversions must involve EUR on one side (e.g. EUR→GBP or JPY→EUR). Cross-rate conversions (e.g. GBP→JPY) and EUR→EUR are not supported.

## Prerequisites

- [Rust](https://rustup.rs) (edition 2024, stable toolchain)

## Installation

### Download a release binary

Pre-built binaries for Linux x86_64, macOS arm64, and Windows x86_64 are attached to each [GitHub Release](https://github.com/lazy404/exchange_rates/releases). Download the binary for your platform and place it somewhere on your `PATH`.

### Build from source

```bash
git clone https://github.com/lazy404/exchange_rates.git
cd exchange_rates
cargo build --release
```

The binary will be at `target/release/exchange_rates`.

## Connecting to Claude

### Claude Desktop

1. Open (or create) the Claude Desktop configuration file:
   - **macOS:** `~/Library/Application Support/Claude/claude_desktop_config.json`
   - **Windows:** `%APPDATA%\Claude\claude_desktop_config.json`

2. Add the server under `mcpServers`:

```json
{
  "mcpServers": {
    "exchange-rates": {
      "command": "/absolute/path/to/exchange_rates/target/release/exchange_rates"
    }
  }
}
```

3. Restart Claude Desktop. The exchange rate tools will appear in the tools panel.

### Claude Code

Register the server with the Claude Code CLI:

```bash
claude mcp add exchange-rates /absolute/path/to/exchange_rates/target/release/exchange_rates
```

Or add it to your project's `.mcp.json` so all project collaborators pick it up automatically:

```json
{
  "mcpServers": {
    "exchange-rates": {
      "command": "/absolute/path/to/exchange_rates/target/release/exchange_rates"
    }
  }
}
```

## Usage examples

Once connected, you can ask Claude naturally:

- *"What was the EUR/USD rate on 15 January 2025?"*
- *"Convert 250 USD to EUR using the ECB rate for last Friday."*
- *"How much is €1,500 in Japanese yen as of 2024-12-31?"*
- *"What's the EUR/GBP rate for today?"*

## Constraints

- **Currencies:** EUR ↔ X only. Exactly one side of every conversion must be EUR (EUR→EUR is rejected).
- **Dates:** rates are available only for ECB trading days. The server walks back up to 10 days to find the nearest published rate, so weekends and public holidays resolve automatically.
- **Future dates:** rejected with an explicit error.
- **Data source:** [ECB Data Portal](https://data-api.ecb.europa.eu) — rates are fetched live and cached in memory per (currency, calendar year) for the lifetime of the server process.

## Development

```bash
cargo check          # fast compile check
cargo test           # run tests — most require internet access (hits the live ECB API)
cargo clippy         # lint
cargo fmt            # format
```
