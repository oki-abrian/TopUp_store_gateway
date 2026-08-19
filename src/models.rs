use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: i32,
    pub username: String,
    pub password: Option<String>,
    pub name: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub balance: f64,
    pub level: String, // Member, Reseller, Admin
    pub sso: Option<String>,
    pub date_cr: Option<String>,
    pub date_up: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct UserApi {
    pub id: i32,
    pub user: String,
    pub uid: String,
    pub ukey: String,
    pub whitelist: String,
    pub callback: Option<String>,
    pub date_cr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Service {
    pub id: i32,
    pub code: String,
    pub name: String,
    pub game: String,
    #[sqlx(rename = "type")]
    pub type_name: String,
    pub price: f64,
    pub member: f64,
    pub reseller: f64,
    pub provider: String,
    pub status: String, // available, empty
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Category {
    pub id: i32,
    pub name: String,
    pub code: String,
    #[sqlx(rename = "type")]
    pub type_name: String,
    pub prefix: String,
    pub data_form: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Transaction {
    pub id: i32,
    pub order_id: String,
    pub order_tid: Option<String>,
    pub provider_order_id: Option<String>,
    pub user: String,
    pub code: String,
    pub service_name: String,
    pub game: String,
    pub target: String,
    pub price: f64,
    pub profit: f64,
    pub status: String,         // pending, process, success, error
    pub payment_status: String, // unpaid, paid
    pub provider: String,
    pub note: Option<String>,
    pub flashsale: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub expired_at: Option<String>,
    pub callback: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Provider {
    pub id: i32,
    pub code: String,
    pub userid: String,
    pub apikey: String,
    pub merchant: Option<String>,
    pub link: String,
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Flashsale {
    pub id: i32,
    pub code: String,
    pub amount: f64,
    pub status: String, // 1 for active
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiOrderRequest {
    pub key: String,
    pub sign: String,
    pub service: String,
    pub target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiOrderDataResponse {
    pub order_id: String,
    pub data: String,
    pub code: String,
    pub service: String,
    pub status: String,
    pub note: String,
    pub price: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiOrderResponse {
    pub result: bool,
    pub data: Option<ApiOrderDataResponse>,
    pub message: String,
}
