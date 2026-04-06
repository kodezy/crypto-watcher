use serde::Deserialize;

pub fn binance_price_url(symbol: &str) -> String {
    format!("https://api.binance.com/api/v3/ticker/price?symbol={}", symbol)
}

pub fn binance_klines_url(symbol: &str, interval: &str, limit: usize) -> String {
    format!(
        "https://api.binance.com/api/v3/klines?symbol={}&interval={}&limit={}",
        symbol, interval, limit
    )
}

#[derive(Deserialize)]
pub struct PriceResponse {
    pub price: String,
}

pub async fn validate_symbol(client: &reqwest::Client, symbol: &str) -> Result<bool, reqwest::Error> {
    let url = binance_price_url(symbol);
    let resp = client.get(&url).send().await?;
    Ok(resp.status().is_success())
}

pub async fn fetch_klines(
    client: &reqwest::Client,
    symbol: &str,
    interval: &str,
    limit: usize,
) -> Result<Vec<(i64, f64)>, Box<dyn std::error::Error + Send + Sync>> {
    let url = binance_klines_url(symbol, interval, limit);
    let resp = client.get(&url).send().await?.error_for_status()?;
    let raw = resp.json::<Vec<Vec<serde_json::Value>>>().await?;

    let mut out = Vec::with_capacity(raw.len());
    for candle in raw {
        if candle.len() < 5 {
            continue;
        }

        let open_time = match candle[0].as_i64() {
            Some(v) => v,
            None => continue,
        };
        let close_price = match candle[4].as_str().and_then(|p| p.parse::<f64>().ok()) {
            Some(v) => v,
            None => continue,
        };
        out.push((open_time, close_price));
    }

    Ok(out)
}
