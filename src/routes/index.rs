use axum::{
    extract::{Form, State},
    response::{Html, Redirect},
};
use chrono::{Local, TimeZone, Timelike, Utc};
use hypertext::prelude::*;
use std::collections::HashMap;

use crate::{
    weather::{self, temp_to_radiator_setting, ForecastPoint},
    AppState,
};

pub async fn handler(State(state): State<AppState>) -> Html<String> {
    let forecast = match weather::fetch_forecast(&state.config.fmi_place).await {
        Ok(f) => f,
        Err(e) => {
            return Html(error_page(&format!("Failed to fetch forecast: {e}")));
        }
    };

    let sub_count = match state.db.list_subscriptions().await {
        Ok(s) => s.len(),
        Err(_) => 0,
    };

    let now = Utc::now();
    let next_24h: Vec<_> = forecast
        .iter()
        .filter(|p| p.timestamp >= now && p.timestamp <= now + chrono::Duration::hours(24))
        .collect();

    let min_temp = next_24h
        .iter()
        .filter(|p| p.temperature_c.is_finite())
        .map(|p| p.temperature_c)
        .fold(f64::INFINITY, f64::min);

    let max_temp = next_24h
        .iter()
        .filter(|p| p.temperature_c.is_finite())
        .map(|p| p.temperature_c)
        .fold(f64::NEG_INFINITY, f64::max);

    let avg_temp = {
        let (sum, count) = next_24h
            .iter()
            .filter(|p| {
                if !p.temperature_c.is_finite() {
                    return false;
                }
                let local_hour = p.timestamp.with_timezone(&Local).hour();
                (8..20).contains(&local_hour)
            })
            .fold((0.0, 0usize), |(s, c), p| (s + p.temperature_c, c + 1));
        if count > 0 {
            sum / count as f64
        } else {
            f64::NAN
        }
    };

    let weighted_avg = ForecastPoint::weighted_avg_temperature(&forecast, 0.9, 24, 3);
    let recommended_setting = temp_to_radiator_setting(weighted_avg);
    let current_radiator = state.db.get_radiator_setting().await.ok().flatten();

    let place = &state.config.fmi_place;

    // Electricity prices — load from start of today (local) through forecast window
    let today_start = Local::now().date_naive().and_hms_opt(0, 0, 0).unwrap();
    let today_start_utc = Local.from_local_datetime(&today_start).unwrap().to_utc();
    let price_from = today_start_utc.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let price_to = (now + chrono::Duration::hours(25))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let electricity_prices = state
        .db
        .get_electricity_prices(&price_from, &price_to)
        .await
        .unwrap_or_default();

    // Current price: find the 15-min slot containing now
    let now_ts = now.timestamp();
    let current_slot = now_ts - (now_ts % 900); // round down to 15-min
    let current_price = electricity_prices.iter().find_map(|p| {
        let dt = chrono::DateTime::parse_from_rfc3339(&p.timestamp).ok()?;
        if dt.to_utc().timestamp() == current_slot {
            Some(p.price_cents_kwh)
        } else {
            None
        }
    });

    // Cheapest hour today (average of 15-min slots per hour, only remaining hours)
    let mut hourly_prices: HashMap<i64, (f64, usize)> = HashMap::new();
    for p in &electricity_prices {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&p.timestamp) {
            let hour_ts = dt.to_utc().timestamp() - (dt.to_utc().timestamp() % 3600);
            let entry = hourly_prices.entry(hour_ts).or_insert((0.0, 0));
            entry.0 += p.price_cents_kwh;
            entry.1 += 1;
        }
    }

    let today_end_utc = today_start_utc + chrono::Duration::hours(24);
    let today_hours: HashMap<_, _> = hourly_prices
        .iter()
        .filter(|(&h, _)| h >= today_start_utc.timestamp() && h < today_end_utc.timestamp())
        .map(|(&h, v)| (h, *v))
        .collect();

    let avg_price = if !today_hours.is_empty() {
        let total: f64 = today_hours
            .values()
            .map(|(s, c)| s / *c as f64)
            .sum::<f64>();
        Some(total / today_hours.len() as f64)
    } else {
        None
    };

    let cheapest_today = today_hours
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

    let most_expensive_today = today_hours
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

    Html(rsx! {
        <!DOCTYPE html>
        <html lang="en">
        <head>
            <meta charset="UTF-8">
            <meta name="viewport" content="width=device-width, initial-scale=1.0">
            <title> "Weather – " (place) </title>
            <link rel="manifest" href="/manifest.json">
            <meta name="theme-color" content="#000">
            <link rel="stylesheet" href="/assets/styles.css">
            <script src="/assets/script.js" defer></script>
        </head>
        <body class="bg-gray-1 text-gray-12 text-sm p-4 max-w-[37.5rem] mx-auto">
            <div class=" text-sm mb-2">
                @let min_s = if min_temp.is_finite() { format!("{:.0}", min_temp) } else { "-".into() };
                @let max_s = if max_temp.is_finite() { format!("{:.0}", max_temp) } else { "-".into() };
                @let avg_s = if avg_temp.is_finite() { format!("{:.0}", avg_temp) } else { "-".into() };
                <p class="mb-0.5"> <span class="bg-gray-a5 px-0.5 -mx-0.5"> (avg_s) "°C" </span> " avg, " (min_s) ".." (max_s) "°C" </p>

                @let current_s = current_price.map(|p| format!("{:.1}", p)).unwrap_or_else(|| "-".into());
                @let avg_p_s = avg_price.map(|p| format!("{:.1}", p)).unwrap_or_else(|| "-".into());
                @let range_s = match (&cheapest_today, &most_expensive_today) {
                    (Some((cheap, cheap_t)), Some((exp, exp_t))) => {
                        format!("{:.1}@{}..{:.1}@{}", cheap, cheap_t, exp, exp_t)
                    }
                    _ => "-".into()
                };
                <p> <span class="bg-gray-a5 px-0.5 -mx-0.5"> (current_s) " snt" </span> " now, avg " (avg_p_s) " | " (range_s) " snt" </p>
            </div>
            <p class="text-gray-11 text-xs"> "Location: " (place) " · " (sub_count) " push subscriber(s)" </p>

            <h2 class="text-base mt-8 mb-2 text-gray-12">Forecast</h2>
            <div class="overflow-x-auto">
                <table class="w-full text-sm">
                    <thead>
                        <tr class="bg-gray-3">
                            <th class="px-3 py-2 text-left font-medium text-gray-11">Time</th>
                            <th class="px-3 py-2 text-left font-medium text-gray-11">Temp</th>
                            <th class="px-3 py-2 text-left font-medium text-gray-11">Wind</th>
                            <th class="px-3 py-2 text-left font-medium text-gray-11">Precip</th>
                            <th class="px-3 py-2 text-left font-medium text-gray-11">"E.Price"</th>
                        </tr>
                    </thead>
                    <tbody>
                        @for p in next_24h.iter().step_by(3) {
                            @let local_dt = Local.from_utc_datetime(&p.timestamp.naive_utc());
                            @let time_str = local_dt.format("%a %H:%M").to_string();
                            @let temp = if p.temperature_c.is_finite() {
                                format!("{:.1}", p.temperature_c)
                            } else {
                                "-".to_string()
                            };
                            @let wind = if p.wind_speed_ms.is_finite() {
                                format!("{:.1}", p.wind_speed_ms)
                            } else {
                                "-".to_string()
                            };
                            @let precip = if p.precipitation_mm.is_finite() {
                                format!("{:.1}", p.precipitation_mm)
                            } else {
                                "-".to_string()
                            };
                            @let hour_ts = p.timestamp.timestamp() - (p.timestamp.timestamp() % 3600);
                            @let price = hourly_prices
                                .get(&hour_ts)
                                .map(|(sum, count)| format!("{:.1}", sum / *count as f64))
                                .unwrap_or_else(|| "-".to_string());
                            <tr class="even:bg-gray-2">
                                <td class="px-3 py-2"> (time_str) </td>
                                <td class="px-3 py-2"> (format!("{}°C", temp)) </td>
                                <td class="px-3 py-2"> (format!("{} m/s", wind)) </td>
                                <td class="px-3 py-2"> (format!("{} mm", precip)) </td>
                                <td class="px-3 py-2"> (format!("{} snt", price)) </td>
                            </tr>
                        }
                    </tbody>
                </table>
            </div>

            <form method="POST" action="/radiator" class="mt-8">
                <h2 class="mb-2 text-gray-12 text-base">
                    "Radiator Setting"

                    @if recommended_setting.is_finite() {
                        @let needs_adjust = current_radiator.map(|c| (c - recommended_setting).abs() >= 0.3).unwrap_or(false);
                        @let rad_style = if needs_adjust { "text-white bg-red-a9 p-1 -m-1 ms-1 text-sm" } else { "ms-1.5 text-gray-11 text-sm" };
                        @let rad_text = if needs_adjust { format!("adjust to → {:.1}", recommended_setting) } else { format!("ideal {:.1}", recommended_setting) };
                        <span class=(rad_style)> (rad_text) </span>
                    }
                </h2>
                <div class="flex gap-2 text-sm">
                    @for (i, v) in [0.0_f64, 2.0, 3.5].iter().enumerate() {
                        @let label = if *v == 0.0 { "Off" } else { &v.to_string() };
                        @let base_classes = "focus flex-1 py-3 px-4 bg-gray-a4 text-gray-12 font-medium".to_owned();
                        @let is_active_setting = current_radiator
                            .map(|c| (c - v).abs() < 0.01)
                            .unwrap_or(false);
                        <button
                            type="submit"
                            name="radiator"
                            value=(format!("{}", v))
                            class=(base_classes + (if is_active_setting { " bg-gray-a8" } else { "" }))
                        >
                            (label)
                        </button>
                    }
                </div>
            </form>

            <div class="flex gap-2 mt-8 flex-wrap">
                <button id="push-btn" onclick="subscribePush()" class="bg-gray-a4 text-gray-12 px-4 py-2 border-none text-sm">Enable Push Notifications</button>
                <button onclick="testSummary(this)" class="bg-gray-a4 text-gray-12 px-4 py-2 border-none text-sm">Test Daily Notif</button>
            </div>
            <div id="push-status" class="text-xs text-gray-11 mt-2"></div>
        </body>
        </html>
    }.render().into_inner())
}

fn error_page(msg: &str) -> String {
    rsx! {
        <!DOCTYPE html>
        <html>
            <head>
                <meta charset="UTF-8">
                <title>Error</title>
                <link rel="stylesheet" href="/output.css">
            </head>
            <body class="bg-gray-12 text-gray-5 p-8 font-system-ui">
                <h1 class="text-xl">Error</h1>
                <p> (msg) </p>
                <a href="/" class="text-gray-12">Retry</a>
            </body>
        </html>
    }
    .render()
    .into_inner()
}

pub async fn radiator_handler(
    State(state): State<AppState>,
    Form(form): Form<HashMap<String, String>>,
) -> Redirect {
    if let Some(val) = form.get("radiator").and_then(|v| v.parse::<f64>().ok()) {
        // Only allow valid settings: 0 (off), 2.0, or 3.5
        let val = if val >= 2.75 {
            3.5
        } else if val >= 1.0 {
            2.0
        } else {
            0.0
        };
        let _ = state.db.set_radiator_setting(val).await;
    }
    Redirect::to("/")
}
