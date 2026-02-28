use anyhow::Result;
use serde::Deserialize;

const API_URL: &str = "https://api.porssisahko.net/v2/latest-prices.json";

#[derive(Debug, Deserialize)]
pub struct PricesResponse {
    pub prices: Vec<PriceEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceEntry {
    pub price: f64,
    pub start_date: String,
}

pub async fn fetch_eprices() -> Result<Vec<(String, f64)>> {
    let resp: PricesResponse = reqwest::get(API_URL).await?.json().await?;

    let prices: Vec<(String, f64)> = resp
        .prices
        .iter()
        .filter_map(|entry| {
            let ts = normalize_timestamp(&entry.start_date)?;
            Some((ts, entry.price))
        })
        .collect();

    Ok(prices)
}

fn normalize_timestamp(timestamp: &str) -> Option<String> {
    let dt = chrono::DateTime::parse_from_rfc3339(timestamp).ok()?;
    Some(dt.to_utc().format("%Y-%m-%dT%H:%M:%SZ").to_string())
}
