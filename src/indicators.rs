use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub enum OverlayIndicator {
    Sma { period: usize },
    Ema { period: usize },
    BollingerBands { period: usize, multiplier: f64 },
}

#[derive(Clone, Debug)]
pub struct IndicatorConfig {
    pub overlays: Vec<OverlayIndicator>,
    pub rsi_period: Option<usize>,
}

impl IndicatorConfig {
    pub fn default_trading_view_style() -> Self {
        Self {
            overlays: vec![
                OverlayIndicator::Ema { period: 20 },
                OverlayIndicator::Sma { period: 50 },
            ],
            rsi_period: Some(14),
        }
    }

    pub fn parse(indicator_specs: &[String]) -> Result<Self, String> {
        if indicator_specs.is_empty() {
            return Ok(Self::default_trading_view_style());
        }

        let mut overlays = Vec::new();
        let mut rsi_period = None;

        for spec_group in indicator_specs {
            for raw in spec_group.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                let mut parts = raw.split(':');
                let name = parts
                    .next()
                    .ok_or_else(|| format!("Invalid indicator '{}'", raw))?
                    .to_ascii_lowercase();

                match name.as_str() {
                    "sma" => {
                        let period = parse_period(parts.next(), 20, raw)?;
                        ensure_no_extra(parts.next(), raw)?;
                        overlays.push(OverlayIndicator::Sma { period });
                    }
                    "ema" => {
                        let period = parse_period(parts.next(), 20, raw)?;
                        ensure_no_extra(parts.next(), raw)?;
                        overlays.push(OverlayIndicator::Ema { period });
                    }
                    "bb" | "bollinger" | "bollingerbands" => {
                        let period = parse_period(parts.next(), 20, raw)?;
                        let multiplier = match parts.next() {
                            Some(value) => value
                                .parse::<f64>()
                                .map_err(|_| format!("Invalid Bollinger multiplier in '{}'", raw))?,
                            None => 2.0,
                        };
                        if multiplier <= 0.0 {
                            return Err(format!("Bollinger multiplier must be > 0 in '{}'", raw));
                        }
                        ensure_no_extra(parts.next(), raw)?;
                        overlays.push(OverlayIndicator::BollingerBands { period, multiplier });
                    }
                    "rsi" => {
                        let period = parse_period(parts.next(), 14, raw)?;
                        ensure_no_extra(parts.next(), raw)?;
                        rsi_period = Some(period);
                    }
                    _ => {
                        return Err(format!(
                            "Unknown indicator '{}'. Allowed: sma[:n], ema[:n], bb[:period[:mult]], rsi[:n]",
                            raw
                        ));
                    }
                }
            }
        }

        Ok(Self { overlays, rsi_period })
    }

    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        for overlay in &self.overlays {
            match overlay {
                OverlayIndicator::Sma { period } => parts.push(format!("SMA({})", period)),
                OverlayIndicator::Ema { period } => parts.push(format!("EMA({})", period)),
                OverlayIndicator::BollingerBands { period, multiplier } => {
                    parts.push(format!("BB({}, {:.1})", period, multiplier))
                }
            }
        }

        if let Some(period) = self.rsi_period {
            parts.push(format!("RSI({})", period));
        }

        if parts.is_empty() {
            "none".to_string()
        } else {
            parts.join(", ")
        }
    }
}

fn parse_period(value: Option<&str>, default: usize, raw: &str) -> Result<usize, String> {
    let period = match value {
        Some(v) => v.parse::<usize>().map_err(|_| format!("Invalid period in '{}'", raw))?,
        None => default,
    };

    if period == 0 {
        return Err(format!("Period must be > 0 in '{}'", raw));
    }

    Ok(period)
}

fn ensure_no_extra(extra: Option<&str>, raw: &str) -> Result<(), String> {
    if extra.is_some() {
        return Err(format!("Too many parameters in indicator '{}'", raw));
    }
    Ok(())
}

#[derive(Debug)]
pub struct OverlayLine {
    pub label: String,
    pub points: Vec<(f64, f64)>,
}

#[derive(Debug)]
pub struct BollingerLines {
    pub middle: Vec<(f64, f64)>,
    pub upper: Vec<(f64, f64)>,
    pub lower: Vec<(f64, f64)>,
}

#[derive(Debug)]
pub struct OverlayOutput {
    pub lines: Vec<OverlayLine>,
    pub bollinger: Vec<BollingerLines>,
}

#[derive(Debug)]
pub struct RsiOutput {
    pub line: Vec<(f64, f64)>,
}

struct BollingerSeries {
    middle: Vec<Option<f64>>,
    upper: Vec<Option<f64>>,
    lower: Vec<Option<f64>>,
}

