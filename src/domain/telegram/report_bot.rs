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

// ============================================================
// DEDUP IN-PROCESS (batch Telegram dibaca serial, tapi pesan
// berbeda bisa diproses paralel oleh teloxide). Guard ini
// mencegah [NEW_ORDER]/[REQ_OTP] yang sama diproses dua kali
// dalam jendela 10 menit — lapisan pertama sebelum klaim DB.
// ============================================================
static SEEN_COMMANDS: std::sync::LazyLock<dashmap::DashMap<String, i64>> =
    std::sync::LazyLock::new(|| dashmap::DashMap::new());

fn seen_recently(cmd_type: &str, order_id: &str) -> bool {
    let key = format!("{}|{}", cmd_type, order_id);
    let now = chrono::Utc::now().timestamp();
    match SEEN_COMMANDS.get(&key) {
        Some(v) if now - *v < 600 => true,
        _ => {
            SEEN_COMMANDS.insert(key, now);
            false
        }
    }
}

// ============================================================
// L1 ANTI-BRUTEFORCE KUPON (lapis batch Telegram)
// Dalam SATU batch pesan, dari N pesanan identik (username+voucher)
// hanya PERTAMA yang lolos diproses; sisanya langsung DITOLAK di
// lapis batch — tanpa query DB, tanpa biaya kuota tambahan.
// TANPA batasan waktu: kirim ulang di batch lain adalah urusan
// validasi Server Topup (L2/L3), bukan urusan L1.
// ============================================================

// ============================================================
// L3 ANTI-REPLAY (cek Server Topup, jaga-jaga Telegram dibajak):
// apakah voucher ini SUDAH dipakai user yang sama pada transaksi
// lain yang masih aktif / sukses?
// ============================================================
async fn voucher_already_used(db: &MySqlPool, username: &str, voucher_code: &str, exclude_order_id: &str) -> bool {
    if voucher_code.is_empty() || voucher_code == "-" || username.is_empty() || username == "-" {
        return false;
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM transaction WHERE user = ? AND voucher = ? AND order_id <> ? AND status IN ('pending','process','processing','success')",
    )
    .bind(username)
    .bind(voucher_code)
    .bind(exclude_order_id)
    .fetch_one(db)
    .await
    .unwrap_or(0);
    count > 0
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

/// Hapus pesan Telegram secara TERTUNDA (180 detik).
/// Memberi waktu semua listener (web & fulfillment) membaca envelope sebelum
/// dihapus — mencegah race condition "terhapus sebelum dibaca".
pub fn delayed_delete(bot: &teloxide::Bot, chat_id: teloxide::types::ChatId, message_id: teloxide::types::MessageId) {
    let bot = bot.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(180)).await;
        let _ = bot.delete_message(chat_id, message_id).await;
    });
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
        crate::domain::telegram::report_bot::delayed_delete(&bot, msg.chat.id, msg.id);
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
                    crate::domain::telegram::report_bot::delayed_delete(&bot, msg.chat.id, msg.id);
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
                crate::domain::telegram::report_bot::delayed_delete(&bot, msg.chat.id, msg.id);
                return Ok(());
            }
        };

        // Split multi-line batch commands and execute each command
        // L1: set (username|voucher) yang sudah lolos DALAM batch ini saja.
        let mut l1_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for line in decrypted_commands.lines() {
            let cmd_text = line.trim();
            if cmd_text.is_empty() {
                continue;
            }
            process_single_command(&bot, msg.chat.id, db_pool, cmd_text, &mut l1_seen).await;
        }

        // Auto-delete the envelope to keep group clean
        crate::domain::telegram::report_bot::delayed_delete(&bot, msg.chat.id, msg.id);
    }

    Ok(())
}

