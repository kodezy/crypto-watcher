use chrono::{DateTime, Local};
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
use serde::Deserialize;
use std::{collections::VecDeque, env, error::Error, io, time::Duration};
use tokio::sync::mpsc;

const MAX_DATA_POINTS: usize = 600; // Increased history buffer
const UPDATE_INTERVAL_MS: u64 = 100; // 0.1 seconds

#[derive(Deserialize)]
struct PriceResponse {
    price: String,
}

struct Asset {
    name: String,
    price: String,
    history: VecDeque<(f64, f64)>, // (timestamp_f64, price)
    timestamps: VecDeque<DateTime<Local>>,
}

impl Asset {
    fn new(name: &str) -> Self {
        Self {
            name: name.replace("USDT", ""),
            price: "0.00".to_string(),
            history: VecDeque::with_capacity(MAX_DATA_POINTS),
            timestamps: VecDeque::with_capacity(MAX_DATA_POINTS),
        }
    }

    fn update(&mut self, price: String) {
        self.price = price.clone();
        if let Ok(p) = price.parse::<f64>() {
            let now = Local::now();
            let timestamp_f64 = now.timestamp() as f64 + (now.timestamp_subsec_millis() as f64 / 1000.0);

            if self.history.len() >= MAX_DATA_POINTS {
                self.history.pop_front();
                self.timestamps.pop_front();
            }

            self.history.push_back((timestamp_f64, p));
            self.timestamps.push_back(now);
        }
    }
}

struct App {
    assets: Vec<Asset>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
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
        let url = format!("https://api.binance.com/api/v3/ticker/price?symbol={}", sym);
        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => valid_symbols.push(sym),
            Ok(_) => {
                println!("Symbol {} not found", sym);
                has_warnings = true;
            }
            Err(e) => {
                println!("Error checking {}: {}", sym, e);
                has_warnings = true;
            }
        }
    }

    if valid_symbols.is_empty() {
        println!("No valid symbols to monitor. Exiting.");
        return Ok(());
    }

    if has_warnings {
        println!("Starting monitoring in 3 seconds...");
        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let (tx, mut rx) = mpsc::channel(50);
    let mut app = App {
        assets: valid_symbols.iter().map(|s| Asset::new(s)).collect(),
    };

    let symbols_for_task = valid_symbols.clone();

    tokio::spawn(async move {
        let client = reqwest::Client::new();
        loop {
            for symbol in &symbols_for_task {
                let url = format!("https://api.binance.com/api/v3/ticker/price?symbol={}", symbol);
                if let Ok(resp) = client.get(&url).send().await
                    && let Ok(json) = resp.json::<PriceResponse>().await
                {
                    let _ = tx.send((symbol.clone(), json.price)).await;
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

    if event::poll(Duration::from_millis(10))?
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
        let (min_t, max_t) = history
            .first()
            .map(|h| (h.0, history.last().unwrap().0))
            .unwrap_or((0.0, 1.0));
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

        // Calculate 5 timestamp bins for the X-axis
        let mut labels = vec![];
        if asset.timestamps.len() >= 2 {
            let start = asset.timestamps.front().unwrap();
            let end = asset.timestamps.back().unwrap();
            let diff = *end - *start;

            for j in 0..5 {
                let fraction = j as f64 / 4.0;
                let ms_offset = (diff.num_milliseconds() as f64 * fraction) as i64;
                let point_time = *start + chrono::Duration::milliseconds(ms_offset);
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
