mod api;
mod app;

use app::{App, Asset};
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
    widgets::{Axis, Block, Borders, Chart, Dataset, GraphType},
};
use std::{env, error::Error, io, time::Duration};
use tokio::sync::mpsc;

const UPDATE_INTERVAL_MS: u64 = 100;
const PRICE_CHANNEL_CAPACITY: usize = 50;
const EVENT_POLL_MS: u64 = 10;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = env::args().skip(1).collect();
    let symbols = if args.is_empty() {
        vec!["SOLUSDT".to_string(), "BTCUSDT".to_string(), "ETHUSDT".to_string()]
    } else {
        args.into_iter()
            .map(|s| {
                let s = s.to_uppercase();
                if s.ends_with("USDT") { s } else { format!("{}USDT", s) }
            })
            .collect()
    };

    let client = reqwest::Client::new();
    let mut valid_symbols = Vec::new();
    let mut has_warnings = false;

    for sym in symbols {
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
    };

    let symbols_for_task = valid_symbols.clone();

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
            tokio::time::sleep(Duration::from_millis(UPDATE_INTERVAL_MS)).await;
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
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            app.assets
                .iter()
                .map(|_| Constraint::Ratio(1, app.assets.len() as u32))
                .collect::<Vec<_>>(),
        )
        .split(f.size());

    for (i, asset) in app.assets.iter().enumerate() {
        let history: Vec<(f64, f64)> = asset.history.iter().copied().collect();
        let (min_t, max_t) = match (history.first(), history.last()) {
            (Some(first), Some(last)) => (first.0, last.0),
            _ => (0.0, 1.0),
        };
        let (min_p, max_p) = history.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |acc, h| {
            (acc.0.min(h.1), acc.1.max(h.1))
        });

        // Add dynamic padding to the Y axis to always look readable
        let mut pad = (max_p - min_p) * 0.1;
        if pad == 0.0 {
            pad = max_p * 0.001;
        } // Handle flatlines gracefully
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
            for j in 0..5 {
                let fraction = j as f64 / 4.0;
                let ms_offset = (diff.num_milliseconds() as f64 * fraction) as i64;
                let point_time = start + chrono::Duration::milliseconds(ms_offset);
                labels.push(point_time.format("%H:%M:%S").to_string().into());
            }
        } else {
            labels = vec!["".into(), "".into(), "".into(), "".into(), "".into()];
        }

        let chart = Chart::new(vec![dataset])
            .block(
                Block::default()
                    .title(format!(" {} ${} ", asset.name, asset.price))
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

        f.render_widget(chart, chunks[i]);
    }
}
