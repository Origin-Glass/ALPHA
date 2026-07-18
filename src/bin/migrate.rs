use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database_url = env::var("DATABASE_URL")?;
    let pool = alpha::db::connect(&database_url).await?;
    alpha::db::migrate(&pool).await?;
    println!("ALPHA 데이터베이스 마이그레이션 완료");
    Ok(())
}
