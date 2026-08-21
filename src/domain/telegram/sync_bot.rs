use sqlx::{MySqlPool, Row};

/// ============================================================
/// BOT 3 — PRICE/PROMO SYNC (Fulfillment -> Web)
/// ============================================================
/// Fulfillment adalah sumber kebenaran harga & promo.
/// Worker ini mendorong snapshot katalog + promo dari DB Topup ke Server Web
/// lewat antrean Telegram terenkripsi yang sama (HMAC-SHA256 + anti-replay).
/// Server Web TIDAK PERNAH diakses langsung oleh fulfillment — web menulis
/// DB-nya sendiri setelah memverifikasi signature pesan.

pub fn sync_interval_seconds() -> u64 {
    std::env::var("SYNC_INTERVAL_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300)
}

fn safe_token(s: &str) -> String {
    s.replace(' ', "_").replace(';', "").replace('|', "")
}

/// Bangun snapshot sinkronisasi dari DB Topup (satu baris = satu perintah).
/// Tiap seksi independen — tabel opsional yang absen tidak menggagalkan keseluruhan.
pub async fn build_sync_snapshot(db: &MySqlPool) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();

    // 1. Harga & status layanan
    let services = sqlx::query("SELECT code, name, price, member, reseller, provider, status FROM service")
        .fetch_all(db)
        .await
        .unwrap_or_default();
    for s in services {
        let code: String = s.try_get("code").unwrap_or_default();
        if code.is_empty() {
            continue;
        }
        let name = safe_token(&s.try_get::<String, _>("name").unwrap_or_default());
        let price: f64 = s.try_get("price").unwrap_or(0.0);
        let member: f64 = s.try_get("member").unwrap_or(0.0);
        let reseller: f64 = s.try_get("reseller").unwrap_or(0.0);
        let provider: String = s.try_get("provider").unwrap_or_else(|_| "DIGI".to_string());
        let status: String = s.try_get("status").unwrap_or_else(|_| "available".to_string());
        lines.push(format!(
            "[SYNC_PRICE] {} {} {} {} {} {} {}",
            code, name, price, member, reseller, provider, status
        ));
    }

    // 2. Flashsale aktif/nonaktif
    let flashes = sqlx::query("SELECT code, amount, status FROM flashsale")
        .fetch_all(db)
        .await
        .unwrap_or_default();
    for f in flashes {
        let code: String = f.try_get("code").unwrap_or_default();
        if code.is_empty() {
            continue;
        }
        let amount: f64 = f.try_get("amount").unwrap_or(0.0);
        let status: String = f.try_get("status").unwrap_or_else(|_| "0".to_string());
        lines.push(format!("[SYNC_FLASH] {} {} {}", code, amount, status));
    }

    // 3. Voucher (stok otoritatif ada di Topup DB; web hanya cermin untuk validasi UX)
    let vouchers = sqlx::query("SELECT code, voucher, discount, minimum, stock FROM voucher")
        .fetch_all(db)
        .await
        .unwrap_or_default();
    for v in vouchers {
        let voucher: String = v.try_get("voucher").unwrap_or_default();
        if voucher.is_empty() {
            continue;
        }
        let category = safe_token(&v.try_get::<String, _>("code").unwrap_or_default());
        let discount: f64 = v.try_get("discount").unwrap_or(0.0);
        let minimum: f64 = v.try_get("minimum").unwrap_or(0.0);
        let stock: i64 = v.try_get("stock").unwrap_or(0);
        let cat_token = if category.is_empty() { "-".to_string() } else { category };
        lines.push(format!(
            "[SYNC_VOUCHER] {} {} {} {} {}",
            cat_token, voucher, discount, minimum, stock
        ));
    }

    // 4. Level user (otoritas = DB Topup; web hanya cermin untuk tampilan harga)
    let users = sqlx::query("SELECT username, level FROM users")
        .fetch_all(db)
        .await
        .unwrap_or_default();
    for u in users {
        let username: String = u.try_get("username").unwrap_or_default();
        if username.is_empty() {
            continue;
        }
        let level: String = u.try_get("level").unwrap_or_else(|_| "Member".to_string());
        lines.push(format!("[SYNC_USER] {} {}", username, level));
    }

    Ok(lines)
}

