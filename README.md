# exchange_rates

[![Rust](https://img.shields.io/badge/rust-2024_edition-orange?logo=rust)](https://www.rust-lang.org)
[![MCP](https://img.shields.io/badge/MCP-compatible-blue?logo=anthropic)](https://modelcontextprotocol.io)
[![Data source](https://img.shields.io/badge/data-ECB_Data_Portal-003591?logo=europeanunion&logoColor=white)](https://data-api.ecb.europa.eu)
[![Build](https://img.shields.io/badge/build-cargo-green?logo=rust)](https://doc.rust-lang.org/cargo/)

An [MCP (Model Context Protocol)](https://modelcontextprotocol.io) server that provides EUR/USD exchange rates sourced directly from the **European Central Bank (ECB)**. Connect it to Claude to look up historical rates and convert amounts between EUR and USD by date.

## Tools

| Tool | Description |
|------|-------------|
| `get_exchange_rate` | Returns the EUR/USD rate published by the ECB for a given date |
| `convert_currency` | Converts an amount between EUR and USD using the ECB rate for a given date |

If the requested date falls on a weekend or public holiday, the server automatically returns the most recent available rate (looking back up to 10 days).

## Prerequisites

- [Rust](https://rustup.rs) (edition 2024, stable toolchain)

## Build

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
- *"How much is €1,500 in dollars as of 2024-12-31?"*

## Constraints

- **Currencies:** only EUR ↔ USD is supported.
- **Dates:** rates are available only for ECB trading days. The server walks back up to 10 days to find the nearest published rate, so weekends and public holidays resolve automatically.
- **Future dates:** rejected with an explicit error.
- **Data source:** [ECB Data Portal](https://data-api.ecb.europa.eu/service/data/EXR/D.USD.EUR.SP00.A) — rates are fetched live and cached in memory per calendar year for the lifetime of the server process.

## Development

```bash
cargo check          # fast compile check
cargo test           # integration tests — requires internet access (hits the live ECB API)
cargo clippy         # lint
cargo fmt            # format
```

> **Note:** the test suite makes real HTTP requests to the ECB API. There are no mocks.
