use crate::models::Transaction;
use crate::providers::digiflazz::DigiFlazzClient;
use reqwest::Client;
use sqlx::{MySql, MySqlPool};
use std::time::Duration;
use tracing;

pub async fn start_status_poller_task(db: MySqlPool, http_client: Client) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            tracing::info!("[CRON WORKER] Checking pending transactions with DigiFlazz check_status...");

            let pending_orders = sqlx::query_as::<MySql, Transaction>(
                "SELECT * FROM transaction WHERE status = 'process' AND provider = 'DIGI' LIMIT 50",
            )
            .fetch_all(&db)
            .await;

            if let Ok(orders) = pending_orders {
                for order in orders {
                    let digi_username = std::env::var("DIGIFLAZZ_USERNAME").unwrap_or_default();
                    let digi_apikey = std::env::var("DIGIFLAZZ_APIKEY").unwrap_or_default();
                    let digi_client = DigiFlazzClient::new(digi_username, digi_apikey);

                    // Panggilan Cek Status yang sebenarnya (bukan repeat order)
                    let res = digi_client
                        .check_status(&http_client, &order.code, &order.target, &order.order_id)
                        .await;

                    match res {
                        Ok(status_res) if status_res.success => {
                            let note = if !status_res.sn.is_empty() {
                                status_res.sn
                            } else {
                                "Transaksi Sukses".to_string()
                            };

                            let _ = sqlx::query("UPDATE transaction SET status = 'success', note = ?, updated_at = NOW() WHERE order_id = ?")
                                .bind(&note)
                                .bind(&order.order_id)
                                .execute(&db)
                                .await;

                            let clean_sn = note.replace(' ', "_").replace(';', "");
                            let _ = crate::domain::telegram::sender::send_report_to_fulfillment(&format!("[STATUS_UPDATE] {} success {}", order.order_id, clean_sn)).await;

                            tracing::info!("[CRON WORKER] Order {} confirmed SUCCESS by DigiFlazz (SN: {})", order.order_id, note);
                        }
                        Ok(status_res) if status_res.is_failed => {
                            let note_str = order.note.as_deref().unwrap_or_default();
                            let is_balance = order.user != "GUEST" && order.user != "-" && !order.user.is_empty() && (note_str.contains("Saldo") || note_str.starts_with("OTP:") || order.provider == "APP" || order.code == "APP");
                            if is_balance {
                                if let Ok(mut tx) = db.begin().await {
                                    let _ = sqlx::query("UPDATE users SET balance = balance + ? WHERE username = ?")
                                        .bind(order.price)
                                        .bind(&order.user)
                                        .execute(&mut *tx)
                                        .await;

                                    let _ = sqlx::query("UPDATE transaction SET status = 'error', note = 'Gagal Topup - Saldo Dikembalikan', updated_at = NOW() WHERE order_id = ?")
                                        .bind(&order.order_id)
                                        .execute(&mut *tx)
                                        .await;
                                    let _ = tx.commit().await;

                                    let _ = crate::domain::telegram::sender::send_report_to_fulfillment(&format!("[REFUND_USER] {} {} {} Gagal_Topup", order.user, order.price, order.order_id)).await;
                                    let _ = crate::domain::telegram::sender::send_report_to_fulfillment(&format!("[STATUS_UPDATE] {} error Gagal_Topup_Saldo_Dikembalikan", order.order_id)).await;

                                    tracing::warn!("[CRON WORKER] Order {} FAILED & REFUNDED to user {}: {}", order.order_id, order.user, status_res.message);
                                }
                            } else {
                                let _ = sqlx::query("UPDATE transaction SET status = 'error', note = 'Gagal Supplier - Hubungi CS/Admin', updated_at = NOW() WHERE order_id = ?")
                                    .bind(&order.order_id)
                                    .execute(&db)
                                    .await;

                                let _ = crate::domain::telegram::sender::send_report_to_fulfillment(&format!("[STATUS_UPDATE] {} error Gagal_Topup_Supplier", order.order_id)).await;

                                tracing::warn!("[CRON WORKER] Order {} FAILED at supplier (Gateway payment - flagged without refund): {}", order.order_id, status_res.message);
                            }
                        }
                        _ => {
                            // Still pending in DigiFlazz, continue monitoring
                        }
                    }
                }
            }
        }
    });
}

