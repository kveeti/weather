use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};
use quick_xml::events::Event;
use quick_xml::Reader;

#[derive(Debug, Clone)]
pub struct ForecastPoint {
    pub timestamp: DateTime<Utc>,
    pub temperature_c: f64,
    pub wind_speed_ms: f64,
    pub precipitation_mm: f64,
}

impl ForecastPoint {
    pub fn weighted_avg_temperature(
        points: &[Self],
        decay: f64,
        horizon_hours: usize,
        skip_hours: usize,
    ) -> f64 {
        if points.is_empty() {
            return f64::NAN;
        }

        let n = points.len().min(skip_hours + horizon_hours);
        let mut sum = 0.0;
        let mut weight_sum = 0.0;

        for (i, point) in points
            .iter()
            .skip(skip_hours)
            .take(n - skip_hours)
            .enumerate()
        {
            if !point.temperature_c.is_finite() {
                continue;
            }
            let w = decay.powi(i as i32);
            sum += point.temperature_c * w;
            weight_sum += w;
        }

        if weight_sum > 0.0 {
            sum / weight_sum
        } else {
            f64::NAN
        }
    }
}

pub fn temp_to_radiator_setting(temp_c: f64) -> f64 {
    if !temp_c.is_finite() {
        return f64::NAN;
    }

    if temp_c < 5.0 {
        3.5
    } else if temp_c < 15.0 {
        2.0
    } else {
        0.0
    }
}

const FMI_WFS_URL: &str = "https://opendata.fmi.fi/wfs";

pub async fn fetch_forecast(place: &str) -> Result<Vec<ForecastPoint>> {
    let client = reqwest::Client::builder().use_rustls_tls().build()?;

    let now = Utc::now();
    let start = now.format("%Y-%m-%dT%H:%M:%S.000Z");
    let end = (now + Duration::days(7)).format("%Y-%m-%dT%H:%M:%S.000Z");

    let params = "Temperature,WindSpeedMS,Precipitation1h";

    let url = format!(
        "{}?request=getFeature&storedquery_id=fmi::forecast::edited::weather::scandinavia::point::multipointcoverage&parameters={}&starttime={}&endtime={}&place={}",
        FMI_WFS_URL, params, start, end, place
    );

    tracing::debug!("FMI request: {url}");
    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        return Err(anyhow!("FMI API returned status {}", resp.status()));
    }
    let xml = resp.text().await?;
    tracing::trace!("FMI response: {} bytes", xml.len());
    parse_multipointcoverage(&xml)
}

fn parse_multipointcoverage(xml: &str) -> Result<Vec<ForecastPoint>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut in_positions = false;
    let mut in_datablock = false;
    let mut positions_text = String::new();
    let mut datablock_text = String::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = e.name();
                let local = name.local_name();
                match local.as_ref() {
                    b"positions" => in_positions = true,
                    b"DataBlock" => in_datablock = true,
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let name = e.name();
                let local = name.local_name();
                match local.as_ref() {
                    b"positions" => in_positions = false,
                    b"DataBlock" => in_datablock = false,
                    _ => {}
                }
            }
            Ok(Event::Text(e)) => {
                if in_positions {
                    positions_text.push_str(&e.unescape()?);
                }
                if in_datablock {
                    datablock_text.push_str(&e.unescape()?);
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow!("XML parse error: {e}")),
            _ => {}
        }
        buf.clear();
    }

    if positions_text.is_empty() {
        return Err(anyhow!("No positions found in FMI response"));
    }
    if datablock_text.is_empty() {
        return Err(anyhow!("No data block found in FMI response"));
    }

    let pos_tokens: Vec<&str> = positions_text.split_whitespace().collect();
    let mut timestamps: Vec<i64> = Vec::new();
    let mut i = 0;
    while i + 2 < pos_tokens.len() {
        let epoch: i64 = pos_tokens[i + 2].parse()?;
        timestamps.push(epoch);
        i += 3;
    }

    let data_tokens: Vec<&str> = datablock_text.split_whitespace().collect();
    const ROW_LEN: usize = 3;
    let n = timestamps.len();
    if data_tokens.len() < n * ROW_LEN {
        return Err(anyhow!(
            "Data block has {} tokens but expected at least {} ({}*{})",
            data_tokens.len(),
            n * ROW_LEN,
            n,
            ROW_LEN
        ));
    }

    let mut points = Vec::with_capacity(n);
    for (row_idx, &epoch) in timestamps.iter().enumerate() {
        let base = row_idx * ROW_LEN;
        let temperature_c = parse_val(data_tokens[base]);
        let wind_speed_ms = parse_val(data_tokens[base + 1]);
        let precipitation_mm = parse_val(data_tokens[base + 2]);

        if temperature_c.is_nan() {
            continue;
        }

        let timestamp = DateTime::from_timestamp(epoch, 0)
            .ok_or_else(|| anyhow!("Invalid timestamp {epoch}"))?;

        points.push(ForecastPoint {
            timestamp,
            temperature_c,
            wind_speed_ms,
            precipitation_mm,
        });
    }

    tracing::trace!("Parsed {} forecast points", points.len());
    Ok(points)
}

fn parse_val(s: &str) -> f64 {
    s.parse().unwrap_or(f64::NAN)
}
