use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let Ok(database_url) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL is required");
        return ExitCode::from(2);
    };
    let Ok(pool) = alpha::db::connect(&database_url).await else {
        eprintln!("publication gate could not connect to database");
        return ExitCode::from(2);
    };
    let violations = match alpha::observability::publication_violations(&pool).await {
        Ok(violations) => violations,
        Err(_) => {
            eprintln!("publication gate could not verify rights state");
            return ExitCode::from(2);
        }
    };
    if violations.is_empty() {
        println!("publication rights gate passed");
        return ExitCode::SUCCESS;
    }
    for violation in violations {
        eprintln!(
            "{} {}: {}",
            violation.resource_kind, violation.resource_id, violation.reason
        );
    }
    ExitCode::FAILURE
}