/// Kirim batch terenkripsi ke grup listener Server Web.
async fn send_sync_batch(lines: Vec<String>) -> Result<usize, String> {
    if lines.is_empty() {
        return Ok(0);
    }

    // Bot 3 punya token sendiri; fallback ke token sender lama agar tetap jalan
    let bot_token = std::env::var("TELEGRAM_BOT_3_TOKEN")
        .or_else(|_| std::env::var("TELEGRAM_BOT_SENDER_TOKEN"))
        .or_else(|_| std::env::var("TELEGRAM_BOT_TOKEN"))
        .unwrap_or_default();
    let group_id = std::env::var("TELEGRAM_GROUP_2_ID").unwrap_or_default();

    if bot_token.is_empty() || group_id.is_empty() {
        return Err("TELEGRAM_BOT_3_TOKEN / TELEGRAM_GROUP_2_ID belum diset — sync dilewati".to_string());
    }

    // Chunking sama seperti sender utama (batas 3000 char/pesan)
    let mut chunks: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut len = 0usize;
    for line in lines {
        let l = line.len() + 1;
        if len + l > 3000 && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            len = 0;
        }
        len += l;
        current.push(line);
    }
    if !current.is_empty() {
        chunks.push(current);
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let url = format!("https://api.telegram.org/bot{}/sendMessage", bot_token);

    let mut total = 0usize;
    for chunk in chunks {
        total += chunk.len();
        let combined = chunk.join("\n");
        let encrypted = crate::domain::telegram::sender::encrypt_telegram_payload(&combined);
        let payload = serde_json::json!({ "chat_id": group_id, "text": encrypted });

        let mut sent = false;
        for attempt in 1..=3 {
            match client.post(&url).json(&payload).send().await {
                Ok(resp) if resp.status().is_success() => {
                    sent = true;
                    break;
                }
                Ok(resp) => {
                    let wait = if resp.status().as_u16() == 429 { 12 } else { 6 * attempt as u64 };
                    tracing::warn!("[SYNC BOT] Attempt {} failed ({})", attempt, resp.status());
                    tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                }
                Err(e) => {
                    tracing::warn!("[SYNC BOT] Attempt {} network error: {}", attempt, e);
                    tokio::time::sleep(std::time::Duration::from_secs(6 * attempt as u64)).await;
                }
            }
        }
        if !sent {
            return Err("Gagal mengirim chunk sync setelah 3 percobaan".to_string());
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }

    Ok(total)
}

/// Sync sekali jalan (dipakai worker periodik & CLI).
pub async fn sync_now(db: &MySqlPool) -> Result<usize, String> {
    let snapshot = build_sync_snapshot(db).await?;
    let count = snapshot.len();
    send_sync_batch(snapshot).await?;
    tracing::info!("[SYNC BOT] Pushed {} sync commands to Web Server", count);
    Ok(count)
}

/// Worker periodik Bot 3.
pub async fn start_price_sync_worker(db: MySqlPool) {
    let interval_secs = sync_interval_seconds();
    tracing::info!("[SYNC BOT] Starting price/promo sync worker (every {}s)...", interval_secs);
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
    // Sync awal saat startup agar web langsung selaras
    ticker.tick().await;
    if let Err(e) = sync_now(&db).await {
        tracing::warn!("[SYNC BOT] Initial sync failed: {}", e);
    }
    loop {
        ticker.tick().await;
        if let Err(e) = sync_now(&db).await {
            tracing::warn!("[SYNC BOT] Periodic sync failed: {}", e);
        }
    }
}
