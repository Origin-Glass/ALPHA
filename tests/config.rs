use std::collections::HashMap;

use alpha::config::Settings;

#[test]
fn production_rejects_test_identity() {
    let values = HashMap::from([
        ("APP_ENV", "production"),
        ("DATABASE_URL", "postgres://alpha:alpha@db/alpha"),
        ("SESSION_SECRET", "0123456789abcdef0123456789abcdef"),
        ("TEST_IDENTITY_ENABLED", "true"),
    ]);

    let error = Settings::from_pairs(values).unwrap_err();

    assert_eq!(
        error.to_string(),
        "production에서는 test identity를 활성화할 수 없습니다"
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
