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

/// Backpressure: batas antrean memori agar flood tidak menghabiskan RAM.
static QUEUE_LEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
const MAX_QUEUE_ITEMS: usize = 50_000;

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
    // Kuota Telegram ±20 call/menit per bot. Dengan 2 bot pengirim bergantian
    // (round-robin) kapasitas menjadi ±40 call/menit → siklus 5 detik aman:
    // 12 siklus/menit × 3 call (2 kirim + 1 hapus) = 36 ≤ 40.
    tracing::info!("[TELEGRAM BATCH SENDER] Starting 5-second queue worker (dual-bot round-robin, ~40 calls/min budget)...");
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    // Kumpulan bot pengirim: BOT_SENDER + BOT_4 (round-robin), fallback ke BOT/BOT_2.
    let mut sender_tokens: Vec<String> = Vec::new();
    for key in [
        "TELEGRAM_BOT_SENDER_TOKEN",
        "TELEGRAM_BOT_4_TOKEN",
        "TELEGRAM_BOT_TOKEN",
        "TELEGRAM_BOT_2_TOKEN",
    ] {
        if let Ok(t) = env::var(key) {
            if !t.is_empty() && !sender_tokens.contains(&t) {
                sender_tokens.push(t);
            }
        }
    }
    if sender_tokens.is_empty() {
        tracing::warn!("[TELEGRAM BATCH SENDER] Tidak ada token bot pengirim. Worker idle.");
        return;
    }
    let rr: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let next_token = |rr: &std::sync::atomic::AtomicUsize| -> String {
        let i = rr.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % sender_tokens.len();
        sender_tokens[i].clone()
    };

    loop {
        interval.tick().await;

        let mut batch = Vec::new();
        while let Ok(item) = rx.try_recv() {
            batch.push(item);
            if batch.len() >= 50 {
                break; // Maksimal 50 item per batch pengiriman
            }
        }
        QUEUE_LEN.fetch_sub(batch.len(), std::sync::atomic::Ordering::Relaxed);

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

        // Kirim bergantian antar bot pengirim (round-robin per chunk)
        let group_id = env::var("TELEGRAM_GROUP_2_ID").unwrap_or_default();

        if sender_tokens.is_empty() || group_id.is_empty() {
            tracing::warn!("[TELEGRAM BATCH SENDER] Token bot pengirim or TELEGRAM_GROUP_2_ID is missing. Dropping batch.");
            continue;
        }

        let mut chunk_index = 0;
        let total_chunks = chunks.len();

        for chunk in chunks {
            if chunk_index > 0 {
                // Jeda antar-chunk dalam satu siklus (5 detik = siklus aman)
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            chunk_index += 1;

            let combined_text = chunk.join("\n");
            let encrypted_message = encrypt_telegram_payload(&combined_text);

            let payload = serde_json::json!({
                "chat_id": group_id,
                "text": encrypted_message
            });

            let mut sent_successfully = false;
            for attempt in 1..=3 {
                // Round-robin: tiap percobaan pakai bot berikutnya — kalau bot A kena
                // limit, bot B langsung meneruskan tanpa menunggu retry_after penuh.
                let bot_token = next_token(&rr);
                let url = format!("https://api.telegram.org/bot{}/sendMessage", bot_token);
                match http_client.post(&url).json(&payload).send().await {
                    Ok(resp) if resp.status().is_success() => {
                        tracing::info!("[TELEGRAM BATCH SENDER] Batch terkirim via bot #{} (chunk {}/{}).", (attempt - 1) % sender_tokens.len() + 1, chunk_index, total_chunks);
                        sent_successfully = true;
                        break;
                    }
                    Ok(resp) => {
                        let status = resp.status();
                        let err_body = resp.text().await.unwrap_or_default();

                        // Parse parameter retry_after Telegram resmi jika terkena HTTP 429
                        let retry_after_secs = if status.as_u16() == 429 {
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&err_body) {
                                val.get("parameters")
                                    .and_then(|p| p.get("retry_after"))
                                    .and_then(|r| r.as_u64())
                                    .unwrap_or(5)
                            } else {
                                5
                            }
                        } else {
                            // Cycle-aligned backoff: kelipatan siklus aman (5s, 10s, 20s)
                            5 * (1 << (attempt - 1))
                        };

                        tracing::warn!(
                            "[TELEGRAM BATCH SENDER] Attempt {}/3 failed via bot (Status: {}). Body: {}. Menunggu {} detik sebelum percobaan berikutnya...",
                            attempt, status, err_body, retry_after_secs
                        );
                        tokio::time::sleep(Duration::from_secs(retry_after_secs)).await;
                    }
                    Err(e) => {
                        let wait_secs = 5 * (1 << (attempt - 1)); // 5s, 10s, 20s
                        tracing::warn!(
                            "[TELEGRAM BATCH SENDER] Attempt {}/3 network/RTO error: {}. Menunggu {} detik sebelum percobaan berikutnya...",
                            attempt, e, wait_secs
                        );
                        tokio::time::sleep(Duration::from_secs(wait_secs)).await;
                    }
                }
            }

            if !sent_successfully {
                tracing::error!(
                    "[TELEGRAM BATCH SENDER] CRITICAL: Gagal mengirim batch chunk {} item setelah 3 percobaan. Memasukkan kembali ke antrean memori (Zero Data Loss Re-queue).",
                    chunk.len()
                );
                let queue = get_queue_sender();
                QUEUE_LEN.fetch_add(chunk.len(), std::sync::atomic::Ordering::Relaxed);
                for item in chunk {
                    let _ = queue.send(item);
                }
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

    // Masukkan ke antrean memori non-blocking (instan <1ms), dengan batas backpressure
    if QUEUE_LEN.load(std::sync::atomic::Ordering::Relaxed) >= MAX_QUEUE_ITEMS {
        tracing::error!("[TELEGRAM BATCH SENDER] Antrean penuh ({} item). Pesan DITOLAK (backpressure): {}", MAX_QUEUE_ITEMS, order_or_cmd);
        return Err("Telegram queue full — coba lagi nanti".to_string());
    }
    QUEUE_LEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    match get_queue_sender().send(formatted) {
        Ok(_) => Ok(()),
        Err(e) => {
            QUEUE_LEN.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            Err(format!("Failed to enqueue report: {}", e))
        }
    }
}


