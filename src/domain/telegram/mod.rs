pub mod gopay_bot;
pub mod report_bot;

use crate::models::Transaction;
use crate::providers::digiflazz::DigiFlazzClient;
use sqlx::{MySql, MySqlPool};

pub async fn process_paid_transaction(
    db: &MySqlPool,
    http_client: &reqwest::Client,
    order_id: &str,
    amount_received: f64,
) -> Result<(), String> {
    // A. Fetch Transaction from DB
    let order: Option<Transaction> = sqlx::query_as::<MySql, Transaction>("SELECT * FROM transaction WHERE order_id = ?")
        .bind(order_id)
        .fetch_optional(db)
        .await
        .map_err(|e| e.to_string())?;

    let order = order.ok_or("Transaction not found".to_string())?;

    // B. Verify Received Amount
    if amount_received > 0.0 && amount_received < order.price {
        tracing::error!("Underpaid invoice for Order {}: received {}, expected {}", order_id, amount_received, order.price);
        return Err("Underpaid invoice".to_string());
    }

    // C. Atomic Claim: Exclusive transition to processing (cegah race condition dobel proses)
    let claim_res = sqlx::query(
        "UPDATE transaction SET payment_status = 'paid', status = 'processing', updated_at = NOW() WHERE order_id = ? AND (payment_status = 'unpaid' OR (payment_status = 'paid' AND status = 'pending'))"
    )
    .bind(order_id)
    .execute(db)
    .await
    .map_err(|e| e.to_string())?;

    if claim_res.rows_affected() == 0 {
        tracing::warn!("Order {} already claimed or marked paid. Skipping duplicate topup dispatch.", order_id);
        return Ok(());
    }

    // D. Dispatch Topup Request to External Supplier API
    if order.provider == "DIGI" {
        let digi_username = std::env::var("DIGIFLAZZ_USERNAME").unwrap_or_default();
        let digi_apikey = std::env::var("DIGIFLAZZ_APIKEY").unwrap_or_default();
        let digi_client = DigiFlazzClient::new(digi_username, digi_apikey);

        let topup_res = digi_client
            .topup(http_client, &order.code, &order.target, &order.order_id)
            .await;

        match topup_res {
            Ok(res) if res.success && !res.is_pending => {
                // Sukses instan dari DigiFlazz
                let note = if !res.sn.is_empty() { res.sn } else { "Transaksi Sukses".to_string() };
                sqlx::query("UPDATE transaction SET order_tid = ?, status = 'success', note = ?, updated_at = NOW() WHERE order_id = ?")
                    .bind(&res.trxid)
                    .bind(&note)
                    .bind(&order.order_id)
                    .execute(db)
                    .await
                    .map_err(|e| e.to_string())?;

                // Broadcast status callback event back to Server Web
                let clean_sn = note.replace(' ', "_").replace(';', "");
                let _ = crate::domain::telegram::sender::send_report_to_fulfillment(&format!("[STATUS_UPDATE] {} success {}", order.order_id, clean_sn)).await;

                tracing::info!("Order {} instantly completed by DigiFlazz (TrxID: {}, SN: {})", order.order_id, res.trxid, note);
                Ok(())
            }
            Ok(res) if res.success && res.is_pending => {
                // Masih pending di DigiFlazz, serahkan ke background status poller
                sqlx::query("UPDATE transaction SET order_tid = ?, status = 'process', updated_at = NOW() WHERE order_id = ?")
                    .bind(&res.trxid)
                    .bind(&order.order_id)
                    .execute(db)
                    .await
                    .map_err(|e| e.to_string())?;

                let _ = crate::domain::telegram::sender::send_report_to_fulfillment(&format!("[STATUS_UPDATE] {} process Processing_Digiflazz", order.order_id)).await;

                tracing::info!("Order {} successfully submitted to DigiFlazz (TrxID: {}, Status: Pending)", order.order_id, res.trxid);
                Ok(())
            }
            Ok(res) => {
                tracing::warn!("DigiFlazz topup failed for Order {}: {}", order.order_id, res.message);
                
                // Ambil note terbaru dari database
                let current_note: Option<(Option<String>,)> = sqlx::query_as("SELECT note FROM transaction WHERE order_id = ?")
                    .bind(&order.order_id)
                    .fetch_optional(db)
                    .await
                    .unwrap_or(None);
                let note_str = current_note.and_then(|n| n.0).unwrap_or_default();
                
                // Auto Refund Saldo HANYA jika pembeli membayar via Saldo Akun (bukan gateway/GoPay)
                let is_balance_payment = order.user != "GUEST" && order.user != "-" && !order.user.is_empty() && (note_str.contains("Saldo") || note_str.starts_with("OTP:") || order.provider == "APP" || order.code == "APP");
                if is_balance_payment {
                    let mut refund_tx = db.begin().await.map_err(|e| e.to_string())?;
                    sqlx::query("UPDATE users SET balance = balance + ? WHERE username = ?")
                        .bind(order.price)
                        .bind(&order.user)
                        .execute(&mut *refund_tx)
                        .await
                        .map_err(|e| e.to_string())?;

                    sqlx::query("UPDATE transaction SET status = 'error', note = 'Gagal Topup - Saldo Dikembalikan', updated_at = NOW() WHERE order_id = ?")
                        .bind(&order.order_id)
                        .execute(&mut *refund_tx)
                        .await
                        .map_err(|e| e.to_string())?;
                    refund_tx.commit().await.map_err(|e| e.to_string())?;

                    // Broadcast refund event back to Server Web
                    let _ = crate::domain::telegram::sender::send_report_to_fulfillment(&format!("[REFUND_USER] {} {} {} Gagal_Topup", order.user, order.price, order.order_id)).await;
                    let _ = crate::domain::telegram::sender::send_report_to_fulfillment(&format!("[STATUS_UPDATE] {} error Gagal_Topup_Saldo_Dikembalikan", order.order_id)).await;

                    tracing::info!("Refunded Rp{} to user {} for failed order {}", order.price, order.user, order.order_id);
                } else {
                    sqlx::query("UPDATE transaction SET status = 'error', note = 'Gagal Topup Supplier - Hubungi CS/Admin', updated_at = NOW() WHERE order_id = ?")
                        .bind(&order.order_id)
                        .execute(db)
                        .await
                        .map_err(|e| e.to_string())?;

                    let _ = crate::domain::telegram::sender::send_report_to_fulfillment(&format!("[STATUS_UPDATE] {} error Gagal_Topup_Supplier", order.order_id)).await;
                    tracing::warn!("Order {} failed. Payment was via Gateway (flagged for CS/Admin without balance leak).", order.order_id);
                }

                Err(res.message)
            }
            Err(err) => Err(err.to_string()),
        }
    } else {
        sqlx::query("UPDATE transaction SET status = 'process', updated_at = NOW() WHERE order_id = ?")
            .bind(&order.order_id)
            .execute(db)
            .await
            .map_err(|e| e.to_string())?;

        Ok(())
    }
}
pub mod sender;
