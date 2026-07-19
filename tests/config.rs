use std::collections::HashMap;

use alpha::config::Settings;

#[test]
fn api_accepts_only_ai_credential_presence_references() {
    let values = HashMap::from([
        (
            "CONTENT_AI_CREDENTIALS_AVAILABLE",
            "OPENAI_API_KEY,SESSION_SECRET",
        ),
        ("SESSION_SECRET", "must-never-be-an-ai-provider-key"),
    ]);

    let settings = Settings::from_pairs(values).unwrap();

    assert!(settings.has_content_ai_credential("OPENAI_API_KEY"));
    assert!(!settings.has_content_ai_credential("SESSION_SECRET"));
}

#[test]
fn production_rejects_test_identity() {
    let values = HashMap::from([
        ("APP_ENV", "production"),
        ("DATABASE_URL", "postgres://alpha:alpha@db/alpha"),
        ("SESSION_SECRET", "0123456789abcdef0123456789abcdef"),
        (
            "WORKSPACE_RECEIPT_SECRET",
            "abcdef0123456789abcdef0123456789",
        ),
        ("TEST_IDENTITY_ENABLED", "true"),
    ]);

    let error = Settings::from_pairs(values).unwrap_err();

    assert_eq!(
        error.to_string(),
        "production에서는 test identity를 활성화할 수 없습니다"
    );
}

#[test]
fn production_requires_a_distinct_workspace_receipt_secret() {
    let values = HashMap::from([
        ("APP_ENV", "production"),
        ("DATABASE_URL", "postgres://alpha:alpha@db/alpha"),
        ("SESSION_SECRET", "0123456789abcdef0123456789abcdef"),
        (
            "WORKSPACE_RECEIPT_SECRET",
            "0123456789abcdef0123456789abcdef",
        ),
    ]);
    assert_eq!(
        Settings::from_pairs(values).unwrap_err().to_string(),
        "production WORKSPACE_RECEIPT_SECRET은 SESSION_SECRET과 다른 32바이트 이상 값이어야 합니다"
    );
}

#[test]
fn enabled_payments_require_a_real_provider() {
    let values = HashMap::from([
        ("APP_ENV", "development"),
        ("DATABASE_URL", "postgres://alpha:alpha@db/alpha"),
        ("SESSION_SECRET", "0123456789abcdef0123456789abcdef"),
        ("PAYMENTS_ENABLED", "true"),
        ("PAYMENT_PROVIDER", "disabled"),
    ]);

    let error = Settings::from_pairs(values).unwrap_err();

    assert_eq!(
        error.to_string(),
        "결제를 활성화하려면 payment provider가 필요합니다"
    );
}

#[test]
fn production_requires_database_url() {
    let values = HashMap::from([
        ("APP_ENV", "production"),
        ("SESSION_SECRET", "0123456789abcdef0123456789abcdef"),
    ]);

    let error = Settings::from_pairs(values).unwrap_err();

    assert_eq!(
        error.to_string(),
        "production에는 DATABASE_URL이 필요합니다"
    );
}

#[test]
fn production_rejects_short_session_secret() {
    let values = HashMap::from([
        ("APP_ENV", "production"),
        ("DATABASE_URL", "postgres://alpha:alpha@db/alpha"),
        ("SESSION_SECRET", "too-short"),
    ]);

    let error = Settings::from_pairs(values).unwrap_err();

    assert_eq!(
        error.to_string(),
        "production SESSION_SECRET은 32바이트 이상이어야 합니다"
    );
}

#[test]
fn production_oauth_requires_https_public_url() {
    let values = HashMap::from([
        ("APP_ENV", "production"),
        ("DATABASE_URL", "postgres://alpha:alpha@db/alpha"),
        ("SESSION_SECRET", "0123456789abcdef0123456789abcdef"),
        (
            "WORKSPACE_RECEIPT_SECRET",
            "abcdef0123456789abcdef0123456789",
        ),
        ("PUBLIC_BASE_URL", "http://alpha.example"),
        ("GOOGLE_CLIENT_ID", "google-client"),
        ("GOOGLE_CLIENT_SECRET", "google-secret"),
    ]);

    let error = Settings::from_pairs(values).unwrap_err();

    assert_eq!(
        error.to_string(),
        "production OAuth PUBLIC_BASE_URL은 https://로 시작해야 합니다"
    );
}

#[test]
fn oauth_rejects_partial_provider_credentials() {
    let values = HashMap::from([
        ("APP_ENV", "development"),
        ("GOOGLE_CLIENT_ID", "google-client"),
    ]);

    let error = Settings::from_pairs(values).unwrap_err();

    assert_eq!(
        error.to_string(),
        "Google OAuth client id와 secret은 함께 설정해야 합니다"
    );
}
