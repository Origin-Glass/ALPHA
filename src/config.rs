use std::collections::HashMap;

use thiserror::Error;

#[derive(Debug)]
pub struct Settings {
    pub app_env: String,
    pub database_url: String,
    pub session_secret: String,
    pub test_identity_enabled: bool,
    pub payments_enabled: bool,
    pub payment_provider: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("production에서는 test identity를 활성화할 수 없습니다")]
    ProductionTestIdentity,
    #[error("결제를 활성화하려면 payment provider가 필요합니다")]
    MissingPaymentProvider,
    #[error("production에는 DATABASE_URL이 필요합니다")]
    MissingDatabaseUrl,
    #[error("production SESSION_SECRET은 32바이트 이상이어야 합니다")]
    ShortSessionSecret,
}

impl Settings {
    pub fn from_pairs<K, V>(pairs: HashMap<K, V>) -> Result<Self, ConfigError>
    where
        K: AsRef<str> + Eq + std::hash::Hash,
        V: AsRef<str>,
    {
        let value = |key: &str| {
            pairs
                .iter()
                .find(|(name, _)| name.as_ref() == key)
                .map(|(_, value)| value.as_ref().to_owned())
                .unwrap_or_default()
        };
        let app_env = value("APP_ENV");
        let test_identity_enabled = value("TEST_IDENTITY_ENABLED") == "true";
        let payments_enabled = value("PAYMENTS_ENABLED") == "true";
        let payment_provider = value("PAYMENT_PROVIDER");

        if app_env == "production" && test_identity_enabled {
            return Err(ConfigError::ProductionTestIdentity);
        }
        let database_url = value("DATABASE_URL");
        if app_env == "production" && database_url.is_empty() {
            return Err(ConfigError::MissingDatabaseUrl);
        }
        let session_secret = value("SESSION_SECRET");
        if app_env == "production" && session_secret.len() < 32 {
            return Err(ConfigError::ShortSessionSecret);
        }
        if payments_enabled && (payment_provider.is_empty() || payment_provider == "disabled") {
            return Err(ConfigError::MissingPaymentProvider);
        }

        Ok(Self {
            app_env,
            database_url,
            session_secret,
            test_identity_enabled,
            payments_enabled,
            payment_provider,
        })
    }
}
