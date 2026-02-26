use serde::Deserialize;

pub fn binance_price_url(symbol: &str) -> String {
    format!("https://api.binance.com/api/v3/ticker/price?symbol={}", symbol)
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
