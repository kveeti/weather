use anyhow::{Context, Result};

#[derive(Clone, Debug)]
pub struct Config {
    pub fmi_place: String,
    pub port: u16,
    pub db_path: String,
    pub vapid_subject: String,
    pub vapid_public_key: String,
    pub vapid_private_key: String,
    pub summary_hour: u32,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Ok(Config {
            fmi_place: std::env::var("FMI_PLACE").unwrap_or_else(|_| "helsinki".to_string()),
            port: std::env::var("PORT")
                .unwrap_or_else(|_| "3000".to_string())
                .parse()
                .context("PORT must be a valid port number")?,
            db_path: std::env::var("DB_PATH").unwrap_or_else(|_| "local.db".to_string()),
            vapid_subject: std::env::var("VAPID_SUBJECT")
                .unwrap_or_else(|_| "mailto:security@veetik.com".to_string()),
            vapid_public_key: std::env::var("VAPID_PUBLIC_KEY").unwrap_or_default(),
            vapid_private_key: std::env::var("VAPID_PRIVATE_KEY").unwrap_or_default(),
            summary_hour: std::env::var("SUMMARY_HOUR")
                .unwrap_or_else(|_| "7".to_string())
                .parse()
                .context("SUMMARY_HOUR must be a number 0-23")?,
        })
    }
}
