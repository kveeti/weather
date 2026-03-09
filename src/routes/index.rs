use axum::{
    extract::{Form, State},
    response::{Html, Redirect},
};
use chrono::{Local, NaiveDate, TimeZone, Utc};
use hypertext::prelude::*;
use std::collections::{BTreeMap, HashMap};

use crate::{
    weather::{self, temp_to_radiator_setting, ForecastPoint},
    AppState,
};

struct HourRow {
    timestamp: chrono::DateTime<Utc>,
    temperature_c: f64,
    wind_speed_ms: f64,
    precipitation_mm: f64,
}

struct DayGroup {
    date: NaiveDate,
    label: String,
    rows: Vec<HourRow>,
    min_temp: f64,
    max_temp: f64,
    total_precip: f64,
    avg_wind: f64,
    avg_price: f64,
}

pub async fn handler(State(state): State<AppState>) -> Html<String> {
    let forecast = match weather::fetch_forecast(&state.config.fmi_sid).await {
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
    let today = now.with_timezone(&Local).date_naive();
    let tomorrow = today + chrono::Duration::days(1);

    // Load observations from DB, fetch on-demand if empty
    let obs_from = (now - chrono::Duration::days(7))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let obs_to = now.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let mut observations = state
        .db
        .get_weather_observations(&obs_from, &obs_to)
        .await
        .unwrap_or_default();
    tracing::info!(
        "Observations from DB: {} (from={}, to={})",
        observations.len(),
        obs_from,
        obs_to
    );
    if observations.is_empty() {
        match weather::fetch_observations(&state.config.fmi_sid).await {
            Ok(points) => {
                tracing::info!("Fetched {} observation points from FMI", points.len());
                if let Err(e) = state.db.upsert_weather_observations(&points).await {
                    tracing::error!("Failed to upsert observations: {e}");
                }
                if let Some(wind_sid) = &state.config.fmi_sid_wind {
                    match weather::fetch_observations(wind_sid).await {
                        Ok(wind_points) => {
                            if let Err(e) = state.db.merge_wind_observations(&wind_points).await {
                                tracing::error!("Failed to merge wind observations: {e}");
                            }
                        }
                        Err(e) => {
                            tracing::error!("Failed to fetch wind observations: {e}");
                        }
                    }
                }
                observations = state
                    .db
                    .get_weather_observations(&obs_from, &obs_to)
                    .await
                    .unwrap_or_default();
                tracing::info!("After upsert, observations from DB: {}", observations.len());
            }
            Err(e) => {
                tracing::error!("Failed to fetch observations from FMI: {e}");
            }
        }
    }

    // Merge observations + forecast into a BTreeMap keyed by hour timestamp.
    // Observations take priority on overlap.
    let mut timeline: BTreeMap<i64, HourRow> = BTreeMap::new();

    // Insert forecast first
    for p in &forecast {
        let hour_ts = p.timestamp.timestamp() - (p.timestamp.timestamp() % 3600);
        timeline.insert(
            hour_ts,
            HourRow {
                timestamp: p.timestamp,
                temperature_c: p.temperature_c,
                wind_speed_ms: p.wind_speed_ms,
                precipitation_mm: p.precipitation_mm,
            },
        );
    }

    // Overwrite with observations (they win on overlap)
    for o in &observations {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&o.timestamp) {
            let utc = dt.to_utc();
            let hour_ts = utc.timestamp() - (utc.timestamp() % 3600);
            timeline.insert(
                hour_ts,
                HourRow {
                    timestamp: utc,
                    temperature_c: o.temperature_c,
                    wind_speed_ms: o.wind_speed_ms,
                    precipitation_mm: o.precipitation_mm,
                },
            );
        }
    }

    // Group by local date
    let mut day_map: BTreeMap<NaiveDate, Vec<HourRow>> = BTreeMap::new();
    for (_, row) in timeline {
        let local_date = row.timestamp.with_timezone(&Local).date_naive();
        day_map.entry(local_date).or_default().push(row);
    }

    // Electricity prices — cover observations + forecast window
    let price_from = (now - chrono::Duration::days(7))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let price_to = (now + chrono::Duration::hours(73))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let electricity_prices = state
        .db
        .get_electricity_prices(&price_from, &price_to)
        .await
        .unwrap_or_default();

    // Hourly prices for table display
    let mut hourly_prices: HashMap<i64, (f64, usize)> = HashMap::new();
    for p in &electricity_prices {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&p.timestamp) {
            let hour_ts = dt.to_utc().timestamp() - (dt.to_utc().timestamp() % 3600);
            let entry = hourly_prices.entry(hour_ts).or_insert((0.0, 0));
            entry.0 += p.price_cents_kwh;
            entry.1 += 1;
        }
    }

    // Build day groups with summaries
    let day_groups: Vec<DayGroup> = day_map
        .into_iter()
        .map(|(date, rows)| {
            let min_temp = rows
                .iter()
                .filter(|r| r.temperature_c.is_finite())
                .map(|r| r.temperature_c)
                .fold(f64::INFINITY, f64::min);
            let max_temp = rows
                .iter()
                .filter(|r| r.temperature_c.is_finite())
                .map(|r| r.temperature_c)
                .fold(f64::NEG_INFINITY, f64::max);
            let total_precip: f64 = rows
                .iter()
                .filter(|r| r.precipitation_mm.is_finite())
                .map(|r| r.precipitation_mm)
                .sum();
            let wind_vals: Vec<f64> = rows
                .iter()
                .filter(|r| r.wind_speed_ms.is_finite())
                .map(|r| r.wind_speed_ms)
                .collect();
            let avg_wind = if wind_vals.is_empty() {
                f64::NAN
            } else {
                wind_vals.iter().sum::<f64>() / wind_vals.len() as f64
            };
            let day_prices: Vec<f64> = rows
                .iter()
                .filter_map(|r| {
                    let hour_ts = r.timestamp.timestamp() - (r.timestamp.timestamp() % 3600);
                    hourly_prices
                        .get(&hour_ts)
                        .map(|(sum, count)| sum / *count as f64)
                })
                .collect();
            let avg_price = if day_prices.is_empty() {
                f64::NAN
            } else {
                day_prices.iter().sum::<f64>() / day_prices.len() as f64
            };
            let label = format!("{}", date.format("%a %-d %b"));
            DayGroup {
                date,
                label,
                rows,
                min_temp,
                max_temp,
                total_precip,
                avg_wind,
                avg_price,
            }
        })
        .collect();

    // Next 24h stats for the summary header
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
            .filter(|p| p.temperature_c.is_finite())
            .fold((0.0, 0usize), |(s, c), p| (s + p.temperature_c, c + 1));
        if count > 0 {
            sum / count as f64
        } else {
            f64::NAN
        }
    };

    let current_temp = forecast
        .iter()
        .filter(|p| p.temperature_c.is_finite())
        .min_by_key(|p| (p.timestamp - now).num_seconds().unsigned_abs())
        .map(|p| p.temperature_c)
        .unwrap_or(f64::NAN);

    let weighted_avg = ForecastPoint::weighted_avg_temperature(&forecast, 0.9, 24, 3);
    let recommended_setting = temp_to_radiator_setting(weighted_avg);
    let current_radiator = state.db.get_radiator_setting().await.ok().flatten();

    let place = &state.config.fmi_sid;

    // Current price: find the 15-min slot containing now
    let now_ts = now.timestamp();
    let current_slot = now_ts - (now_ts % 900);
    let current_price = electricity_prices.iter().find_map(|p| {
        let dt = chrono::DateTime::parse_from_rfc3339(&p.timestamp).ok()?;
        if dt.to_utc().timestamp() == current_slot {
            Some(p.price_cents_kwh)
        } else {
            None
        }
    });

    // Today's price stats
    let today_start = Local::now().date_naive().and_hms_opt(0, 0, 0).unwrap();
    let today_start_utc = Local.from_local_datetime(&today_start).unwrap().to_utc();
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
                @let cur_s = if current_temp.is_finite() { format!("{:.0}", current_temp) } else { "-".into() };
                @let avg_s = if avg_temp.is_finite() { format!("{:.0}", avg_temp) } else { "-".into() };
                <p class="mb-0.5"> <span class="bg-gray-a5 px-0.5 -mx-0.5"> (cur_s) "°C" </span> " now, avg " (avg_s) " | " (min_s) ".." (max_s) "°C" </p>

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
            <p class="text-gray-11 text-xs mb-4"> "Location: " (place) " · " (sub_count) " push subscriber(s)" </p>

            <div class="grid grid-cols-[max-content_1fr_1fr_1fr_1fr] gap-x-2">
                @for (idx, day) in day_groups.iter().enumerate() {
                    @let is_open = day.date == today || day.date == tomorrow;
                    @let min_s = if day.min_temp.is_finite() { format!("{:.0}", day.min_temp) } else { "-".into() };
                    @let max_s = if day.max_temp.is_finite() { format!("{:.0}", day.max_temp) } else { "-".into() };
                    @let precip_s = format!("{:.1}", day.total_precip);
                    @let wind_s = if day.avg_wind.is_finite() { format!("{:.0}", day.avg_wind) } else { "-".into() };
                    @let price_s = if day.avg_price.is_finite() { format!("{:.1}", day.avg_price) } else { "-".into() };
                    @let is_today = day.date == today;
                    @let day_label = if is_today { format!("Today") } else { day.label.clone() };
                    @let panel_id = format!("day-{idx}");
                    <div class="col-span-5 grid grid-cols-subgrid cursor-pointer py-2 px-3 mb-1 bg-gray-3 text-gray-12 text-sm font-medium select-none whitespace-nowrap"
                         onclick=(format!("document.getElementById('{panel_id}').toggleAttribute('hidden')"))>
                        <span> (day_label) </span>
                        <span class="text-gray-11 font-normal"> (min_s) ".." (max_s) "°C" </span>
                        <span class="text-gray-11 font-normal"> (wind_s) " m/s" </span>
                        <span class="text-gray-11 font-normal"> (precip_s) " mm" </span>
                        <span class="text-gray-11 font-normal"> (price_s) " snt" </span>
                    </div>
                    <div id=(panel_id) class="col-span-5 overflow-x-auto" hidden=[(!is_open).then_some("")]>
                        <table class="w-full text-sm">
                            <thead>
                                <tr class="bg-gray-2">
                                    <th class="px-3 py-1.5 text-left font-medium text-gray-11">Time</th>
                                    <th class="px-3 py-1.5 text-left font-medium text-gray-11">Temp</th>
                                    <th class="px-3 py-1.5 text-left font-medium text-gray-11">Wind</th>
                                    <th class="px-3 py-1.5 text-left font-medium text-gray-11">Precip</th>
                                    <th class="px-3 py-1.5 text-left font-medium text-gray-11">"E.Price"</th>
                                </tr>
                            </thead>
                            <tbody>
                                @for row in &day.rows {
                                    @let local_dt = Local.from_utc_datetime(&row.timestamp.naive_utc());
                                    @let time_str = local_dt.format("%H:%M").to_string();
                                    @let temp = if row.temperature_c.is_finite() {
                                        format!("{:.1}", row.temperature_c)
                                    } else {
                                        "-".to_string()
                                    };
                                    @let wind = if row.wind_speed_ms.is_finite() {
                                        format!("{:.1}", row.wind_speed_ms)
                                    } else {
                                        "-".to_string()
                                    };
                                    @let precip = if row.precipitation_mm.is_finite() {
                                        format!("{:.1}", row.precipitation_mm)
                                    } else {
                                        "-".to_string()
                                    };
                                    @let hour_ts = row.timestamp.timestamp() - (row.timestamp.timestamp() % 3600);
                                    @let price = hourly_prices
                                        .get(&hour_ts)
                                        .map(|(sum, count)| format!("{:.1}", sum / *count as f64))
                                        .unwrap_or_else(|| "-".to_string());
                                    <tr class="even:bg-gray-2">
                                        <td class="px-3 py-1.5"> (time_str) </td>
                                        <td class="px-3 py-1.5"> (format!("{}°C", temp)) </td>
                                        <td class="px-3 py-1.5"> (format!("{} m/s", wind)) </td>
                                        <td class="px-3 py-1.5"> (format!("{} mm", precip)) </td>
                                        <td class="px-3 py-1.5"> (format!("{} snt", price)) </td>
                                    </tr>
                                }
                            </tbody>
                        </table>
                    </div>
                }
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
                    @for (_, v) in [0.0_f64, 2.0, 3.5].iter().enumerate() {
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

            <div class="mt-8">
                <a href="/" class="bg-gray-a4 text-gray-12 px-4 py-2 text-sm inline-block no-underline">Refresh</a>
            </div>
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
