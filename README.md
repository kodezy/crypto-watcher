# Crypto Watcher

> **Note:** This project was created specifically to study and practice Rust programming concepts, particularly async runtime, TUI development, and clean code practices.

![Project Preview](./assets/preview.png)

---

## Features

- Monitors real-time prices for any symbol on Binance
- Renders elegant sparkline charts using `ratatui`
- Maintains persistent history up to 600 data points
- Displays dynamically scaling Y-axes and human-readable X-axes with timestamps

---

## Requirements

- Rust 1.70+
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
cargo run <symbol1> <symbol2> ...
```

Or run the compiled binary:

```bash
./target/release/crypto-watcher
```

- **Exit:** Press `q` to safely exit the application.

---

## Architecture

- **`main.rs`**: Core application logic. Contains the data structures (`Asset`), the Tokio-based async fetching loop, and the rendering logic using Ratatui.
- **Dependencies**:
  - `ratatui` & `crossterm`: Terminal UI rendering and event mapping.
  - `tokio`: Async runtime for parallel REST requests.
  - `reqwest`: HTTP client to fetch data.
  - `serde` & `serde_json`: JSON parsing.
  - `chrono`: Real-time timestamp formatting.
