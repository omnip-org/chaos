use anyhow::Context;
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let database_url = std::env::var("DATABASE_URL").context("DATABASE_URL is required")?;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .context("failed to connect to PostgreSQL for migrations")?;

    sqlx::migrate!("../../migrations")
        .run(&pool)
        .await
        .context("database migration failed")?;

    println!("database migrations completed");
    Ok(())
}
