use base64::prelude::*;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::sync::Arc;
use sqlx::{MySqlPool, Row};
use teloxide::prelude::*;

type HmacSha256 = Hmac<Sha256>;

fn is_safe_string(s: &str) -> bool {
    // Only allow alphanumeric, dash, underscore, and dot. Blocks < > ; ' " and other script/injection chars.
    s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

pub fn get_encryption_key() -> String {
    let raw = std::env::var("TELEGRAM_ENCRYPTION_KEY").unwrap_or_default();
    if !raw.is_empty() {
        raw
    } else {
        let api_hash = std::env::var("TELEGRAM_API_HASH").unwrap_or_default();
        format!("ARUTERU_SHOPPU_KEY_2026_{}", api_hash)
    }
}

pub fn decrypt_telegram_payload(enc_str: &str) -> Result<String, String> {
    let payload = enc_str.strip_prefix("ENC:").ok_or_else(|| "Not an encrypted payload".to_string())?;
    let parts: Vec<&str> = payload.split('.').collect();
    if parts.len() != 2 {
        return Err("Invalid encrypted envelope format".to_string());
    }

    let signature_hex = parts[0];
    let base64_payload = parts[1];

    let key = get_encryption_key();
    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).map_err(|e| e.to_string())?;
    mac.update(base64_payload.as_bytes());
    let expected_sig = hex::encode(mac.finalize().into_bytes());

    if !signature_hex.eq_ignore_ascii_case(&expected_sig) {
        return Err("Cryptographic signature mismatch! Tampered or spoofed message.".to_string());
    }

    let decoded_bytes = BASE64_STANDARD.decode(base64_payload).map_err(|e| e.to_string())?;
    let raw_payload = String::from_utf8(decoded_bytes).map_err(|e| e.to_string())?;

    let elements: Vec<&str> = raw_payload.splitn(3, '|').collect();
    if elements.len() < 3 {
        return Err("Invalid payload structure".to_string());
    }

    let ts: i64 = elements[0].parse().map_err(|_| "Invalid timestamp in payload".to_string())?;
    let now = chrono::Utc::now().timestamp();

    // Anti-Replay: Tolak pesan yang lebih lama dari 120 detik
    if (now - ts).abs() > 120 {
        return Err(format!("Payload timestamp expired (Age: {}s). Possible Replay Attack blocked.", now - ts));
    }

    Ok(elements[2].to_string())
}

async fn handle_report_message(
    bot: Bot,
    msg: Message,
    state: Arc<(MySqlPool, String)>,
) -> ResponseResult<()> {
    let db_pool = &state.0;
    let expected_group_id = &state.1;

    // Check if message is in the correct group
    let chat_id = msg.chat.id.to_string();
    if chat_id != *expected_group_id {
        return Ok(());
    }

    // Reject non-text messages
    if msg.text().is_none() {
        tracing::info!("[REPORT BOT] Rejected non-text message.");
        let _ = bot.delete_message(msg.chat.id, msg.id).await;
        return Ok(());
    }

    if let Some(text) = msg.text() {
        tracing::info!("[REPORT BOT] Received Telegram message envelope (Length: {} bytes)", text.len());

        let decrypted_commands = if text.starts_with("ENC:") {
            match decrypt_telegram_payload(text) {
                Ok(content) => {
                    tracing::info!("[REPORT BOT] Successfully authenticated and decrypted payload.");
                    content
                }
                Err(err) => {
                    tracing::warn!("[REPORT BOT] SECURITY ALERT: Dropping unauthenticated message: {}", err);
                    let _ = bot.delete_message(msg.chat.id, msg.id).await;
                    return Ok(());
                }
            }
        } else {
            // Periksa jika sistem memperbolehkan plain text (opsional untuk testing lokal)
            let allow_plain = std::env::var("ALLOW_PLAIN_TELEGRAM").unwrap_or_default() == "true";
            if allow_plain {
                text.to_string()
            } else {
                tracing::warn!("[REPORT BOT] Blocked plain-text message! Telegram End-to-End Encryption is enforced.");
                let _ = bot.delete_message(msg.chat.id, msg.id).await;
                return Ok(());
            }
        };

        // Split multi-line batch commands and execute each command
        for line in decrypted_commands.lines() {
            let cmd_text = line.trim();
            if cmd_text.is_empty() {
                continue;
            }
            process_single_command(&bot, msg.chat.id, db_pool, cmd_text).await;
        }

        // Auto-delete the envelope to keep group clean
        let _ = bot.delete_message(msg.chat.id, msg.id).await;
    }

    Ok(())
}

