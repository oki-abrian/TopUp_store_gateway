use base64::prelude::*;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::env;
use std::time::Duration;
use tokio::sync::mpsc;

type HmacSha256 = Hmac<Sha256>;

use std::sync::Mutex;

lazy_static::lazy_static! {
    static ref SENDER_TX: Mutex<Option<mpsc::UnboundedSender<String>>> = Mutex::new(None);
}

fn get_queue_sender() -> mpsc::UnboundedSender<String> {
    let mut lock = SENDER_TX.lock().unwrap();
    if let Some(ref tx) = *lock {
        tx.clone()
    } else {
        let (tx, rx) = mpsc::unbounded_channel::<String>();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(start_telegram_batch_worker(rx));
        }
        *lock = Some(tx.clone());
        tx
    }
}

pub fn get_encryption_key() -> String {
    let raw = env::var("TELEGRAM_ENCRYPTION_KEY").unwrap_or_default();
    if !raw.is_empty() {
        raw
    } else {
        let api_hash = env::var("TELEGRAM_API_HASH").unwrap_or_default();
        format!("ARUTERU_SHOPPU_KEY_2026_{}", api_hash)
    }
}

pub fn encrypt_telegram_payload(data: &str) -> String {
    let key = get_encryption_key();
    let ts = chrono::Utc::now().timestamp();
    let nonce: u64 = rand::random();
    let raw_payload = format!("{}|{}|{}", ts, nonce, data);
    let base64_payload = BASE64_STANDARD.encode(raw_payload.as_bytes());

    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).expect("HMAC can take key of any size");
    mac.update(base64_payload.as_bytes());
    let signature = hex::encode(mac.finalize().into_bytes());

    format!("ENC:{}.{}", signature, base64_payload)
}

pub async fn init_telegram_sender() -> Result<(), String> {
    // Memastikan antrean worker aktif saat server startup
    let _ = get_queue_sender();
    tracing::info!("[TELEGRAM SENDER] Sender queue initialized.");
    Ok(())
}

async fn start_telegram_batch_worker(mut rx: mpsc::UnboundedReceiver<String>) {
    tracing::info!("[TELEGRAM BATCH SENDER] Starting 5-second queue worker...");
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    loop {
        interval.tick().await;

        let mut batch = Vec::new();
        while let Ok(item) = rx.try_recv() {
            batch.push(item);
            if batch.len() >= 50 {
                break; // Maksimal 50 item per batch pengiriman
            }
        }

        if batch.is_empty() {
            continue;
        }

        let mut chunks: Vec<Vec<String>> = Vec::new();
        let mut current_chunk = Vec::new();
        let mut current_len = 0;

        for item in batch {
            let item_len = item.len() + 1;
            if current_len + item_len > 3000 && !current_chunk.is_empty() {
                chunks.push(current_chunk);
                current_chunk = Vec::new();
                current_len = 0;
            }
            current_len += item_len;
            current_chunk.push(item);
        }
        if !current_chunk.is_empty() {
            chunks.push(current_chunk);
        }

        // Kirim menggunakan TELEGRAM_BOT_SENDER_TOKEN atau TELEGRAM_BOT_TOKEN atau TELEGRAM_BOT_2_TOKEN
        let bot_token = env::var("TELEGRAM_BOT_SENDER_TOKEN")
            .or_else(|_| env::var("TELEGRAM_BOT_TOKEN"))
            .or_else(|_| env::var("TELEGRAM_BOT_2_TOKEN"))
            .unwrap_or_default();
        let group_id = env::var("TELEGRAM_GROUP_2_ID").unwrap_or_default();

        if bot_token.is_empty() || group_id.is_empty() {
            tracing::warn!("[TELEGRAM BATCH SENDER] TELEGRAM_BOT_TOKEN or TELEGRAM_GROUP_2_ID is missing. Dropping batch.");
            continue;
        }

        let url = format!("https://api.telegram.org/bot{}/sendMessage", bot_token);

        for chunk in chunks {
            let combined_text = chunk.join("\n");
            let encrypted_message = encrypt_telegram_payload(&combined_text);

            let payload = serde_json::json!({
                "chat_id": group_id,
                "text": encrypted_message
            });

            let mut sent_successfully = false;
            for attempt in 1..=3 {
                match http_client.post(&url).json(&payload).send().await {
                    Ok(resp) if resp.status().is_success() => {
                        tracing::info!("[TELEGRAM BATCH SENDER] Successfully sent encrypted batch chunk of {} items to Telegram.", chunk.len());
                        sent_successfully = true;
                        break;
                    }
                    Ok(resp) => {
                        let status = resp.status();
                        let err_body = resp.text().await.unwrap_or_default();
                        tracing::warn!("[TELEGRAM BATCH SENDER] Attempt {}/3 failed ({}): {}", attempt, status, err_body);
                        tokio::time::sleep(Duration::from_millis(1000 * attempt)).await;
                    }
                    Err(e) => {
                        tracing::warn!("[TELEGRAM BATCH SENDER] Attempt {}/3 network error: {}", attempt, e);
                        tokio::time::sleep(Duration::from_millis(1000 * attempt)).await;
                    }
                }
            }

            if !sent_successfully {
                tracing::error!("[TELEGRAM BATCH SENDER] CRITICAL: Failed to deliver batch chunk of {} items after 3 attempts.", chunk.len());
            }
        }
    }
}

pub async fn send_report_to_fulfillment(order_or_cmd: &str) -> Result<(), String> {
    let formatted = if order_or_cmd.starts_with('[') {
        order_or_cmd.to_string()
    } else {
        format!("[REPORT] {}", order_or_cmd)
    };

    // Masukkan ke antrean memori non-blocking (instan <1ms)
    get_queue_sender()
        .send(formatted)
        .map_err(|e| format!("Failed to enqueue report: {}", e))?;

    Ok(())
}


