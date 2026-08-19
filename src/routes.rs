use axum::{
    extract::{ConnectInfo, Query, State},
    routing::{get, post},
    Form, Json, Router,
};
use crate::{
    checkers::{duniagames::DuniaGamesChecker, smileone_mlbb::SmileOneChecker},
    domain::orders::engine::process_api_order,
    error::AppError,
    models::{ApiOrderRequest, Service},
    state::AppState,
};
use serde::Deserialize;
use serde_json::json;
use sqlx::MySql;
use std::net::SocketAddr;

#[derive(Deserialize)]
pub struct MLCheckQuery {
    pub user_id: String,
    pub zone_id: String,
}

#[derive(Deserialize)]
pub struct GameCheckQuery {
    pub user_id: String,
    pub zone_id: Option<String>,
    pub game_code: String,
}

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/api/order", post(handle_api_order))
        .route("/api/service", get(handle_list_services))
        .route("/api/check-ml", get(handle_check_ml))
        .route("/api/check-game", get(handle_check_game))
        .with_state(state)
}

async fn health_check() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "service": "Aruteru Shoppu Rust Backend" }))
}

async fn handle_api_order(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Form(payload): Form<ApiOrderRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let client_ip = addr.ip().to_string();
    let res = process_api_order(&state, &client_ip, payload).await?;
    Ok(Json(serde_json::to_value(res).unwrap()))
}

async fn handle_list_services(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    let services = sqlx::query_as::<MySql, Service>(
        "SELECT * FROM service WHERE status = 'available' ORDER BY id ASC",
    )
    .fetch_all(&state.db)
    .await?;

    Ok(Json(json!({
        "result": true,
        "data": services,
        "message": "Data layanan berhasil didapatkan"
    })))
}

async fn handle_check_ml(
    State(state): State<AppState>,
    Query(query): Query<MLCheckQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let res = SmileOneChecker::check_role(&state.http_client, &query.user_id, &query.zone_id).await?;
    Ok(Json(json!(res)))
}

async fn handle_check_game(
    State(state): State<AppState>,
    Query(query): Query<GameCheckQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let zone = query.zone_id.unwrap_or_default();
    let res = DuniaGamesChecker::check(&state.http_client, &query.user_id, &zone, &query.game_code).await?;
    Ok(Json(json!(res)))
}
