use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;




#[derive(Debug)]
pub enum AppError {
    DatabaseError(String),
    UserNotFound,
    AccountLocked(String),
    InvalidApiSignature,
    IpNotPermitted(String),
    ServiceNotFound,
    ServerNotFound,
    TargetEmpty,
    InsufficientBalance,
    HoldBalanceViolation { required_hold: f64 },
    RateLimitTriggered { wait_seconds: u64 },
    ProviderError(String),
    InvalidCredentials,
    InternalError(String),
    BadRequest(String),
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::UserNotFound => write!(f, "User not found"),
            AppError::AccountLocked(reason) => write!(f, "Account locked: {}", reason),
            AppError::InvalidApiSignature => write!(f, "Invalid API signature"),
            AppError::IpNotPermitted(ip) => write!(f, "IP {} not permitted", ip),
            AppError::ServiceNotFound => write!(f, "Service not found"),
            AppError::ServerNotFound => write!(f, "Server not found"),
            AppError::TargetEmpty => write!(f, "Target cannot be empty"),
            AppError::InsufficientBalance => write!(f, "Saldo Akun tidak mencukupi"),
            AppError::HoldBalanceViolation { required_hold } => write!(f, "Saldo Akun minimum {}", required_hold),
            AppError::RateLimitTriggered { wait_seconds } => write!(f, "Rate limit triggered, wait {}s", wait_seconds),
            AppError::ProviderError(msg) => write!(f, "Provider error: {}", msg),
            AppError::InvalidCredentials => write!(f, "Invalid credentials"),
            AppError::DatabaseError(err) => write!(f, "Database error: {}", err),
            AppError::InternalError(err) => write!(f, "Internal error: {}", err),
            AppError::BadRequest(err) => write!(f, "Bad request: {}", err),
        }
    }
}

impl std::error::Error for AppError {}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::UserNotFound => (StatusCode::BAD_REQUEST, "User not found.".to_string()),
            AppError::AccountLocked(reason) => (StatusCode::FORBIDDEN, reason),
            AppError::InvalidApiSignature => (StatusCode::UNAUTHORIZED, "API Signature not valid.".to_string()),
            AppError::IpNotPermitted(ip) => (StatusCode::FORBIDDEN, format!("IP {} is not permitted", ip)),
            AppError::ServiceNotFound => (StatusCode::NOT_FOUND, "Service not found.".to_string()),
            AppError::ServerNotFound => (StatusCode::NOT_FOUND, "Server not found.".to_string()),
            AppError::TargetEmpty => (StatusCode::BAD_REQUEST, "Target cannot be empty.".to_string()),
            AppError::InsufficientBalance => (StatusCode::BAD_REQUEST, "Saldo Akun tidak mencukupi.".to_string()),
            AppError::HoldBalanceViolation { required_hold } => (
                StatusCode::BAD_REQUEST,
                format!("Saldo Akun minimum {}.", required_hold),
            ),
            AppError::RateLimitTriggered { wait_seconds } => (
                StatusCode::TOO_MANY_REQUESTS,
                format!("Silakan ulangi transaksi dalam waktu {} detik.", wait_seconds),
            ),
            AppError::ProviderError(msg) => (StatusCode::BAD_GATEWAY, msg),
            AppError::InvalidCredentials => (StatusCode::UNAUTHORIZED, "Kredensial tidak valid.".to_string()),
            AppError::DatabaseError(err) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Database Error: {}", err)),
            AppError::InternalError(err) => (StatusCode::INTERNAL_SERVER_ERROR, err),
            AppError::BadRequest(err) => (StatusCode::BAD_REQUEST, err),
        };

        let body = Json(json!({
            "result": false,
            "data": serde_json::Value::Null,
            "message": message
        }));

        (status, body).into_response()
    }
}

impl From<sqlx::Error> for AppError {
    fn from(err: sqlx::Error) -> Self {
        AppError::DatabaseError(err.to_string())
    }
}

impl From<reqwest::Error> for AppError {
    fn from(err: reqwest::Error) -> Self {
        AppError::ProviderError(err.to_string())
    }
}
