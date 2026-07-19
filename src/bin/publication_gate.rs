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
    let status = match alpha::observability::publication_gate_status(&pool).await {
        Ok(status) => status,
        Err(_) => {
            eprintln!("publication gate could not verify rights state");
            return ExitCode::from(2);
        }
    };
    if status.blocker_count == 0 {
        println!("publication rights gate passed");
        return ExitCode::SUCCESS;
    }
    for blocker in status.blockers {
        eprintln!("content_review {}: {}", blocker.resource_id, blocker.reason);
    }
    if status.blocker_count > 20 {
        eprintln!("{} additional blockers omitted", status.blocker_count - 20);
    }
    ExitCode::FAILURE
}