async fn process_single_command(
    bot: &Bot,
    chat_id: teloxide::types::ChatId,
    db_pool: &MySqlPool,
    text: &str,
    l1_seen: &mut std::collections::HashSet<String>,
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
            let voucher_code = if parts.len() >= 7 { parts[6] } else { "-" };

            if !is_safe_string(order_id) || !is_safe_string(username) || !is_safe_string(service_code) {
                tracing::warn!("[REPORT BOT] Blocked hack attempt! Unsafe characters in REQ_OTP payload.");
                return;
            }

            // Dedup: [REQ_OTP] yang sama untuk order yang sama diabaikan
            if seen_recently("REQ_OTP", order_id) {
                tracing::info!("[REPORT BOT] Duplicate REQ_OTP for {} ignored (dedup).", order_id);
                return;
            }

            // ===== L1: duplikat (username+voucher) DALAM batch ini → tolak instan tanpa DB
            let has_voucher = voucher_code != "-" && !voucher_code.is_empty();
            if has_voucher {
                let l1_key = format!("REQ_OTP|{}|{}", username, voucher_code);
                if !l1_seen.insert(l1_key) {
                    tracing::warn!("[REPORT BOT] L1: REQ_OTP {} ditolak — voucher {} duplikat oleh user {} dalam batch yang sama", order_id, voucher_code, username);
                    let _ = crate::domain::telegram::sender::send_report_to_fulfillment(
                        &format!("[ORDER_REJECTED] {} Voucher_Duplikat", order_id),
                    ).await;
                    return;
                }
            }

            // ANTI DUPLIKASI: jika [NEW_ORDER] sudah memproses order ini dengan voucher
            // yang sama, harga FINAL sudah terkunci di DB — jangan hitung ulang dan
            // jangan sentuh stok (stok bisa saja sudah habis → harga salah naik).
            let mut reuse_price: Option<f64> = None;
            if voucher_code != "-" && !voucher_code.is_empty() {
                if let Ok(Some(prev_row)) = sqlx::query(
                    "SELECT price, COALESCE(voucher, '') AS v FROM transaction WHERE order_id = ? LIMIT 1",
                )
                .bind(order_id)
                .fetch_optional(db_pool)
                .await
                {
                    let prev_voucher: String = prev_row.try_get("v").unwrap_or_default();
                    if prev_voucher == voucher_code {
                        reuse_price = Some(prev_row.try_get("price").unwrap_or(0.0));
                    }
                }
            }

            let mut resolved = match crate::domain::orders::pricing::resolve_price(db_pool, service_code, username, voucher_code).await {
                Ok(r) => Some(r),
                Err(e) => {
                    if reuse_price.is_none() {
                        tracing::error!("[REPORT BOT] Rejecting order {}: {}", order_id, e);
                        let _ = bot.send_message(chat_id, format!("❌ Gagal memproses {}: {}", order_id, e)).await;
                        return;
                    }
                    None
                }
            };

            // ===== L3 anti-replay (lewati bila harga sudah terkunci handler pertama)
            if reuse_price.is_none() {
                if let Some(ref rr) = resolved {
                    if !rr.voucher_code.is_empty()
                        && voucher_already_used(db_pool, username, &rr.voucher_code, order_id).await
                    {
                        tracing::warn!("[REPORT BOT] L3: REQ_OTP {} — voucher {} sudah terpakai oleh {}, diskon dicabut", order_id, rr.voucher_code, username);
                        resolved = match crate::domain::orders::pricing::resolve_price(db_pool, service_code, username, "-").await {
                            Ok(r2) => Some(r2),
                            Err(_) => None,
                        };
                    }
                }
            }

            // ANTI DOUBLE-SPEND: klaim idempoten per-order; hanya pemenang klaim yang
            // memotong stok. Kalah klaim = pesan duplikat → pakai harga yang sudah ada.
            if reuse_price.is_none() {
                if let Some(ref r) = resolved {
                    if !r.voucher_code.is_empty() {
                        if crate::domain::orders::pricing::claim_order_voucher(db_pool, order_id, &r.voucher_code).await {
                    if !crate::domain::orders::pricing::try_consume_voucher(db_pool, &r.voucher_code).await {
                        if let Ok(r2) = crate::domain::orders::pricing::resolve_price(db_pool, service_code, username, "-").await {
                            let new_price = r2.final_price;
                            resolved = Some(r2);
                            crate::domain::orders::pricing::release_order_voucher(db_pool, order_id).await;
                            let _ = sqlx::query("UPDATE transaction SET price = ? WHERE order_id = ?")
                                .bind(new_price)
                                .bind(order_id)
                                .execute(db_pool)
                                .await;
                            tracing::warn!("[REPORT BOT] REQ_OTP {}: voucher stock gone, recalculated price without voucher → {}", order_id, new_price);
                        }
                    }
                        } else {
                            tracing::info!("[REPORT BOT] REQ_OTP {}: voucher {} already claimed for this order, skipping stock decrement", order_id, r.voucher_code);
                        }
                    }
                }
            }

            let (real_price, provider, service_name): (f64, String, String) = if let Some(p) = reuse_price {
                // Harga terkunci oleh handler pertama; ambil metadata layanan tanpa ubah harga.
                let meta = crate::domain::orders::pricing::resolve_price(db_pool, service_code, username, "-").await.ok();
                (
                    p,
                    meta.as_ref().map(|m| m.provider.clone()).unwrap_or_else(|| "DIGI".to_string()),
                    meta.as_ref().map(|m| m.service_name.clone()).unwrap_or_else(|| service_code.to_string()),
                )
            } else if let Some(r) = resolved.clone() {
                (r.final_price, r.provider.clone(), r.service_name.clone())
            } else {
                // Tidak ada harga fallback & layanan tak dikenal → tolak order ini
                tracing::error!("[REPORT BOT] REQ_OTP {}: service {} unknown in Topup DB. Rejected.", order_id, service_code);
                let _ = bot.send_message(chat_id, format!("❌ Order {} ditolak: produk tidak dikenal di Server Topup.", order_id)).await;
                let _ = crate::domain::telegram::sender::send_report_to_fulfillment(
                    &format!("[ORDER_REJECTED] {} Produk_Tidak_Dikenal", order_id),
                ).await;
                return;
            };

            tracing::info!("[REPORT BOT] REQ_OTP received: {} for {} (Provider: {}). Authoritative Price: {}", order_id, username, provider, real_price);

            let _ = sqlx::query(
                "INSERT INTO transaction (order_id, user, code, service_name, target, price, status, payment_status, provider, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, 'pending', 'unpaid', ?, NOW(), NOW()) ON DUPLICATE KEY UPDATE price = VALUES(price), provider = VALUES(provider), updated_at = NOW()"
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

            // CATATAN: transaction.voucher sudah di-set oleh claim_order_voucher()
            // pada handler yang memenangkan klaim — tidak perlu ditulis ulang di sini.

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
            let mut provider = parts[5].to_string();
            // Format baru (opsional): price_fallback name_underscored voucher
            let price_fallback: f64 = if parts.len() >= 7 { parts[6].parse().unwrap_or(0.0) } else { 0.0 };
            let name_fallback: String = if parts.len() >= 8 { parts[7].replace('_', " ") } else { service_code.to_string() };
            let voucher_code = if parts.len() >= 9 { parts[8] } else { "-" };

            if !is_safe_string(order_id) || !is_safe_string(username) || !is_safe_string(service_code) || !is_safe_string(&provider) {
                tracing::warn!("[REPORT BOT] Blocked hack attempt! Unsafe characters in NEW_ORDER payload.");
                return;
            }

            // Dedup: [NEW_ORDER] yang sama untuk order yang sama diabaikan
            if seen_recently("NEW_ORDER", order_id) {
                tracing::info!("[REPORT BOT] Duplicate NEW_ORDER for {} ignored (dedup).", order_id);
                return;
            }

            // ===== L1: duplikat (username+voucher) DALAM batch ini → TOLAK instan.
            // Tanpa bot.send_message & tanpa query DB — nol biaya kuota per duplikat.
            // Kirim ulang di batch lain = urusan L2/L3 Server Topup, bukan L1.
            let has_voucher = voucher_code != "-" && !voucher_code.is_empty();
            if has_voucher {
                let l1_key = format!("NEW_ORDER|{}|{}", username, voucher_code);
                if !l1_seen.insert(l1_key) {
                    tracing::warn!("[REPORT BOT] L1: order {} ditolak — voucher {} duplikat oleh user {} dalam batch yang sama", order_id, voucher_code, username);
                    let _ = crate::domain::telegram::sender::send_report_to_fulfillment(
                        &format!("[ORDER_REJECTED] {} Voucher_Duplikat", order_id),
                    ).await;
                    return;
                }
            }

            // Harga OTORITATIF dari DB Topup — nilai dari web hanya fallback bila layanan
            // tidak dikenal (dan order tetap ditolak bila layanan benar-benar tidak ada).
            let mut resolved = crate::domain::orders::pricing::resolve_price(db_pool, service_code, username, voucher_code).await.ok();

            // ===== L3 anti-replay: voucher sudah dipakai user di transaksi lain aktif?
            if let Some(ref rr) = resolved {
                if !rr.voucher_code.is_empty()
                    && voucher_already_used(db_pool, username, &rr.voucher_code, order_id).await
                {
                    tracing::warn!("[REPORT BOT] L3: order {} — voucher {} sudah terpakai oleh {}, diskon dicabut", order_id, rr.voucher_code, username);
                    resolved = match crate::domain::orders::pricing::resolve_price(db_pool, service_code, username, "-").await {
                        Ok(r2) => Some(r2),
                        Err(_) => None,
                    };
                }
            }

            let (mut real_price, service_name): (f64, String) = if let Some(ref r) = resolved {
                if !r.provider.is_empty() {
                    provider = r.provider.clone();
                }
                tracing::info!("[REPORT BOT] NEW_ORDER {}: authoritative price {} (base {}, margin {}, flash {}, voucher {})", order_id, r.final_price, r.base, r.margin, r.flash_discount, r.voucher_discount);
                (r.final_price, r.service_name.clone())
            } else if price_fallback > 0.0 {
                tracing::warn!("[REPORT BOT] NEW_ORDER {}: service not in Topup DB, using web fallback price {}", order_id, price_fallback);
                (price_fallback, name_fallback)
            } else {
                // TOLAK order: beri tahu Server Web agar invoice gateway TIDAK dibuat
                tracing::error!("[REPORT BOT] NEW_ORDER {}: service {} unknown in Topup DB and no fallback price. Rejected.", order_id, service_code);
                let _ = bot.send_message(chat_id, format!("❌ Order {} ditolak: produk {} tidak dikenal di Server Topup.", order_id, service_code)).await;
                let _ = crate::domain::telegram::sender::send_report_to_fulfillment(
                    &format!("[ORDER_REJECTED] {} Produk_Tidak_Dikenal", order_id),
                ).await;
                return;
            };

            if provider.is_empty() {
                provider = "DIGI".to_string();
            }

            tracing::info!("[REPORT BOT] NEW_ORDER received: {} for {} (Price: {}, Provider: {})", order_id, username, real_price, provider);

            let _ = sqlx::query(
                "INSERT INTO transaction (order_id, user, code, service_name, target, price, status, payment_status, provider, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, 'pending', 'unpaid', ?, NOW(), NOW()) ON DUPLICATE KEY UPDATE price = VALUES(price), service_name = VALUES(service_name), provider = VALUES(provider), updated_at = NOW()"
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

            // ANTI DOUBLE-SPEND + ANTI DUPLIKASI PESAN:
            // 1) Klaim slot voucher pada order ini (idempoten — handler kedua utk order
            //    yang sama akan kalah klaim dan TIDAK memotong stok lagi).
            // 2) Yang menang klaim memotong stok secara atomik; kalah race stok
            //    (habis) → harga dihitung ulang TANPA voucher.
            if let Some(ref r) = resolved {
                if !r.voucher_code.is_empty() {
                    if crate::domain::orders::pricing::claim_order_voucher(db_pool, order_id, &r.voucher_code).await {
                        if crate::domain::orders::pricing::try_consume_voucher(db_pool, &r.voucher_code).await {
                            tracing::info!("[REPORT BOT] NEW_ORDER {}: voucher {} applied", order_id, r.voucher_code);
                        } else if let Ok(r2) = crate::domain::orders::pricing::resolve_price(db_pool, service_code, username, "-").await {
                            real_price = r2.final_price;
                            crate::domain::orders::pricing::release_order_voucher(db_pool, order_id).await;
                            let _ = sqlx::query("UPDATE transaction SET price = ? WHERE order_id = ?")
                                .bind(real_price)
                                .bind(order_id)
                                .execute(db_pool)
                                .await;
                            tracing::warn!("[REPORT BOT] NEW_ORDER {}: voucher stock gone, recalculated price without voucher → {}", order_id, real_price);
                        }
                    } else {
                        tracing::info!("[REPORT BOT] NEW_ORDER {}: voucher {} already claimed for this order (duplicate message), skipping stock decrement", order_id, r.voucher_code);
                    }
                }
            }

            // ACCEPT: beri tahu Server Web bahwa order lolos validasi otoritatif.
            // Web baru akan membuat invoice gateway SETELAH pesan ini diterima.
            let _ = crate::domain::telegram::sender::send_report_to_fulfillment(
                &format!("[ORDER_ACCEPTED] {} {}", order_id, real_price),
            ).await;
            tracing::info!("[REPORT BOT] Order {} ACCEPTED at Rp{}", order_id, real_price);
        }
    } else if text.starts_with("[API_ORDER]") {
        // ============================================================
        // ORDER API RESELLER — dua fase, otoritas penuh di Server Topup.
        // Web HANYA membuat baris order unpaid + broadcast; potong saldo,
        // validasi harga, dan topup SEMUA terjadi di sini (DB Topup).
        // Format: [API_ORDER] oid username code target provider price name voucher(-)
        // ============================================================
        let parts: Vec<&str> = text.split_whitespace().collect();
        if parts.len() >= 8 {
            let order_id = parts[1];
            let username = parts[2];
            let service_code = parts[3];
            let target_game = parts[4];
            let mut provider = parts[5].to_string();
            let _price_fallback: f64 = parts[6].parse().unwrap_or(0.0);

            if !is_safe_string(order_id) || !is_safe_string(username) || !is_safe_string(service_code) {
                tracing::warn!("[REPORT BOT] Blocked hack attempt! Unsafe characters in API_ORDER payload.");
                return;
            }

            // Dedup: [API_ORDER] yang sama untuk order yang sama diabaikan
            if seen_recently("API_ORDER", order_id) {
                tracing::info!("[REPORT BOT] Duplicate API_ORDER for {} ignored (dedup).", order_id);
                return;
            }

            // Harga OTORITATIF dari DB Topup (tanpa voucher untuk API)
            let (real_price, api_service_name): (f64, String) = match crate::domain::orders::pricing::resolve_price(db_pool, service_code, username, "-").await {
                Ok(r) => {
                    tracing::info!("[REPORT BOT] API_ORDER {}: authoritative price {}", order_id, r.final_price);
                    (r.final_price, r.service_name)
                }
                Err(_) => {
                    tracing::error!("[REPORT BOT] API_ORDER {}: produk {} tidak dikenal di Topup DB. Rejected.", order_id, service_code);
                    let _ = crate::domain::telegram::sender::send_report_to_fulfillment(
                        &format!("[ORDER_REJECTED] {} Produk_Tidak_Dikenal", order_id),
                    ).await;
                    return;
                }
            };

            if provider.is_empty() {
                provider = "DIGI".to_string();
            }

            // Upsert baris transaksi di DB Topup (idempoten)
            let _ = sqlx::query(
                "INSERT INTO transaction (order_id, user, code, service_name, target, price, status, payment_status, provider, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, 'pending', 'unpaid', ?, NOW(), NOW()) ON DUPLICATE KEY UPDATE price = VALUES(price), provider = VALUES(provider), updated_at = NOW()"
            )
            .bind(order_id)
            .bind(username)
            .bind(service_code)
            .bind(&api_service_name)
            .bind(target_game)
            .bind(real_price)
            .bind(&provider)
            .execute(db_pool)
            .await;

            // Idempotensi eksekusi: jangan proses ulang order yang sudah dibayar/tuntas
            if let Ok(Some(row)) = sqlx::query("SELECT status, payment_status FROM transaction WHERE order_id = ? LIMIT 1")
                .bind(order_id)
                .fetch_optional(db_pool)
                .await
            {
                use sqlx::Row;
                let st: String = row.try_get("status").unwrap_or_default();
                let pay: String = row.try_get("payment_status").unwrap_or_default();
                if pay == "paid" || st == "success" || st == "process" || st == "processing" {
                    tracing::info!("[REPORT BOT] API_ORDER {}: already processed ({}), ack only.", order_id, st);
                    return;
                }
            }

            // POTONG SALDO ATOMIK di DB Topup (paritas dengan alur SALDO/OTP)
            let deduct = sqlx::query("UPDATE users SET balance = balance - ? WHERE username = ? AND balance >= ?")
                .bind(real_price)
                .bind(username)
                .bind(real_price)
                .execute(db_pool)
                .await;

            match deduct {
                Ok(d) if d.rows_affected() > 0 => {
                    let _ = sqlx::query("INSERT INTO mutation (username, type, amount, note, date_cr) VALUES (?, '-', ?, ?, NOW())")
                        .bind(username)
                        .bind(real_price)
                        .bind(format!("Order API :: {}", order_id))
                        .execute(db_pool)
                        .await;

                    let _ = sqlx::query("UPDATE transaction SET payment_status = 'paid', updated_at = NOW() WHERE order_id = ?")
                        .bind(order_id)
                        .execute(db_pool)
                        .await;

                    tracing::info!("[REPORT BOT] API_ORDER {}: saldo {} dipotong Rp{}. Memproses topup...", username, order_id, real_price);
                    let client = reqwest::Client::new();
                    if let Err(e) = crate::domain::telegram::process_paid_transaction(db_pool, &client, order_id, real_price).await {
                        tracing::error!("[REPORT BOT] API_ORDER {}: gagal topup: {}", order_id, e);
                    }
                }
                _ => {
                    tracing::warn!("[REPORT BOT] API_ORDER {}: saldo user {} tidak cukup (butuh Rp{})", order_id, username, real_price);
                    let _ = sqlx::query("UPDATE transaction SET status = 'error', payment_status = 'unpaid', updated_at = NOW() WHERE order_id = ?")
                        .bind(order_id)
                        .execute(db_pool)
                        .await;
                    let _ = crate::domain::telegram::sender::send_report_to_fulfillment(
                        &format!("[ORDER_REJECTED] {} Saldo_Tidak_Cukup", order_id),
                    ).await;
                }
            }
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

                                        // CATATAN: stok voucher sudah dikonsumsi ATOMIK saat order dibuat,
                                        // bukan di sini (mencegah double-decrement).

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
        
        let mut row_opt = None;
        for attempt in 1..=3 {
            if let Ok(Some(row)) = sqlx::query(order_query).bind(order_id).fetch_optional(db_pool).await {
                row_opt = Some(row);
                break;
            }
            if attempt < 3 {
                // Jeda 1.5 detik jika webhook masuk bersamaan dengan pembuatan order (anti race-condition)
                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            }
        }

        if let Some(row) = row_opt {
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
            tracing::warn!("[REPORT BOT] Order {} not found or already processed after 3 lookup attempts.", order_id);
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

