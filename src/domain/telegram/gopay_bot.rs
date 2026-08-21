use std::sync::Arc;
use sqlx::{MySqlPool, Row};
use teloxide::prelude::*;

async fn handle_gopay_message(
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
        tracing::info!("[GOPAY BOT] Rejected non-text message.");
        crate::domain::telegram::report_bot::delayed_delete(&bot, msg.chat.id, msg.id);
        return Ok(());
    }

    if let Some(text) = msg.text() {
        tracing::info!("[GOPAY BOT] Received text: {}", text);
        
        let mut detected_amount: Option<f64> = None;
        let words: Vec<&str> = text.split_whitespace().collect();
        for (i, word) in words.iter().enumerate() {
            let lw = word.to_lowercase();
            if lw.starts_with("rp") {
                let stripped = lw.trim_start_matches("rp").replace('.', "").replace(',', "");
                if let Ok(amt) = stripped.parse::<f64>() {
                    if amt > 0.0 {
                        detected_amount = Some(amt);
                        break;
                    }
                } else if i + 1 < words.len() {
                    let next_stripped = words[i + 1].replace('.', "").replace(',', "");
                    if let Ok(amt) = next_stripped.parse::<f64>() {
                        if amt > 0.0 {
                            detected_amount = Some(amt);
                            break;
                        }
                    }
                }
            }
        }

        if let Some(amount) = detected_amount {
            tracing::info!("[GOPAY BOT] Detected GoPay amount: {}", amount);

            // Matching GoPay Unpaid Order (FIFO: order terlama lebih dulu)
            let order_query = "SELECT order_id, price FROM transaction WHERE (provider = 'QR_LANN' OR code LIKE '%GOPAY%' OR code LIKE '%QRIS%' OR service_name LIKE '%GOPAY%' OR service_name LIKE '%QRIS%') AND payment_status = 'unpaid' AND (price = ? OR (price + profit) = ?) ORDER BY id ASC LIMIT 1";
            if let Ok(Some(row)) = sqlx::query(order_query).bind(amount).bind(amount).fetch_optional(db_pool).await {
                let order_id: String = row.try_get("order_id").unwrap_or_default();
                
                tracing::info!("[GOPAY BOT] Matched order {} for GoPay amount {}. Calling process_paid_transaction...", order_id, amount);
                let _ = bot.send_message(msg.chat.id, format!("✅ Memproses pesanan untuk GoPay Rp{} (Order: {})", amount, order_id)).await;
                
                // Call digiflazz through exclusive atomic claim
                let client = reqwest::Client::new();
                match crate::domain::telegram::process_paid_transaction(db_pool, &client, &order_id, amount).await {
                    Ok(_) => {
                        let _ = bot.send_message(msg.chat.id, format!("🎉 Order {} berhasil di-Topup!", order_id)).await;
                    }
                    Err(e) => {
                        let _ = bot.send_message(msg.chat.id, format!("❌ Gagal memproses Order {}: {}", order_id, e)).await;
                    }
                }
            } else {
                tracing::warn!("[GOPAY BOT] Amount Rp{} not found or already paid in DB.", amount);
            }
        }
        
        // Auto-delete the original notification to keep group clean
        crate::domain::telegram::report_bot::delayed_delete(&bot, msg.chat.id, msg.id);
    }

    Ok(())
}

pub async fn start_gopay_bot(db_pool: MySqlPool, token: String, group_id: String) {
    tracing::info!("[GOPAY BOT] Starting bot thread...");
    let bot = Bot::new(token);
    let state = Arc::new((db_pool, group_id));
    
    let handler = dptree::endpoint(handle_gopay_message);
    
    // We cannot use enable_ctrlc_handler here because we are running multiple dispatchers
    // inside a tokio runtime, and only one ctrlc handler can be registered per process.
    let mut dispatcher = Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![state])
        .build();
        
    dispatcher.dispatch().await;
}
