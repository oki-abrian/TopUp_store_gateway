use crate::config::Config;
use reqwest::Client as HttpClient;
use sqlx::MySqlPool;
use std::sync::Arc;
use dashmap::DashMap;
use crate::domain::auth::models::PendingRegistration;

#[derive(Clone)]
pub struct AppState {
    pub db: MySqlPool,
    pub http_client: HttpClient,
    pub config: Arc<Config>,
    pub pending_registers: Arc<DashMap<String, PendingRegistration>>,
}

impl AppState {
    pub fn new(db: MySqlPool, config: Config) -> Self {
        Self {
            db,
            http_client: HttpClient::new(),
            config: Arc::new(config),
            pending_registers: Arc::new(DashMap::new()),
        }
    }
}
