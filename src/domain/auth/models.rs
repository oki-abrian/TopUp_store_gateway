use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct PendingRegistration {
    pub name: Option<String>,
    pub username: String,
    pub email: String,
    pub phone: String,
    pub password: String,
    pub otp: String,
    pub created_at: i64,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    pub turnstile_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub name: Option<String>,
    pub username: String,
    pub email: String,
    pub phone: String,
    pub password: String,
    pub turnstile_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct VerifyOtpRequest {
    pub session_id: String,
    pub otp: String,
}

#[derive(Debug, Deserialize)]
pub struct ResendOtpRequest {
    pub session_id: String,
}

#[derive(Debug, Serialize)]
pub struct AuthResponse {
    pub result: bool,
    pub token: Option<String>,
    pub username: Option<String>,
    pub message: String,
}
