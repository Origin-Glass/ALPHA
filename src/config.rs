use std::collections::HashMap;

use thiserror::Error;

#[derive(Clone, Debug)]
pub struct Settings {
    pub app_env: String,
    pub database_url: String,
    pub session_secret: String,
    pub test_identity_enabled: bool,
    pub payments_enabled: bool,
    pub payment_provider: String,
    pub public_base_url: String,
    pub google_client_id: String,
    pub google_client_secret: String,
    pub github_client_id: String,
    pub github_client_secret: String,
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
    #[error("{0} OAuth client id와 secret은 함께 설정해야 합니다")]
    IncompleteOauthProvider(&'static str),
    #[error("OAuth를 설정하려면 PUBLIC_BASE_URL이 필요합니다")]
    MissingPublicBaseUrl,
    #[error("production OAuth PUBLIC_BASE_URL은 https://로 시작해야 합니다")]
    InsecureOauthPublicBaseUrl,
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
        let google_client_id = value("GOOGLE_CLIENT_ID");
        let google_client_secret = value("GOOGLE_CLIENT_SECRET");
        if google_client_id.is_empty() != google_client_secret.is_empty() {
            return Err(ConfigError::IncompleteOauthProvider("Google"));
        }
        let github_client_id = value("GITHUB_CLIENT_ID");
        let github_client_secret = value("GITHUB_CLIENT_SECRET");
        if github_client_id.is_empty() != github_client_secret.is_empty() {
            return Err(ConfigError::IncompleteOauthProvider("GitHub"));
        }
        let public_base_url = value("PUBLIC_BASE_URL");
        if public_base_url.is_empty()
            && (!google_client_id.is_empty() || !github_client_id.is_empty())
        {
            return Err(ConfigError::MissingPublicBaseUrl);
        }
        if app_env == "production"
            && (!google_client_id.is_empty() || !github_client_id.is_empty())
            && !public_base_url.starts_with("https://")
        {
            return Err(ConfigError::InsecureOauthPublicBaseUrl);
        }

        Ok(Self {
            app_env,
            database_url,
            session_secret,
            test_identity_enabled,
            payments_enabled,
            payment_provider,
            public_base_url,
            google_client_id,
            google_client_secret,
            github_client_id,
            github_client_secret,
        })
    }
}
