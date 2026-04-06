mod api;
mod app;

use app::{App, Asset, MAX_DATA_POINTS};
use chrono::TimeZone;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    symbols,
    text::Line,
    widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, Paragraph, Wrap},
};
use std::{env, error::Error, io, time::Duration};
use tokio::sync::mpsc;

const UPDATE_INTERVAL_MS: u64 = 100;
const DEFAULT_LOOKBACK_POINTS: usize = 240;
const MAX_LOOKBACK_POINTS: usize = MAX_DATA_POINTS;
const PRICE_CHANNEL_CAPACITY: usize = 50;
const EVENT_POLL_MS: u64 = 10;
const DEFAULT_INTERVAL: &str = "1m";
const DEFAULT_SYMBOLS: [&str; 3] = ["SOLUSDT", "BTCUSDT", "ETHUSDT"];
const ALLOWED_INTERVALS: [&str; 16] = [
    "1s", "1m", "3m", "5m", "15m", "30m", "1h", "2h", "4h", "6h", "8h", "12h", "1d", "3d", "1w", "1M",
];

struct CliConfig {
    symbols: Vec<String>,
    interval: String,
    lookback_points: usize,
    refresh_ms: u64,
}

fn print_usage() {
    println!("crypto-watcher [OPTIONS] [SYMBOLS...]");
    println!();
    println!("Options:");
    println!("  -i, --interval <VALUE>     Candle interval (ex: 1m, 5m, 1h, 1d)");
    println!(
        "  -l, --lookback <POINTS>    Number of historical candles to preload (1-{})",
        MAX_LOOKBACK_POINTS
    );
    println!("  -r, --refresh-ms <MS>      Real-time price polling interval in milliseconds");
    println!("  -h, --help                 Show this help");
    println!();
    println!("Examples:");
    println!("  cargo run -- --interval 5m --lookback 300 BTC ETH SOL");
    println!("  cargo run -- -i 1h -l 168 BTCUSDT");
}

fn parse_cli_config() -> Result<CliConfig, String> {
    let mut interval = DEFAULT_INTERVAL.to_string();
    let mut lookback_points = DEFAULT_LOOKBACK_POINTS;
    let mut refresh_ms = UPDATE_INTERVAL_MS;
    let mut raw_symbols = Vec::new();

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            "-i" | "--interval" => {
                let value = args.next().ok_or("Missing value for --interval")?;
                interval = value;
            }
            "-l" | "--lookback" => {
                let value = args.next().ok_or("Missing value for --lookback")?;
                lookback_points = value
                    .parse::<usize>()
                    .map_err(|_| "Invalid --lookback: expected a positive integer")?;
            }
            "-r" | "--refresh-ms" => {
                let value = args.next().ok_or("Missing value for --refresh-ms")?;
                refresh_ms = value
                    .parse::<u64>()
                    .map_err(|_| "Invalid --refresh-ms: expected an integer in ms")?;
            }
            _ if arg.starts_with('-') => {
                return Err(format!("Unknown option: {}", arg));
            }
            _ => raw_symbols.push(arg),
        }
    }

    if !ALLOWED_INTERVALS.contains(&interval.as_str()) {
        return Err(format!(
            "Invalid interval '{}'. Allowed values: {}",
            interval,
            ALLOWED_INTERVALS.join(", ")
        ));
    }

    if lookback_points == 0 || lookback_points > MAX_LOOKBACK_POINTS {
        return Err(format!(
            "Invalid --lookback '{}': expected between 1 and {}",
            lookback_points, MAX_LOOKBACK_POINTS
        ));
    }

    if refresh_ms == 0 {
        return Err("Invalid --refresh-ms '0': expected > 0".to_string());
    }

    let symbols = if raw_symbols.is_empty() {
        DEFAULT_SYMBOLS.into_iter().map(|s| s.to_string()).collect()
    } else {
        raw_symbols
            .into_iter()
            .map(|s| {
                let s = s.to_uppercase();
                if s.ends_with("USDT") { s } else { format!("{}USDT", s) }
            })
            .collect()
    };

    Ok(CliConfig {
        symbols,
        interval,
        lookback_points,
        refresh_ms,
    })
}

