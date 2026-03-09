use anyhow::Result;
use chrono::NaiveDate;
use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};

use crate::weather::ForecastPoint;

#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn init_db(db_path: &str) -> Result<Self> {
        let url = format!("sqlite://{}?mode=rwc", db_path);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&url)
            .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS subscriptions (
                id       INTEGER PRIMARY KEY,
                endpoint TEXT NOT NULL UNIQUE,
                p256dh   TEXT NOT NULL,
                auth     TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS notification_log (
                id          INTEGER PRIMARY KEY,
                kind        TEXT NOT NULL,
                sent_date   TEXT NOT NULL,
                UNIQUE(kind, sent_date)
            )",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS radiator_setting (
                id          INTEGER PRIMARY KEY CHECK (id = 1),
                setting     REAL NOT NULL,
                updated_at  TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS electricity_prices (
                timestamp       TEXT NOT NULL PRIMARY KEY,
                price_cents_kwh REAL NOT NULL
            )",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS weather_observations (
                timestamp        TEXT NOT NULL PRIMARY KEY,
                temperature_c    REAL NOT NULL,
                wind_speed_ms    REAL,
                precipitation_mm REAL,
                humidity         REAL,
                wind_direction   REAL
            )",
        )
        .execute(&pool)
        .await?;

        Ok(Self::new(pool))
    }

    // --- Subscriptions ---

    pub async fn insert_subscription(
        &self,
        endpoint: &str,
        p256dh: &str,
        auth: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO subscriptions (endpoint, p256dh, auth) VALUES (?, ?, ?)",
        )
        .bind(endpoint)
        .bind(p256dh)
        .bind(auth)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_subscription(&self, endpoint: &str) -> Result<()> {
        sqlx::query("DELETE FROM subscriptions WHERE endpoint = ?")
            .bind(endpoint)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn list_subscriptions(&self) -> Result<Vec<Subscription>> {
        let rows =
            sqlx::query_as::<_, Subscription>("SELECT endpoint, p256dh, auth FROM subscriptions")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows)
    }

    // --- Notification log ---

    pub async fn already_notified(&self, kind: &str, date: NaiveDate) -> Result<bool> {
        let date_str = date.format("%Y-%m-%d").to_string();
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM notification_log WHERE kind = ? AND sent_date = ?")
                .bind(kind)
                .bind(&date_str)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.is_some())
    }

    pub async fn log_notification(&self, kind: &str, date: NaiveDate) -> Result<()> {
        let date_str = date.format("%Y-%m-%d").to_string();
        sqlx::query("INSERT OR IGNORE INTO notification_log (kind, sent_date) VALUES (?, ?)")
            .bind(kind)
            .bind(&date_str)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // --- Radiator setting ---

    pub async fn get_radiator_setting(&self) -> Result<Option<f64>> {
        let row: Option<(f64,)> =
            sqlx::query_as("SELECT setting FROM radiator_setting WHERE id = 1")
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|r| r.0))
    }

    pub async fn set_radiator_setting(&self, setting: f64) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO radiator_setting (id, setting, updated_at) VALUES (1, ?, ?)
             ON CONFLICT(id) DO UPDATE SET setting = excluded.setting, updated_at = excluded.updated_at",
        )
        .bind(setting)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // --- Electricity prices ---

    pub async fn upsert_electricity_prices(&self, prices: &[(String, f64)]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for (ts, price) in prices {
            sqlx::query(
                "INSERT OR REPLACE INTO electricity_prices (timestamp, price_cents_kwh) VALUES (?, ?)",
            )
            .bind(ts)
            .bind(price)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn get_latest_electricity_timestamp(&self) -> Result<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT MAX(timestamp) FROM electricity_prices")
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.and_then(|r| if r.0.is_empty() { None } else { Some(r.0) }))
    }

    pub async fn get_electricity_prices(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<ElectricityPrice>> {
        let rows = sqlx::query_as::<_, ElectricityPrice>(
            "SELECT timestamp, price_cents_kwh FROM electricity_prices WHERE timestamp >= ? AND timestamp < ? ORDER BY timestamp",
        )
        .bind(from)
        .bind(to)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
    // --- Weather observations ---

    pub async fn upsert_weather_observations(&self, points: &[ForecastPoint]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for p in points {
            let ts = p.timestamp.format("%Y-%m-%dT%H:%M:%SZ").to_string();
            sqlx::query(
                "INSERT OR REPLACE INTO weather_observations (timestamp, temperature_c, wind_speed_ms, precipitation_mm, humidity, wind_direction) VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(&ts)
            .bind(p.temperature_c)
            .bind(finite_or_none(p.wind_speed_ms))
            .bind(finite_or_none(p.precipitation_mm))
            .bind(finite_or_none(p.humidity))
            .bind(finite_or_none(p.wind_direction))
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Merge wind data from a secondary station into existing observations.
    /// Only updates wind columns for timestamps that already exist.
    pub async fn merge_wind_observations(&self, points: &[ForecastPoint]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for p in points {
            let ts = p.timestamp.format("%Y-%m-%dT%H:%M:%SZ").to_string();
            let ws = finite_or_none(p.wind_speed_ms);
            let wd = finite_or_none(p.wind_direction);
            if ws.is_some() || wd.is_some() {
                sqlx::query(
                    "UPDATE weather_observations SET wind_speed_ms = COALESCE(?, wind_speed_ms), wind_direction = COALESCE(?, wind_direction) WHERE timestamp = ?",
                )
                .bind(ws)
                .bind(wd)
                .bind(&ts)
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn get_latest_observation_timestamp(&self) -> Result<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT MAX(timestamp) FROM weather_observations")
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.and_then(|r| if r.0.is_empty() { None } else { Some(r.0) }))
    }

    pub async fn get_weather_observations(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<WeatherObservation>> {
        let rows = sqlx::query_as::<_, WeatherObservation>(
            "SELECT timestamp, temperature_c, wind_speed_ms, precipitation_mm, humidity, wind_direction FROM weather_observations WHERE timestamp >= ? AND timestamp < ? ORDER BY timestamp",
        )
        .bind(from)
        .bind(to)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}

// --- Types ---

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Subscription {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ElectricityPrice {
    pub timestamp: String,
    pub price_cents_kwh: f64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WeatherObservation {
    pub timestamp: String,
    pub temperature_c: f64,
    pub wind_speed_ms: f64,
    pub precipitation_mm: f64,
    pub humidity: f64,
    pub wind_direction: f64,
}

fn finite_or_none(v: f64) -> Option<f64> {
    if v.is_finite() { Some(v) } else { None }
}
