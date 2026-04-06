mod api;
mod app;
mod indicators;

use app::{App, Asset, MAX_DATA_POINTS};
use chrono::TimeZone;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use indicators::{IndicatorConfig, compute_overlays, compute_rsi};
use ratatui::{
    Frame, Terminal,
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span},
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
const UI_BORDER: Color = Color::Rgb(74, 81, 95);
const PRICE_COLOR: Color = Color::Rgb(229, 189, 91);
const RSI_COLOR: Color = Color::Rgb(143, 158, 255);
const RSI_OVERBOUGHT_COLOR: Color = Color::Rgb(255, 110, 110);
const RSI_OVERSOLD_COLOR: Color = Color::Rgb(80, 217, 153);
const BB_MID_COLOR: Color = Color::Rgb(100, 162, 255);
const BB_UPPER_COLOR: Color = Color::Rgb(255, 129, 117);
const BB_LOWER_COLOR: Color = Color::Rgb(86, 214, 146);

struct CliConfig {
    symbols: Vec<String>,
    interval: String,
    lookback_points: usize,
    refresh_ms: u64,
    indicators: IndicatorConfig,
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
    println!("  -I, --indicator <SPEC>     Indicator spec (repeatable):");
    println!("                             sma:20, ema:50, bb:20:2, rsi:14");
    println!("                             Default: EMA(20), SMA(50), RSI(14)");
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
    let mut indicator_specs = Vec::new();
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
            "-I" | "--indicator" => {
                let value = args.next().ok_or("Missing value for --indicator")?;
                indicator_specs.push(value);
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

    let indicators = IndicatorConfig::parse(&indicator_specs)?;

    Ok(CliConfig {
        symbols,
        interval,
        lookback_points,
        refresh_ms,
        indicators,
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
        indicators: cli.indicators.clone(),
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
        .constraints([Constraint::Length(3), Constraint::Min(1), Constraint::Length(3)])
        .split(f.size());

    let header = Paragraph::new(Line::from(format!(
        " Crypto Watcher  |  Interval: {}  |  Lookback: {} candles  |  Assets: {}  |  Indicators: {} ",
        app.interval,
        app.lookback_points,
        app.assets.len(),
        app.indicators.describe(),
    )))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(UI_BORDER)),
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
        let overlay_data = compute_overlays(&asset.history, &app.indicators.overlays);

        let mut price_datasets = Vec::new();
        price_datasets.push(
            Dataset::default()
                .name("Price")
                .marker(symbols::Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(PRICE_COLOR))
                .data(&history),
        );

        for (line_idx, line) in overlay_data.lines.iter().enumerate() {
            let color = match line_idx % 6 {
                0 => Color::Rgb(57, 179, 255),
                1 => Color::Rgb(190, 107, 255),
                2 => Color::Rgb(113, 227, 151),
                3 => Color::Rgb(255, 132, 128),
                4 => Color::Rgb(226, 175, 65),
                _ => Color::Rgb(168, 176, 189),
            };

            price_datasets.push(
                Dataset::default()
                    .name(line.label.clone())
                    .marker(symbols::Marker::Braille)
                    .graph_type(GraphType::Line)
                    .style(Style::default().fg(color))
                    .data(&line.points),
            );
        }

        for bb in &overlay_data.bollinger {
            price_datasets.push(
                Dataset::default()
                    .name("BB Mid")
                    .marker(symbols::Marker::Braille)
                    .graph_type(GraphType::Line)
                    .style(Style::default().fg(BB_MID_COLOR))
                    .data(&bb.middle),
            );
            price_datasets.push(
                Dataset::default()
                    .name("BB Upper")
                    .marker(symbols::Marker::Braille)
                    .graph_type(GraphType::Line)
                    .style(Style::default().fg(BB_UPPER_COLOR))
                    .data(&bb.upper),
            );
            price_datasets.push(
                Dataset::default()
                    .name("BB Lower")
                    .marker(symbols::Marker::Braille)
                    .graph_type(GraphType::Line)
                    .style(Style::default().fg(BB_LOWER_COLOR))
                    .data(&bb.lower),
            );
        }

        let mut y_sources: Vec<&[(f64, f64)]> = vec![&history];
        for line in &overlay_data.lines {
            y_sources.push(&line.points);
        }
        for bb in &overlay_data.bollinger {
            y_sources.push(&bb.middle);
            y_sources.push(&bb.upper);
            y_sources.push(&bb.lower);
        }
        let y_bounds = compute_padded_bounds(&y_sources);

        let has_rsi = app.indicators.rsi_period.is_some();
        let inner_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(if has_rsi {
                vec![Constraint::Ratio(2, 3), Constraint::Ratio(1, 3)]
            } else {
                vec![Constraint::Min(1)]
            })
            .split(chart_chunks[i]);

        let price_chart = Chart::new(price_datasets)
            .block(
                Block::default()
                    .title(format!(" {} ${} | {} pts ", asset.name, asset.price, history.len()))
                    .title_style(Style::default().add_modifier(Modifier::BOLD))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(UI_BORDER)),
            )
            .x_axis(Axis::default().bounds([min_t, max_t]).labels(x_labels(asset)))
            .y_axis(Axis::default().bounds(y_bounds).labels(y_labels(y_bounds)));

        f.render_widget(price_chart, inner_chunks[0]);

        if let Some(period) = app.indicators.rsi_period {
            let rsi = compute_rsi(&asset.history, period);
            let upper_level = vec![(min_t, 70.0), (max_t, 70.0)];
            let lower_level = vec![(min_t, 30.0), (max_t, 30.0)];

            let rsi_chart = Chart::new(vec![
                Dataset::default()
                    .name("RSI")
                    .marker(symbols::Marker::Braille)
                    .graph_type(GraphType::Line)
                    .style(Style::default().fg(RSI_COLOR))
                    .data(&rsi.line),
                Dataset::default()
                    .name("70")
                    .marker(symbols::Marker::Dot)
                    .graph_type(GraphType::Line)
                    .style(Style::default().fg(RSI_OVERBOUGHT_COLOR))
                    .data(&upper_level),
                Dataset::default()
                    .name("30")
                    .marker(symbols::Marker::Dot)
                    .graph_type(GraphType::Line)
                    .style(Style::default().fg(RSI_OVERSOLD_COLOR))
                    .data(&lower_level),
            ])
            .block(
                Block::default()
                    .title(format!(" RSI {} ", period))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(UI_BORDER)),
            )
            .x_axis(Axis::default().bounds([min_t, max_t]).labels(x_labels(asset)))
            .y_axis(Axis::default().bounds([0.0, 100.0]).labels(vec![
                "0".into(),
                "50".into(),
                "100".into(),
            ]));

            f.render_widget(rsi_chart, inner_chunks[1]);
        }
    }

    let footer = Paragraph::new(vec![
        Line::from(vec![
            Span::raw("Legend: "),
            Span::styled("Price", Style::default().fg(PRICE_COLOR)),
            Span::raw(" | "),
            Span::styled("SMA/EMA", Style::default().fg(Color::Rgb(57, 179, 255))),
            Span::raw(" | "),
            Span::styled("BB Mid", Style::default().fg(BB_MID_COLOR)),
            Span::raw(" | "),
            Span::styled("BB Upper", Style::default().fg(BB_UPPER_COLOR)),
            Span::raw(" | "),
            Span::styled("BB Lower", Style::default().fg(BB_LOWER_COLOR)),
            Span::raw(" | "),
            Span::styled("RSI", Style::default().fg(RSI_COLOR)),
            Span::raw(" | "),
            Span::styled("RSI 70", Style::default().fg(RSI_OVERBOUGHT_COLOR)),
            Span::raw(" | "),
            Span::styled("RSI 30", Style::default().fg(RSI_OVERSOLD_COLOR)),
        ]),
        Line::from("Controls: q = quit  |  CLI: --interval <tf> --lookback <n> --refresh-ms <ms> --indicator <spec>"),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(UI_BORDER)),
    )
    .wrap(Wrap { trim: true });
    f.render_widget(footer, base_chunks[2]);
}

fn x_labels(asset: &Asset) -> Vec<Span<'static>> {
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
    labels
}

fn y_labels(bounds: [f64; 2]) -> Vec<Span<'static>> {
    vec![
        format!("{:.2}", bounds[0]).into(),
        format!("{:.2}", (bounds[0] + bounds[1]) / 2.0).into(),
        format!("{:.2}", bounds[1]).into(),
    ]
}

fn compute_padded_bounds(datasets: &[&[(f64, f64)]]) -> [f64; 2] {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for dataset in datasets {
        for (_, value) in dataset.iter() {
            min = min.min(*value);
            max = max.max(*value);
        }
    }

    if !min.is_finite() || !max.is_finite() {
        return [0.0, 1.0];
    }

    let mut pad = (max - min) * 0.1;
    if pad == 0.0 {
        pad = if max == 0.0 { 1.0 } else { max.abs() * 0.001 };
    }

    [min - pad, max + pad]
}