fn x_axis_time_format(diff: chrono::Duration) -> &'static str {
    if diff.num_days() >= 60 {
        "%d %b %Y"
    } else if diff.num_days() >= 2 {
        "%d %b %H:%M"
    } else if diff.num_hours() >= 2 {
        "%H:%M"
    } else {
        "%H:%M:%S"
    }
}

fn draw_loading_screen<B: Backend>(
    terminal: &mut Terminal<B>,
    loaded: usize,
    total: usize,
    symbol: &str,
    interval: &str,
    lookback: usize,
) -> io::Result<()> {
    terminal.draw(|f| {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(1), Constraint::Length(3)])
            .split(f.size());

        let header = Paragraph::new(Line::from(" Crypto Watcher - Loading Market Data "))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray)),
            )
            .style(Style::default().add_modifier(Modifier::BOLD));
        f.render_widget(header, chunks[0]);

        let body = Paragraph::new(format!(
            "Preparing charts...\nAsset: {}\nProgress: {}/{}\nInterval: {} | Lookback: {} candles",
            symbol, loaded, total, interval, lookback
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .wrap(Wrap { trim: true });
        f.render_widget(body, chunks[1]);

        let footer = Paragraph::new("Please wait. Data is being preloaded from Binance.")
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray)),
            )
            .wrap(Wrap { trim: true });
        f.render_widget(footer, chunks[2]);
    })?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = match parse_cli_config() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("Error: {}", e);
            eprintln!();
            print_usage();
            return Ok(());
        }
    };

    let client = reqwest::Client::new();
    let mut valid_symbols = Vec::new();
    let mut has_warnings = false;

    for sym in cli.symbols {
        match api::validate_symbol(&client, &sym).await {
            Ok(true) => valid_symbols.push(sym),
            Ok(false) => {
                tracing::warn!("Symbol {} not found", sym);
                has_warnings = true;
            }
            Err(e) => {
                tracing::warn!("Error checking {}: {}", sym, e);
                has_warnings = true;
            }
        }
    }

    if valid_symbols.is_empty() {
        tracing::error!("No valid symbols to monitor. Exiting.");
        return Ok(());
    }

    if has_warnings {
        tracing::info!("Starting monitoring in 3 seconds...");
        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let (tx, mut rx) = mpsc::channel(PRICE_CHANNEL_CAPACITY);
    let mut app = App {
        assets: valid_symbols.iter().map(|s| Asset::new(s)).collect(),
        interval: cli.interval.clone(),
        lookback_points: cli.lookback_points,
    };

    let total_assets = app.assets.len();
    for (idx, asset) in app.assets.iter_mut().enumerate() {
        let symbol = format!("{}USDT", asset.name);
        draw_loading_screen(
            &mut terminal,
            idx,
            total_assets,
            &symbol,
            &cli.interval,
            cli.lookback_points,
        )?;

        match api::fetch_klines(&client, &symbol, &cli.interval, cli.lookback_points).await {
            Ok(points) => {
                for (open_time_ms, close_price) in points {
                    if let Some(ts) = chrono::Local.timestamp_millis_opt(open_time_ms).single() {
                        asset.push_point(ts, close_price);
                    }
                }
            }
            Err(e) => tracing::warn!("Could not preload history for {}: {}", symbol, e),
        }

        draw_loading_screen(
            &mut terminal,
            idx + 1,
            total_assets,
            &symbol,
            &cli.interval,
            cli.lookback_points,
        )?;
    }

    let symbols_for_task = valid_symbols.clone();
    let refresh_ms = cli.refresh_ms;

    tokio::spawn(async move {
        let client = reqwest::Client::new();
        loop {
            for symbol in &symbols_for_task {
                let url = api::binance_price_url(symbol);
                match client.get(&url).send().await {
                    Ok(resp) if resp.status().is_success() => {
                        if let Ok(json) = resp.json::<api::PriceResponse>().await {
                            let _ = tx.send((symbol.clone(), json.price)).await;
                        } else {
                            tracing::debug!("Failed to parse price response for {}", symbol);
                        }
                    }
                    Ok(_) => tracing::debug!("Non-success response for {}", symbol),
                    Err(e) => tracing::warn!("Fetch failed for {}: {}", symbol, e),
                }
            }
            tokio::time::sleep(Duration::from_millis(refresh_ms)).await;
        }
    });

    while let Ok(res) = run_app(&mut terminal, &mut app, &mut rx).await {
        if res {
            break;
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    Ok(())
}

async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    rx: &mut mpsc::Receiver<(String, String)>,
) -> io::Result<bool> {
    terminal.draw(|f| ui(f, app))?;

    if event::poll(Duration::from_millis(EVENT_POLL_MS))?
        && let Event::Key(key) = event::read()?
        && let KeyCode::Char('q') = key.code
    {
        return Ok(true);
    }

    while let Ok((symbol, price)) = rx.try_recv() {
        if let Some(asset) = app.assets.iter_mut().find(|a| format!("{}USDT", a.name) == symbol) {
            asset.update(price);
        }
    }

    Ok(false)
}

fn ui(f: &mut Frame, app: &App) {
    let base_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1), Constraint::Length(2)])
        .split(f.size());

    let header = Paragraph::new(Line::from(format!(
        " Crypto Watcher  |  Interval: {}  |  Lookback: {} candles  |  Assets: {} ",
        app.interval,
        app.lookback_points,
        app.assets.len()
    )))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    )
    .style(Style::default().add_modifier(Modifier::BOLD));
    f.render_widget(header, base_chunks[0]);

    let chart_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            app.assets
                .iter()
                .map(|_| Constraint::Ratio(1, app.assets.len() as u32))
                .collect::<Vec<_>>(),
        )
        .split(base_chunks[1]);

    for (i, asset) in app.assets.iter().enumerate() {
        let history: Vec<(f64, f64)> = asset.history.iter().copied().collect();
        let (min_t, max_t) = match (history.first(), history.last()) {
            (Some(first), Some(last)) => (first.0, last.0),
            _ => (0.0, 1.0),
        };
        let (min_p, max_p) = if history.is_empty() {
            (0.0, 1.0)
        } else {
            history.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |acc, h| {
                (acc.0.min(h.1), acc.1.max(h.1))
            })
        };

        let mut pad = (max_p - min_p) * 0.1;
        if pad == 0.0 {
            pad = if max_p == 0.0 { 1.0 } else { max_p * 0.001 };
        }
        let y_bounds = [min_p - pad, max_p + pad];

        let dataset = Dataset::default()
            .marker(symbols::Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(match i % 6 {
                0 => Color::Cyan,
                1 => Color::Yellow,
                2 => Color::Magenta,
                3 => Color::Green,
                4 => Color::Red,
                _ => Color::Blue,
            }))
            .data(&history);

        let mut labels = vec![];
        if let (Some(&start), Some(&end)) = (asset.timestamps.front(), asset.timestamps.back()) {
            let diff = end - start;
            let fmt = x_axis_time_format(diff);
            for j in 0..5 {
                let fraction = j as f64 / 4.0;
                let ms_offset = (diff.num_milliseconds() as f64 * fraction) as i64;
                let point_time = start + chrono::Duration::milliseconds(ms_offset);
                labels.push(point_time.format(fmt).to_string().into());
            }
        } else {
            labels = vec!["".into(), "".into(), "".into(), "".into(), "".into()];
        }

        let chart = Chart::new(vec![dataset])
            .block(
                Block::default()
                    .title(format!(" {} ${} | {} pts ", asset.name, asset.price, history.len()))
                    .title_style(Style::default().add_modifier(Modifier::BOLD))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray)),
            )
            .x_axis(Axis::default().bounds([min_t, max_t]).labels(labels))
            .y_axis(Axis::default().bounds(y_bounds).labels(vec![
                format!("{:.2}", y_bounds[0]).into(),
                format!("{:.2}", (y_bounds[0] + y_bounds[1]) / 2.0).into(),
                format!("{:.2}", y_bounds[1]).into(),
            ]));

        f.render_widget(chart, chart_chunks[i]);
    }

    let footer =
        Paragraph::new("Controls: q = quit  |  Startup config: --interval <tf> --lookback <n> --refresh-ms <ms>")
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray)),
            )
            .wrap(Wrap { trim: true });
    f.render_widget(footer, base_chunks[2]);
}