pub fn compute_overlays(history: &VecDeque<(f64, f64)>, overlays: &[OverlayIndicator]) -> OverlayOutput {
    let points: Vec<(f64, f64)> = history.iter().copied().collect();
    let closes: Vec<f64> = points.iter().map(|(_, p)| *p).collect();
    let xs: Vec<f64> = points.iter().map(|(t, _)| *t).collect();

    let mut lines = Vec::new();
    let mut bollinger = Vec::new();

    for indicator in overlays {
        match indicator {
            OverlayIndicator::Sma { period } => {
                let values = sma(&closes, *period);
                lines.push(OverlayLine {
                    label: format!("SMA {}", period),
                    points: zip_some_points(&xs, &values),
                });
            }
            OverlayIndicator::Ema { period } => {
                let values = ema(&closes, *period);
                lines.push(OverlayLine {
                    label: format!("EMA {}", period),
                    points: zip_some_points(&xs, &values),
                });
            }
            OverlayIndicator::BollingerBands { period, multiplier } => {
                let bb = bollinger_bands(&closes, *period, *multiplier);
                bollinger.push(BollingerLines {
                    middle: zip_some_points(&xs, &bb.middle),
                    upper: zip_some_points(&xs, &bb.upper),
                    lower: zip_some_points(&xs, &bb.lower),
                });
            }
        }
    }

    OverlayOutput { lines, bollinger }
}

pub fn compute_rsi(history: &VecDeque<(f64, f64)>, period: usize) -> RsiOutput {
    let points: Vec<(f64, f64)> = history.iter().copied().collect();
    let closes: Vec<f64> = points.iter().map(|(_, p)| *p).collect();
    let xs: Vec<f64> = points.iter().map(|(t, _)| *t).collect();
    let values = rsi(&closes, period);
    RsiOutput {
        line: zip_some_points(&xs, &values),
    }
}

fn zip_some_points(xs: &[f64], ys: &[Option<f64>]) -> Vec<(f64, f64)> {
    xs.iter()
        .copied()
        .zip(ys.iter().copied())
        .filter_map(|(x, y)| y.map(|v| (x, v)))
        .collect()
}

fn sma(prices: &[f64], period: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; prices.len()];
    if prices.is_empty() {
        return out;
    }

    let mut sum = 0.0;
    for i in 0..prices.len() {
        sum += prices[i];
        if i >= period {
            sum -= prices[i - period];
        }
        if i + 1 >= period {
            out[i] = Some(sum / period as f64);
        }
    }
    out
}

fn ema(prices: &[f64], period: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; prices.len()];
    if prices.is_empty() {
        return out;
    }

    let alpha = 2.0 / (period as f64 + 1.0);
    let mut prev = prices[0];
    for (i, price) in prices.iter().copied().enumerate() {
        let next = alpha * price + (1.0 - alpha) * prev;
        prev = next;
        if i + 1 >= period {
            out[i] = Some(next);
        }
    }
    out
}

fn bollinger_bands(prices: &[f64], period: usize, multiplier: f64) -> BollingerSeries {
    let mut mid = vec![None; prices.len()];
    let mut up = vec![None; prices.len()];
    let mut low = vec![None; prices.len()];

    if prices.is_empty() {
        return BollingerSeries {
            middle: mid,
            upper: up,
            lower: low,
        };
    }

    for i in 0..prices.len() {
        if i + 1 < period {
            continue;
        }
        let window = &prices[(i + 1 - period)..=i];
        let mean = window.iter().sum::<f64>() / period as f64;
        let variance = window.iter().map(|p| (p - mean).powi(2)).sum::<f64>() / period as f64;
        let sd = variance.sqrt();
        mid[i] = Some(mean);
        up[i] = Some(mean + sd * multiplier);
        low[i] = Some(mean - sd * multiplier);
    }

    BollingerSeries {
        middle: mid,
        upper: up,
        lower: low,
    }
}

fn rsi(prices: &[f64], period: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; prices.len()];
    if prices.is_empty() || prices.len() < 2 {
        return out;
    }

    let mut gains = 0.0;
    let mut losses = 0.0;

    for i in 1..prices.len() {
        let diff = prices[i] - prices[i - 1];
        if diff > 0.0 {
            gains += diff;
        } else {
            losses += -diff;
        }

        if i == period {
            let mut avg_gain = gains / period as f64;
            let mut avg_loss = losses / period as f64;
            out[i] = Some(rsi_from_avgs(avg_gain, avg_loss));

            for j in (i + 1)..prices.len() {
                let d = prices[j] - prices[j - 1];
                let gain = if d > 0.0 { d } else { 0.0 };
                let loss = if d < 0.0 { -d } else { 0.0 };
                avg_gain = ((avg_gain * (period as f64 - 1.0)) + gain) / period as f64;
                avg_loss = ((avg_loss * (period as f64 - 1.0)) + loss) / period as f64;
                out[j] = Some(rsi_from_avgs(avg_gain, avg_loss));
            }
            break;
        }
    }

    out
}

fn rsi_from_avgs(avg_gain: f64, avg_loss: f64) -> f64 {
    if avg_loss == 0.0 {
        return 100.0;
    }
    let rs = avg_gain / avg_loss;
    100.0 - (100.0 / (1.0 + rs))
}