async fn process_single_command(
    bot: &Bot,
    chat_id: teloxide::types::ChatId,
    db_pool: &MySqlPool,
    text: &str,
) {
    if text.starts_with("[NEW_DEPOSIT]") {
        let parts: Vec<&str> = text.split_whitespace().collect();
        if parts.len() >= 6 {
            let deposit_id = parts[1];
            let username = parts[2];
            let phone = parts[3];
            let method = parts[4];
            let nominal_str = parts[5];

            if !is_safe_string(deposit_id) || !is_safe_string(username) || !is_safe_string(phone) {
                tracing::warn!("[REPORT BOT] Blocked hack attempt! Unsafe characters in NEW_DEPOSIT payload.");
                return;
            }

            let nominal: f64 = nominal_str.parse().unwrap_or(0.0);
            tracing::info!("[REPORT BOT] NEW_DEPOSIT received: {} for {}", deposit_id, username);

            let _ = sqlx::query("INSERT INTO users (username, phone, balance, level) VALUES (?, ?, 0.0, 'Member') ON DUPLICATE KEY UPDATE phone = ?")
                .bind(username)
                .bind(phone)
                .bind(phone)
                .execute(db_pool)
                .await;

            let _ = sqlx::query(
                "INSERT INTO deposit (deposit_id, username, method, amount, status, date) VALUES (?, ?, ?, ?, 'unpaid', NOW())"
            )
            .bind(deposit_id)
            .bind(username)
            .bind(method)
            .bind(nominal)
            .execute(db_pool)
            .await;

            let _ = bot.send_message(chat_id, format!("📥 Deposit {} (Rp{}) dari {} dicatat. Menunggu sinkronisasi Tokopay...", deposit_id, nominal, username)).await;
        }
    } else if text.starts_with("[REQ_OTP]") {
        let parts: Vec<&str> = text.split_whitespace().collect();
        if parts.len() >= 6 {
            let order_id = parts[1];
            let username = parts[2];
            let service_code = parts[3];
            let target_game = parts[4];

            if !is_safe_string(order_id) || !is_safe_string(username) || !is_safe_string(service_code) {
                tracing::warn!("[REPORT BOT] Blocked hack attempt! Unsafe characters in REQ_OTP payload.");
                return;
            }

            let (real_price, provider, service_name): (f64, String, String) = if let Ok(Some(svc_row)) = sqlx::query("SELECT price, provider, name FROM service WHERE code = ? LIMIT 1").bind(&service_code).fetch_optional(db_pool).await {
                let p = svc_row.try_get("price").unwrap_or(0.0);
                let prov = svc_row.try_get("provider").unwrap_or_else(|_| "DIGI".to_string());
                let n = svc_row.try_get("name").unwrap_or_default();
                (p, prov, n)
            } else {
                tracing::error!("[REPORT BOT] Service code {} not found in Topup DB! Rejecting order.", service_code);
                let _ = bot.send_message(chat_id, format!("❌ Gagal memproses {}: Produk tidak ditemukan di Server Topup.", order_id)).await;
                return;
            };

            tracing::info!("[REPORT BOT] REQ_OTP received: {} for {} (Provider: {}). Real Price: {}", order_id, username, provider, real_price);

            let _ = sqlx::query(
                "INSERT INTO transaction (order_id, user, code, service_name, target, price, status, payment_status, provider, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, 'pending', 'unpaid', ?, NOW(), NOW()) ON DUPLICATE KEY UPDATE provider = VALUES(provider), updated_at = NOW()"
            )
            .bind(order_id)
            .bind(username)
            .bind(service_code)
            .bind(&service_name)
            .bind(target_game)
            .bind(real_price)
            .bind(&provider)
            .execute(db_pool)
            .await;

            let phone_query = "SELECT phone FROM users WHERE username = ? LIMIT 1";
            if let Ok(Some(user_row)) = sqlx::query(phone_query).bind(username).fetch_optional(db_pool).await {
                let phone: String = user_row.try_get("phone").unwrap_or_default();

                if phone.is_empty() {
                    tracing::warn!("[REPORT BOT] User {} has no phone number.", username);
                    let _ = bot.send_message(chat_id, format!("❌ Gagal mengirim OTP: Nomor HP user {} tidak ditemukan.", username)).await;
                } else {
                    let otp = format!("{:06}", rand::random::<u32>() % 1000000);
                    let now_ts = chrono::Utc::now().timestamp();
                    let otp_record = format!("OTP:{}:{}:0", otp, now_ts); // OTP:CODE:TIMESTAMP:ATTEMPTS

                    let update_note = "UPDATE transaction SET note = ? WHERE order_id = ?";
                    if sqlx::query(update_note).bind(&otp_record).bind(order_id).execute(db_pool).await.is_ok() {
                        tracing::info!("[REPORT BOT] OTP generated for {}: {}", order_id, otp);

                        let mpwa_key = std::env::var("MPWA_API_KEY").unwrap_or_default();
                        let mpwa_sender = std::env::var("MPWA_SENDER_PHONE").unwrap_or_default();

                        if !mpwa_key.is_empty() {
                            let msg_text = format!("Kode OTP Anda untuk order {} adalah: *{}*. Berlaku selama 10 menit. JANGAN berikan kode ini ke siapapun.", order_id, otp);
                            let _ = crate::domain::auth::service::send_whatsapp_message(&mpwa_key, &mpwa_sender, &phone, &msg_text).await;
                            tracing::info!("[REPORT BOT] Dispatched WhatsApp OTP to {}", phone);
                        }

                        let _ = bot.send_message(chat_id, format!("✉️ OTP dikirim ke user {} untuk order {}", username, order_id)).await;
                    }
                }
            }
        }
    } else if text.starts_with("[NEW_ORDER]") {
        let parts: Vec<&str> = text.split_whitespace().collect();
        if parts.len() >= 6 {
            let order_id = parts[1];
            let username = parts[2];
            let service_code = parts[3];
            let target_game = parts[4];
            let provider = parts[5];

            if !is_safe_string(order_id) || !is_safe_string(username) || !is_safe_string(service_code) || !is_safe_string(provider) {
                tracing::warn!("[REPORT BOT] Blocked hack attempt! Unsafe characters in NEW_ORDER payload.");
                return;
            }

            let (real_price, service_name): (f64, String) = if parts.len() >= 8 {
                let p: f64 = parts[6].parse().unwrap_or(0.0);
                let n = parts[7].replace('_', " ");
                (p, n)
            } else if let Ok(Some(svc_row)) = sqlx::query("SELECT price, name FROM service WHERE code = ? LIMIT 1").bind(&service_code).fetch_optional(db_pool).await {
                let p = svc_row.try_get("price").unwrap_or(0.0);
                let n = svc_row.try_get("name").unwrap_or_default();
                (p, n)
            } else {
                (0.0, service_code.to_string())
            };

            tracing::info!("[REPORT BOT] NEW_ORDER received: {} for {} (Price: {}, Provider: {})", order_id, username, real_price, provider);

            let _ = sqlx::query(
                "INSERT INTO transaction (order_id, user, code, service_name, target, price, status, payment_status, provider, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, 'pending', 'unpaid', ?, NOW(), NOW()) ON DUPLICATE KEY UPDATE price = VALUES(price), service_name = VALUES(service_name), updated_at = NOW()"
            )
            .bind(order_id)
            .bind(username)
            .bind(service_code)
            .bind(&service_name)
            .bind(target_game)
            .bind(real_price)
            .bind(provider)
            .execute(db_pool)
            .await;
        }
    } else if text.starts_with("[VERIFY]") {
        let parts: Vec<&str> = text.split_whitespace().collect();
        if parts.len() >= 4 {
            let order_id = parts[1];
            let username = parts[2];
            let otp_input = parts[3];
            tracing::info!("[REPORT BOT] VERIFY for Order ID: {}, User: {}", order_id, username);

            let order_query = "SELECT order_id, price, note FROM transaction WHERE order_id = ? AND user = ? AND (payment_status IN ('unpaid', 'pending') OR status = 'pending') LIMIT 1";
            if let Ok(Some(trx_row)) = sqlx::query(order_query).bind(order_id).bind(username).fetch_optional(db_pool).await {
                let db_order_id: String = trx_row.try_get("order_id").unwrap_or_default();
                let price: f64 = trx_row.try_get("price").unwrap_or_default();
                let db_note: String = trx_row.try_get("note").unwrap_or_default();

                // Format: OTP:CODE:TIMESTAMP:ATTEMPTS or plain CODE
                let mut valid_otp = false;
                let mut expired = false;
                let mut max_attempts_reached = false;

                if db_note.starts_with("OTP:") {
                    let otp_parts: Vec<&str> = db_note.split(':').collect();
                    if otp_parts.len() >= 4 {
                        let actual_code = otp_parts[1];
                        let created_ts: i64 = otp_parts[2].parse().unwrap_or(0);
                        let attempts: u32 = otp_parts[3].parse().unwrap_or(0);
                        let now_ts = chrono::Utc::now().timestamp();

                        if (now_ts - created_ts) > 600 {
                            expired = true;
                        } else if attempts >= 3 {
                            max_attempts_reached = true;
                        } else if actual_code == otp_input {
                            valid_otp = true;
                        } else {
                            // Increment attempt counter
                            let new_record = format!("OTP:{}:{}:{}", actual_code, created_ts, attempts + 1);
                            let _ = sqlx::query("UPDATE transaction SET note = ? WHERE order_id = ?")
                                .bind(&new_record)
                                .bind(&db_order_id)
                                .execute(db_pool)
                                .await;
                        }
                    }
                } else if db_note == otp_input && !db_note.is_empty() {
                    valid_otp = true;
                }

                if expired {
                    tracing::warn!("[REPORT BOT] OTP kadaluarsa untuk order {}", order_id);
                    let _ = bot.send_message(chat_id, format!("❌ OTP untuk order {} telah kadaluarsa (maksimal 10 menit). Silakan minta OTP baru.", order_id)).await;
                    return;
                }

                if max_attempts_reached {
                    tracing::warn!("[REPORT BOT] Maksimal percobaan OTP terlampaui untuk order {}", order_id);
                    let _ = bot.send_message(chat_id, format!("❌ Percobaan OTP melebihi batas 3 kali untuk order {}. Transaksi dibatalkan.", order_id)).await;
                    return;
                }

                if valid_otp {
                    let user_query = "SELECT id, balance FROM users WHERE username = ? LIMIT 1";
                    if let Ok(Some(user_row)) = sqlx::query(user_query).bind(username).fetch_optional(db_pool).await {
                        let balance: f64 = user_row.try_get("balance").unwrap_or_default();

                        if balance >= price {
                            if let Ok(mut tx) = db_pool.begin().await {
                                let deduct_res = sqlx::query("UPDATE users SET balance = balance - ? WHERE username = ? AND balance >= ?")
                                    .bind(price)
                                    .bind(username)
                                    .bind(price)
                                    .execute(&mut *tx)
                                    .await;

                                if let Ok(d_res) = deduct_res {
                                    if d_res.rows_affected() > 0 {
                                        let _ = sqlx::query("UPDATE transaction SET payment_status = 'paid', note = 'Proses Report Bot (Saldo)' WHERE order_id = ?")
                                            .bind(&db_order_id)
                                            .execute(&mut *tx)
                                            .await;

                                        let _ = tx.commit().await;

                                        tracing::info!("[REPORT BOT] OTP Verified & Saldo Deducted for {}. Proceeding to Topup Digiflazz...", db_order_id);
                                        let _ = bot.send_message(chat_id, format!("✅ OTP Valid. Saldo {} dipotong Rp{}. Memproses Order: {}", username, price, db_order_id)).await;

                                        let client = reqwest::Client::new();
                                        match crate::domain::telegram::process_paid_transaction(db_pool, &client, &db_order_id, price).await {
                                            Ok(_) => {
                                                let _ = bot.send_message(chat_id, format!("🎉 Order {} berhasil di-Topup!", db_order_id)).await;
                                            }
                                            Err(e) => {
                                                let _ = bot.send_message(chat_id, format!("❌ Gagal memproses Order {}: {}", db_order_id, e)).await;
                                            }
                                        }
                                    } else {
                                        let _ = tx.rollback().await;
                                        tracing::warn!("[REPORT BOT] Gagal memotong saldo user {}", username);
                                    }
                                } else {
                                    let _ = tx.rollback().await;
                                }
                            }
                        } else {
                            tracing::warn!("[REPORT BOT] Saldo user {} tidak cukup untuk order {}", username, order_id);
                            let _ = bot.send_message(chat_id, format!("❌ Saldo user {} tidak mencukupi untuk order {}", username, order_id)).await;
                        }
                    }
                } else {
                    tracing::warn!("[REPORT BOT] OTP tidak valid untuk order {}", order_id);
                    let _ = bot.send_message(chat_id, format!("❌ OTP tidak valid untuk order {}", order_id)).await;
                }
            }
        }
    } else if text.starts_with("[REPORT]") || text.starts_with("ORD") {
        let parts: Vec<&str> = text.split_whitespace().collect();
        let order_id = if text.starts_with("[REPORT]") {
            if parts.len() >= 2 { parts[1] } else { "" }
        } else {
            parts[0]
        };

        if order_id.is_empty() || !is_safe_string(order_id) {
            tracing::warn!("[REPORT BOT] Invalid Order ID in REPORT: {}", order_id);
            return;
        }

        tracing::info!("[REPORT BOT] Processing report for Order ID: {}", order_id);

        let order_query = "SELECT order_id, provider, price FROM transaction WHERE order_id = ? AND (payment_status IN ('unpaid', 'pending') OR status = 'pending') LIMIT 1";
        if let Ok(Some(row)) = sqlx::query(order_query).bind(order_id).fetch_optional(db_pool).await {
            let db_order_id: String = row.try_get("order_id").unwrap_or_default();
            let price: f64 = row.try_get("price").unwrap_or_default();

            tracing::info!("[REPORT BOT] Order {} VERIFIED! Proceeding to Topup Digiflazz...", db_order_id);
            let _ = bot.send_message(chat_id, format!("✅ Laporan Terverifikasi. Memproses Order: {}", db_order_id)).await;

            let client = reqwest::Client::new();
            match crate::domain::telegram::process_paid_transaction(db_pool, &client, &db_order_id, price).await {
                Ok(_) => {
                    let _ = bot.send_message(chat_id, format!("🎉 Order {} berhasil di-Topup!", db_order_id)).await;
                }
                Err(e) => {
                    let _ = bot.send_message(chat_id, format!("❌ Gagal memproses Order {}: {}", db_order_id, e)).await;
                }
            }
        } else {
            tracing::warn!("[REPORT BOT] Order {} not found or already processed.", order_id);
        }
    }
}

pub async fn start_report_bot(db_pool: MySqlPool, token: String, group_id: String) {
    tracing::info!("[REPORT BOT] Starting bot thread with End-to-End Encryption & Anti-Replay...");
    let bot = Bot::new(token);
    let state = Arc::new((db_pool, group_id));

    let handler = dptree::endpoint(handle_report_message);

    let mut dispatcher = Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![state])
        .build();

    dispatcher.dispatch().await;
}

