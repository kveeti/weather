use anyhow::Result;
use chrono::NaiveDate;
use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};

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
