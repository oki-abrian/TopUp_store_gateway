use crate::error::AppError;
use crate::models::User;
use bcrypt::{hash, verify, DEFAULT_COST};
use sqlx::{MySql, MySqlPool};
use serde::Deserialize;
use reqwest::Client;

pub fn normalize_phone(phone: &str) -> String {
    let clean: String = phone.chars().filter(|c| c.is_ascii_digit()).collect();
    if clean.starts_with("62") {
        clean
    } else if clean.starts_with('0') {
        format!("62{}", &clean[1..])
    } else {
        clean
    }
}

#[derive(Deserialize)]
struct TurnstileResponse {
    success: bool,
}

pub async fn verify_turnstile(secret_key: &str, token: &str, ip: &str) -> Result<bool, AppError> {
    let client = Client::new();
    let res = client
        .post("https://challenges.cloudflare.com/turnstile/v0/siteverify")
        .form(&[
            ("secret", secret_key),
            ("response", token),
            ("remoteip", ip),
        ])
        .send()
        .await
        .map_err(|e| AppError::InternalError(e.to_string()))?;

    let turnstile_res: TurnstileResponse = res
        .json()
        .await
        .map_err(|e| AppError::InternalError(e.to_string()))?;

    Ok(turnstile_res.success)
}

pub async fn send_whatsapp_message(api_key: &str, sender: &str, phone: &str, message: &str) -> Result<bool, AppError> {
    let wa_url = std::env::var("WA_GATEWAY_URL")
        .or_else(|_| std::env::var("MPWA_URL"))
        .unwrap_or_else(|_| "https://mpwa.byllann.com/send-message".to_string());
    
    let client = Client::new();
    let normalized_phone = normalize_phone(phone);

    // OpenWA REST API / JSON endpoint support vs MPWA Form endpoint
    let res = if wa_url.contains("openwa") || wa_url.contains("3000") || wa_url.contains("v1/send-message") || wa_url.contains("/api/send-message") {
        client.post(&wa_url)
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&serde_json::json!({
                "session": if sender.is_empty() { "default" } else { sender },
                "to": normalized_phone,
                "number": normalized_phone,
                "message": message
            }))
            .send()
            .await
    } else {
        client.post(&wa_url)
            .form(&[
                ("api_key", api_key),
                ("sender", sender),
                ("number", &normalized_phone),
                ("message", message),
            ])
            .send()
            .await
    };

    match res {
        Ok(resp) if resp.status().is_success() => {
            tracing::info!("[WA GATEWAY] WhatsApp message sent successfully to {}", normalized_phone);
            Ok(true)
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            tracing::warn!("[WA GATEWAY] Failed to send WhatsApp message ({}): {}", status, body);
            Ok(false)
        }
        Err(e) => {
            tracing::error!("[WA GATEWAY] Network error sending WhatsApp message: {}", e);
            Err(AppError::InternalError(e.to_string()))
        }
    }
}

pub async fn send_mpwa_otp(api_key: &str, sender: &str, phone: &str, message: &str) -> Result<bool, AppError> {
    send_whatsapp_message(api_key, sender, phone, message).await
}

pub async fn login_user(
    db: &MySqlPool,
    username: &str,
    password: &str,
    user_agent: &str,
    ip: &str,
) -> Result<(User, String), AppError> {
    let user_opt: Option<User> = sqlx::query_as::<MySql, User>("SELECT * FROM users WHERE username = ?")
        .bind(username)
        .fetch_optional(db)
        .await?;

    let user = user_opt.ok_or(AppError::UserNotFound)?;

    let hashed_pw = user.password.as_deref().unwrap_or("");
    let is_valid = verify(password, hashed_pw).unwrap_or(false);
    if !is_valid {
        return Err(AppError::InvalidCredentials);
    }

    let token = format!("SESS_{}{}", chrono::Utc::now().timestamp(), rand::random::<u32>());
    let expired_at = (chrono::Utc::now() + chrono::Duration::days(7)).format("%Y-%m-%d %H:%M:%S").to_string();

    sqlx::query("INSERT INTO users_cookie (username, cookie, token, active, ua, ip, expired) VALUES (?, ?, ?, NOW(), ?, ?, ?)")
        .bind(&user.username)
        .bind(&token)
        .bind(&token)
        .bind(user_agent)
        .bind(ip)
        .bind(&expired_at)
        .execute(db)
        .await?;

    Ok((user, token))
}

pub async fn register_user(
    db: &MySqlPool,
    name: &str,
    username: &str,
    email: &str,
    phone: &str,
    password: &str,
) -> Result<User, AppError> {
    let normalized_phone = normalize_phone(phone);

    let existing: Option<User> = sqlx::query_as::<MySql, User>("SELECT * FROM users WHERE username = ? OR email = ? OR phone = ?")
        .bind(username)
        .bind(email)
        .bind(&normalized_phone)
        .fetch_optional(db)
        .await?;

    if let Some(e) = existing {
        if e.username == username {
            return Err(AppError::InternalError("Username sudah terdaftar.".to_string()));
        } else if e.email.as_deref() == Some(email) {
            return Err(AppError::InternalError("Email sudah terdaftar.".to_string()));
        } else {
            return Err(AppError::InternalError("No. WhatsApp sudah terdaftar.".to_string()));
        }
    }

    let hashed_pw = hash(password, DEFAULT_COST).map_err(|e| AppError::InternalError(e.to_string()))?;
    let normalized_phone = normalize_phone(phone);

    let result = sqlx::query("INSERT INTO users (username, password, name, email, phone, balance, level, date_cr, date_up) VALUES (?, ?, ?, ?, ?, 0.0, 'Member', NOW(), NOW())")
        .bind(username)
        .bind(&hashed_pw)
        .bind(name)
        .bind(email)
        .bind(&normalized_phone)
        .execute(db)
        .await?;

    let user_id = result.last_insert_id();
    let user: User = sqlx::query_as::<MySql, User>("SELECT * FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_one(db)
        .await?;

    Ok(user)
}
