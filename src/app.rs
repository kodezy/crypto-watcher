use chrono::{DateTime, Local};
use std::collections::VecDeque;

pub const MAX_DATA_POINTS: usize = 600;

pub struct Asset {
    pub name: String,
    pub price: String,
    pub history: VecDeque<(f64, f64)>,
    pub timestamps: VecDeque<DateTime<Local>>,
}

impl Asset {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.replace("USDT", ""),
            price: "0.00".to_string(),
            history: VecDeque::with_capacity(MAX_DATA_POINTS),
            timestamps: VecDeque::with_capacity(MAX_DATA_POINTS),
        }
    }

    pub fn update(&mut self, price: String) {
        self.price = price.clone();
        if let Ok(p) = price.parse::<f64>() {
            self.push_point(Local::now(), p);
        }
    }

    pub fn push_point(&mut self, timestamp: DateTime<Local>, price: f64) {
        let timestamp_f64 = timestamp.timestamp() as f64 + (timestamp.timestamp_subsec_millis() as f64 / 1000.0);

        if self.history.len() >= MAX_DATA_POINTS {
            self.history.pop_front();
            self.timestamps.pop_front();
        }

        self.history.push_back((timestamp_f64, price));
        self.timestamps.push_back(timestamp);
        self.price = format!("{:.4}", price);
    }
}

pub struct App {
    pub assets: Vec<Asset>,
    pub interval: String,
    pub lookback_points: usize,
}
