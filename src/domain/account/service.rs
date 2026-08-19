use crate::error::AppError;
use crate::models::{Transaction, User, UserApi};
use serde::{Deserialize, Serialize};
use sqlx::{MySql, MySqlPool};

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct MutationLog {
    pub id: u64,
    pub username: String,
    pub type_name: String, // + or -
    pub amount: f64,
    pub note: String,
    pub date_cr: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateProfileRequest {
    pub username: String,
    pub name: String,
    pub email: String,
    pub phone: String,
}

#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    pub username: String,
    pub old: String,
    pub new: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateApiSettingsRequest {
    pub username: String,
    pub whitelist: String,
    pub callback: String,
}

#[derive(Debug, Deserialize)]
pub struct DepositRequest {
    pub username: String,
    pub method: String,
    pub amount: f64,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct ActivityLog {
    pub id: u64,
    pub date: String,
    pub action: String,
    pub ip: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct DepositLog {
    pub id: String,
    pub method: String,
    pub amount: f64,
    pub status: String,
}

pub async fn get_user_profile(db: &MySqlPool, username: &str) -> Result<User, AppError> {
    let user: Option<User> = sqlx::query_as::<MySql, User>("SELECT * FROM users WHERE username = ?")
        .bind(username)
        .fetch_optional(db)
        .await?;

    user.ok_or(AppError::UserNotFound)
}

pub async fn get_user_mutations(db: &MySqlPool, username: &str) -> Result<Vec<MutationLog>, AppError> {
    let logs: Vec<MutationLog> = sqlx::query_as::<MySql, MutationLog>(
        "SELECT id, username, type as type_name, amount, note, date_cr FROM mutation WHERE username = ? ORDER BY id DESC LIMIT 100",
    )
    .bind(username)
    .fetch_all(db)
    .await?;

    Ok(logs)
}

pub async fn get_user_transactions(db: &MySqlPool, username: &str) -> Result<Vec<Transaction>, AppError> {
    let trxs: Vec<Transaction> = sqlx::query_as::<MySql, Transaction>(
        "SELECT * FROM transaction WHERE user = ? ORDER BY id DESC LIMIT 100",
    )
    .bind(username)
    .fetch_all(db)
    .await?;

    Ok(trxs)
}

pub async fn generate_api_keys(db: &MySqlPool, username: &str) -> Result<UserApi, AppError> {
    let uid = format!("UID{}", rand::random::<u32>());
    let ukey = format!("KEY_{}{}", chrono::Utc::now().timestamp(), rand::random::<u32>());

    sqlx::query("DELETE FROM users_api WHERE user = ?")
        .bind(username)
        .execute(db)
        .await?;

    sqlx::query("INSERT INTO users_api (user, uid, ukey, whitelist, callback, date_cr) VALUES (?, ?, ?, '*', '', NOW())")
        .bind(username)
        .bind(&uid)
        .bind(&ukey)
        .execute(db)
        .await?;

    let api_record: UserApi = sqlx::query_as::<MySql, UserApi>("SELECT * FROM users_api WHERE user = ?")
        .bind(username)
        .fetch_one(db)
        .await?;

    Ok(api_record)
}

pub async fn update_api_settings(
    db: &MySqlPool,
    username: &str,
    req: UpdateApiSettingsRequest,
) -> Result<bool, AppError> {
    sqlx::query("UPDATE users_api SET whitelist = ?, callback = ? WHERE user = ?")
        .bind(&req.whitelist)
        .bind(&req.callback)
        .bind(username)
        .execute(db)
        .await?;

    Ok(true)
}

use chrono::Local;
use rand::Rng;

pub async fn create_deposit(
    db: &sqlx::MySqlPool,
    req: DepositRequest,
) -> Result<String, crate::error::AppError> {
    let method_code = String::from_utf8(base64::decode(&req.method).unwrap_or_default()).unwrap_or_default();
    
    // Fetch method
    let row = sqlx::query("SELECT * FROM metode WHERE code = ? AND status = 'on'")
        .bind(&method_code)
        .fetch_optional(db)
        .await?;

    if let Some(r) = row {
        use sqlx::Row;
        let method_min: f64 = r.try_get("min").unwrap_or(0.0);
        let method_max: f64 = r.try_get("max").unwrap_or(0.0);
        let method_type: String = r.try_get("type").unwrap_or_default();
        let method_name: String = r.try_get("name").unwrap_or_default();
        let method_provider: String = r.try_get("provider").unwrap_or_default();
        let method_guide: String = r.try_get("guide").unwrap_or_default();
        let expired_minutes: i64 = r.try_get("expired").unwrap_or(60);
        
        let fee_type: String = r.try_get("fee_type").unwrap_or_default();
        let fee_using: String = r.try_get("fee_using").unwrap_or_default();
        let percent: f64 = r.try_get("percent").unwrap_or(0.0);
        let flat: f64 = r.try_get("flat").unwrap_or(0.0);

        if req.amount < method_min {
            return Err(crate::error::AppError::BadRequest(format!("Minimal deposit Rp {}", method_min)));
        }
        if req.amount > method_max {
            return Err(crate::error::AppError::BadRequest(format!("Maksimal deposit Rp {}", method_max)));
        }

        let mut fee = if fee_type == "+" {
            flat
        } else {
            (req.amount * percent).ceil() + flat
        };
        
        if fee_using == "merchant" {
            fee = 0.0;
        }

        let uniq = if method_provider == "X" {
            rand::thread_rng().gen_range(100..450) as f64
        } else {
            0.0
        };

        // Generate deposit_id like DP2024...
        let prefix = "DP";
        let date_str = Local::now().format("%Y%m%d%H%M").to_string();
        let rand_num: i32 = rand::thread_rng().gen_range(10..99);
        let post_rid = format!("{}{}{}", prefix, date_str, rand_num);

        let dtme = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let expired_at = (Local::now() + chrono::Duration::minutes(expired_minutes)).format("%Y-%m-%d %H:%M:%S").to_string();

        // Fetch user data
        let user: Option<crate::models::User> = sqlx::query_as("SELECT * FROM users WHERE username = ?")
            .bind(&req.username)
            .fetch_optional(db)
            .await?;
        let (user_email, user_phone) = if let Some(ref u) = user {
            (u.email.clone().unwrap_or_else(|| "anonim@gmail.com".to_string()), u.phone.clone().unwrap_or_else(|| "08000000000".to_string()))
        } else {
            ("anonim@gmail.com".to_string(), "08000000000".to_string())
        };

        // Fetch provider credentials
        let provider_row = sqlx::query("SELECT * FROM provider WHERE code = ?")
            .bind(&method_provider)
            .fetch_optional(db)
            .await?;
            
        let mut req_action = "-".to_string();
        let mut req_pid = post_rid.clone();
        let mut req_status = "Menunggu Pembayaran".to_string();
        
        if let Some(p) = provider_row {
            use sqlx::Row;
            let p_userid: String = p.try_get("userid").unwrap_or_default();
            let p_apikey: String = p.try_get("apikey").unwrap_or_default();
            
            let client = reqwest::Client::new();
            let total_amount = req.amount + uniq;
            let return_url = format!("https://yoursite.com/deposit/invoices/{}", post_rid);

            if method_provider == "PAYDISINI" {
                let gateway = crate::payments::paydisini::PaydisiniGateway::new(p_apikey, p_userid);
                match gateway.create_transaction(&client, &method_code, &post_rid, total_amount as u64, "Deposit", &req.username, &user_email, &user_phone, &return_url).await {
                    Ok(data) => {
                        req_pid = post_rid.clone();
                        req_action = if method_type == "ewallet" {
                            data.checkout_url.or(data.qr_content).unwrap_or_default()
                        } else if method_type == "virtual" {
                            data.pay_code.unwrap_or_default()
                        } else {
                            data.pay_code.unwrap_or_default()
                        };
                    },
                    Err(e) => {
                        return Err(crate::error::AppError::InternalError(format!("Gateway Error: {:?}", e)));
                    }
                }
            } else if method_provider == "TRIPAY" {
                // Tripay requires merchant_code, api_key, private_key which are usually stored in 'merchant', 'apikey', 'secret'
                let p_merchant: String = p.try_get("merchant").unwrap_or_default(); // merchant_code
                let p_secret: String = p.try_get("secret").unwrap_or_default(); // private_key
                let is_prod = true; // Assuming production for now
                let gateway = crate::payments::tripay::TripayGateway::new(p_merchant, p_apikey, p_secret, is_prod);
                match gateway.create_transaction(&client, &method_code, &post_rid, total_amount, "Deposit", &req.username, &user.clone().map_or("Anon".to_string(), |u| u.name.unwrap_or_default()), &user_email, &user_phone, &return_url).await {
                    Ok(data) => {
                        req_pid = data.reference;
                        req_action = if method_type == "ewallet" {
                            data.checkout_url.or(data.qr_string).unwrap_or_default()
                        } else if method_type == "virtual" {
                            data.pay_code.unwrap_or_default()
                        } else {
                            data.checkout_url.unwrap_or_default()
                        };
                    },
                    Err(e) => return Err(crate::error::AppError::InternalError(format!("Tripay Gateway Error: {:?}", e))),
                }
            } else if method_provider == "TOKOPAY" {
                req_action = format!("https://tokopay.co.id/checkout/{}", post_rid);
            } else if method_provider == "DUITKU" {
                req_action = format!("https://duitku.com/checkout/{}", post_rid);
            }
        } else if method_provider == "X" {
            req_action = method_guide.clone();
        }

        sqlx::query(
            "INSERT INTO deposit (deposit_id, deposit_pid, username, method, type, name, message, amount, fee, balance, status, action, provider, note, date, expired) 
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&post_rid)
        .bind(&req_pid)
        .bind(&req.username)
        .bind(&method_code)
        .bind(&method_type)
        .bind(&method_name)
        .bind(&req_status)
        .bind(req.amount)
        .bind(fee)
        .bind(uniq)
        .bind("unpaid")
        .bind(&req_action)
        .bind(&method_provider)
        .bind(&method_guide)
        .bind(&dtme)
        .bind(&expired_at)
        .bind(&expired_at)
        .execute(db)
        .await?;

        // Inform Topup Server via Telegram
        let msg = format!("[NEW_DEPOSIT] {} {} {} {} {}", post_rid, req.username, user_phone, method_provider, req.amount);
        let _ = crate::domain::telegram::sender::send_report_to_fulfillment(&msg).await;

        Ok(post_rid)
    } else {
        Err(crate::error::AppError::BadRequest("Metode tidak ditemukan".to_string()))
    }
}
