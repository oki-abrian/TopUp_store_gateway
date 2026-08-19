use rust_backend::{
    config::Config,
    domain::cron::{auto_refund::start_expired_cleaner_task, status_poller::start_status_poller_task, tokopay_worker::start_tokopay_worker},
    domain::telegram::gopay_bot::start_gopay_bot,
    domain::telegram::report_bot::start_report_bot,
};
use sqlx::mysql::MySqlPoolOptions;
use std::env;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "fulfillment_service=debug,teloxide=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Config::from_env();
    tracing::info!("Starting [SERVICE 2: FULFILLMENT_SERVICE] ZERO-INBOUND TELEGRAM BOT ARCHITECTURE...");

    let db_pool = MySqlPoolOptions::new()
        .max_connections(20)
        .connect_lazy(&config.database_url)?;

    let http_client = reqwest::Client::new();

    // START BACKGROUND CRON WORKERS (Status Poller, Expired Cleaner, Tokopay Poller)
    start_status_poller_task(db_pool.clone(), http_client.clone()).await;
    start_expired_cleaner_task(db_pool.clone()).await;
    
    start_tokopay_worker(db_pool.clone()).await;
    
    tracing::info!("[SERVICE 2] Tokio Background Cron Workers started.");

    // START TELEGRAM BOTS
    let bot_1_token = env::var("TELEGRAM_BOT_1_TOKEN").unwrap_or_default();
    let group_1_id = env::var("TELEGRAM_GROUP_1_ID").unwrap_or_default();
    let bot_2_token = env::var("TELEGRAM_BOT_2_TOKEN")
        .or_else(|_| env::var("TELEGRAM_BOT_TOKEN"))
        .unwrap_or_default();
    let group_2_id = env::var("TELEGRAM_ADMIN_CHAT_ID")
        .or_else(|_| env::var("TELEGRAM_CHAT_ID"))
        .or_else(|_| env::var("TELEGRAM_GROUP_2_ID"))
        .unwrap_or_default();

    if !bot_1_token.is_empty() && !group_1_id.is_empty() {
        let db_pool_clone = db_pool.clone();
        tokio::spawn(async move {
            start_gopay_bot(db_pool_clone, bot_1_token, group_1_id).await;
        });
    } else {
        tracing::warn!("TELEGRAM_BOT_1_TOKEN or TELEGRAM_GROUP_1_ID is missing. GoPay Bot will not start.");
    }

    if !bot_2_token.is_empty() && !group_2_id.is_empty() {
        let db_pool_clone = db_pool.clone();
        tokio::spawn(async move {
            start_report_bot(db_pool_clone, bot_2_token, group_2_id).await;
        });
    } else {
        tracing::warn!("TELEGRAM_BOT_2_TOKEN or TELEGRAM_GROUP_2_ID is missing. Report Bot will not start.");
    }

    // Keep the main thread alive, acting purely as a background worker
    tracing::info!("[SERVICE 2: FULFILLMENT_SERVICE] Server is fully active. Zero HTTP ports opened.");
    
    // Wait for shutdown signal
    tokio::signal::ctrl_c().await?;
    tracing::info!("[SERVICE 2] Shutting down...");

    Ok(())
}


