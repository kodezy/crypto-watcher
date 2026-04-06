# Crypto Watcher

> **Note:** This project was created specifically to study and practice Rust programming concepts, particularly async runtime, TUI development, and clean code practices.

![Project Preview](./assets/preview.png)

---

## Features

- Monitors real-time prices for any symbol on Binance
- Preloads historical candles at startup for immediate chart context
- Configurable timeframe (`--interval`) and historical depth (`--lookback`)
- Renders multi-asset charts using `ratatui`
- Displays dynamically scaling Y-axes and human-readable X-axes with timestamps

---

## Requirements

- Rust (latest stable, with Edition 2024 support)
- Cargo
- Working internet connection (to reach Binance API)

---

## Installation

1. Clone the repository:
```bash
git clone https://github.com/kodezy/crypto-watcher.git
cd crypto-watcher
```

2. Build the project:
```bash
cargo build --release
```

---

## Usage

Start the application directly using Cargo:

```bash
cargo run -- [OPTIONS] [symbol1] [symbol2] ...
```

Or run the compiled binary:

```bash
./target/release/crypto-watcher [OPTIONS] [symbol1] [symbol2] ...
```

- **`--interval` / `-i`**: Candle interval (examples: `1m`, `5m`, `1h`, `1d`).
- **`--lookback` / `-l`**: Number of historical candles to preload (1 to 600).
- **`--refresh-ms` / `-r`**: Real-time polling interval in milliseconds.

Examples:

```bash
cargo run -- --interval 5m --lookback 300 BTC ETH SOL
cargo run -- -i 1h -l 168 BTCUSDT
```

- **Exit:** Press `q` to safely exit the application.

---

## Architecture

- **`main.rs`**: Core application flow, CLI parsing, startup preload, async update loop, and TUI rendering.
- **`app.rs`**: Application state models (`App`, `Asset`) and in-memory history management.
- **`api.rs`**: Binance API helpers (symbol validation, spot price, and historical klines fetch).
- **Dependencies**:
  - `ratatui` & `crossterm`: Terminal UI rendering and event mapping.
  - `tokio`: Async runtime for parallel REST requests.
  - `reqwest`: HTTP client to fetch data.
  - `serde` & `serde_json`: JSON parsing.
  - `chrono`: Real-time timestamp formatting.
