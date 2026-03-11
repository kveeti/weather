use anyhow::{Context, Result};
use chrono_tz::Tz;

#[derive(Clone, Debug)]
pub struct Config {
    pub fmi_sid: String,
    pub fmi_sid_wind: Option<String>,
    pub port: u16,
    pub db_path: String,
    pub vapid_subject: String,
    pub vapid_public_key: String,
    pub vapid_private_key: String,
    pub summary_hour: u32,
    pub tz: Tz,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Ok(Config {
            fmi_sid: std::env::var("FMI_SID").unwrap_or_else(|_| "101799".to_string()),
            fmi_sid_wind: std::env::var("FMI_SID_WIND").ok(),
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
            tz: std::env::var("TZ")
                .unwrap_or_else(|_| "Europe/Helsinki".to_string())
                .parse()
                .context("TZ must be a valid IANA timezone (e.g. Europe/Helsinki)")?,
        })
    }
}
