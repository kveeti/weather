use chrono::{Local, TimeZone, Timelike, Utc};
use std::collections::HashMap;
use tracing::{error, info};

use crate::{
    config::Config,
    db, electricity, notify,
    notify::VapidConfig,
    weather::{self, temp_to_radiator_setting, ForecastPoint},
};

pub fn spawn(db: db::Db, config: Config) {
    tokio::spawn(async move {
        loop {
            if let Err(e) = run_check(&db, &config).await {
                error!("Scheduler error: {e}");
            }
            let now = Local::now();
            let next_hour = (now + chrono::Duration::hours(1))
                .with_minute(2)
                .unwrap()
                .with_second(0)
                .unwrap();
            let sleep_duration = (next_hour - now)
                .to_std()
                .unwrap_or(std::time::Duration::from_secs(3600));
            info!(
                "Next scheduler run at {next_hour} (sleeping {}s)",
                sleep_duration.as_secs()
            );
            tokio::time::sleep(sleep_duration).await;
        }
    });
}

pub async fn build_daily_summary(db: &db::Db, config: &Config) -> anyhow::Result<String> {
    let forecast = weather::fetch_forecast(&config.fmi_place).await?;

    let now = Utc::now();

    let next_24h: Vec<_> = forecast
        .iter()
        .filter(|p| p.timestamp >= now && p.timestamp <= now + chrono::Duration::hours(24))
        .collect();

    let min_temp = next_24h
        .iter()
        .map(|p| p.temperature_c)
        .filter(|t| t.is_finite())
        .fold(f64::INFINITY, f64::min);

    let max_temp = next_24h
        .iter()
        .map(|p| p.temperature_c)
        .filter(|t| t.is_finite())
        .fold(f64::NEG_INFINITY, f64::max);

    let weighted_avg = ForecastPoint::weighted_avg_temperature(&forecast, 0.9, 24, 3);
    let recommended_setting = temp_to_radiator_setting(weighted_avg);

    let temp_at = |local_hour: u32| -> String {
        let target = Local::now()
            .date_naive()
            .and_hms_opt(local_hour, 0, 0)
            .unwrap();
        let target_utc = Local.from_local_datetime(&target).unwrap().to_utc();
        forecast
            .iter()
            .min_by_key(|p| (p.timestamp - target_utc).num_seconds().unsigned_abs())
            .filter(|p| {
                p.temperature_c.is_finite() && (p.timestamp - target_utc).num_hours().abs() <= 1
            })
            .map(|p| format!("{:.0}", p.temperature_c))
            .unwrap_or_else(|| "?".into())
    };
    let temp_9 = temp_at(9);
    let temp_16 = temp_at(16);

    let min_str = if min_temp.is_finite() {
        format!("{:.0}", min_temp)
    } else {
        "?".to_string()
    };
    let max_str = if max_temp.is_finite() {
        format!("{:.0}", max_temp)
    } else {
        "?".to_string()
    };

    let today_start = Local::now().date_naive().and_hms_opt(0, 0, 0).unwrap();
    let today_start_utc = Local.from_local_datetime(&today_start).unwrap().to_utc();
    let today_end_utc = today_start_utc + chrono::Duration::hours(24);
    let price_from = today_start_utc.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let price_to = today_end_utc.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let prices = db
        .get_electricity_prices(&price_from, &price_to)
        .await
        .unwrap_or_default();

    let mut hourly: HashMap<i64, (f64, usize)> = HashMap::new();
    for p in &prices {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&p.timestamp) {
            let h = dt.to_utc().timestamp() / 3600 * 3600;
            let e = hourly.entry(h).or_insert((0.0, 0));
            e.0 += p.price_cents_kwh;
            e.1 += 1;
        }
    }

    let avg_price = if !hourly.is_empty() {
        let total: f64 = hourly.values().map(|(s, c)| s / *c as f64).sum::<f64>();
        Some(total / hourly.len() as f64)
    } else {
        None
    };

    let cheapest = hourly
        .iter()
        .min_by(|a, b| {
            let avg_a = a.1 .0 / a.1 .1 as f64;
            let avg_b = b.1 .0 / b.1 .1 as f64;
            avg_a
                .partial_cmp(&avg_b)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(&h, (sum, count))| {
            let dt = chrono::DateTime::from_timestamp(h, 0).unwrap();
            let local = Local.from_utc_datetime(&dt.naive_utc());
            (sum / *count as f64, local.format("%H:%M").to_string())
        });

    let most_expensive = hourly
        .iter()
        .max_by(|a, b| {
            let avg_a = a.1 .0 / a.1 .1 as f64;
            let avg_b = b.1 .0 / b.1 .1 as f64;
            avg_a
                .partial_cmp(&avg_b)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(&h, (sum, count))| {
            let dt = chrono::DateTime::from_timestamp(h, 0).unwrap();
            let local = Local.from_utc_datetime(&dt.naive_utc());
            (sum / *count as f64, local.format("%H:%M").to_string())
        });

    let price_part = match (avg_price, &cheapest, &most_expensive) {
        (Some(avg), Some((cheap, cheap_t)), Some((exp, exp_t))) => {
            format!(
                "\nE: avg {:.1} | {:.1}@{}..{:.1}@{} snt",
                avg, cheap, cheap_t, exp, exp_t
            )
        }
        (Some(avg), _, _) => format!("\nE: avg {:.1} snt", avg),
        _ => String::new(),
    };

    let current_setting = db.get_radiator_setting().await.ok().flatten();
    let radiator_part = if recommended_setting.is_finite() {
        let already_set = current_setting
            .map(|c| (c - recommended_setting).abs() < 0.01)
            .unwrap_or(false);
        if already_set {
            String::new()
        } else {
            let label = if recommended_setting == 0.0 {
                "off"
            } else if recommended_setting == 3.5 {
                "3.5"
            } else {
                "2"
            };
            format!(" | Radiator → {}", label)
        }
    } else {
        String::new()
    };

    Ok(format!(
        "W: {}..{} | {}..{}{}{radiator_part}",
        min_str, max_str, temp_9, temp_16, price_part
    ))
}

async fn run_check(db: &db::Db, config: &Config) -> anyhow::Result<()> {
    let needs_fetch = match db.get_latest_electricity_timestamp().await {
        Ok(Some(latest)) => match chrono::DateTime::parse_from_rfc3339(&latest) {
            Ok(latest_dt) => {
                let hours_ahead = (latest_dt.to_utc() - Utc::now()).num_hours();
                info!("Latest electricity price is {hours_ahead}h ahead");
                hours_ahead < 12
            }
            Err(_) => true,
        },
        _ => true,
    };
    if needs_fetch {
        match electricity::fetch_eprices().await {
            Ok(prices) => {
                info!("Fetched {} electricity price entries", prices.len());
                if let Err(e) = db.upsert_electricity_prices(&prices).await {
                    error!("Failed to upsert electricity prices: {e}");
                }
            }
            Err(e) => {
                error!("Failed to fetch electricity prices: {e}");
            }
        }
    }

    info!("Scheduler: fetching forecast for {}", config.fmi_place);

    let forecast = match weather::fetch_forecast(&config.fmi_place).await {
        Ok(f) => f,
        Err(e) => {
            info!("Failed to fetch forecast: {e}");
            return Ok(());
        }
    };

    let now = Utc::now();
    let today = now.with_timezone(&Local).date_naive();

    let next_24h: Vec<_> = forecast
        .iter()
        .filter(|p| p.timestamp >= now && p.timestamp <= now + chrono::Duration::hours(24))
        .collect();

    let min_temp = next_24h
        .iter()
        .map(|p| p.temperature_c)
        .filter(|t| t.is_finite())
        .fold(f64::INFINITY, f64::min);

    let max_temp = next_24h
        .iter()
        .map(|p| p.temperature_c)
        .filter(|t| t.is_finite())
        .fold(f64::NEG_INFINITY, f64::max);

    let weighted_avg = ForecastPoint::weighted_avg_temperature(&forecast, 0.9, 24, 3);
    let recommended_setting = temp_to_radiator_setting(weighted_avg);

    tracing::debug!(
        "min_temp {min_temp}, max_temp {max_temp}, weighted_avg {weighted_avg:.1}, recommended_setting {recommended_setting:.2}"
    );

    let subscriptions = db.list_subscriptions().await?;

    if subscriptions.is_empty() {
        info!("No push subscribers, skipping notifications");
    }

    let vapid = VapidConfig {
        subject: config.vapid_subject.clone(),
        public_key_b64: config.vapid_public_key.clone(),
        private_key_b64: config.vapid_private_key.clone(),
    };

    // Daily summary
    let local_hour = Local::now().hour();
    if local_hour == config.summary_hour {
        let summary_key = "daily_summary";
        let already_sent = db.already_notified(summary_key, today).await?;
        if !already_sent {
            let message = build_daily_summary(db, config).await?;
            info!("Sending daily summary: {message}");
            let results = notify::send_all(&subscriptions, &message, &vapid).await;
            let success_count = results.iter().filter(|r| r.is_ok()).count();
            info!(
                "Daily summary sent to {}/{} subscribers",
                success_count,
                subscriptions.len()
            );
            db.log_notification(summary_key, today).await?;
        }
    }

    // Radiator adjustment check
    if recommended_setting.is_finite() {
        let current_setting = db.get_radiator_setting().await?;
        let diff = if let Some(current) = current_setting {
            (recommended_setting - current).abs()
        } else {
            f64::INFINITY
        };

        if diff >= 0.5 {
            let radiator_key = format!("radiator_{:.1}", recommended_setting.round());
            let already_sent = db.already_notified(&radiator_key, today).await?;
            if !already_sent {
                let current_str = current_setting
                    .map(|c| format!("{:.1}", c))
                    .unwrap_or_else(|| "unknown".to_string());
                let message = format!(
                    "Radiator: {:.1} → {:.1} (avg {:.0}°C next 24h)",
                    current_str, recommended_setting, weighted_avg
                );
                info!("Sending radiator notification: {message}");
                let results = notify::send_all(&subscriptions, &message, &vapid).await;
                let success_count = results.iter().filter(|r| r.is_ok()).count();
                info!(
                    "Radiator notification sent to {}/{} subscribers",
                    success_count,
                    subscriptions.len()
                );
                db.log_notification(&radiator_key, today).await?;
            }
        }
    }

    Ok(())
}
