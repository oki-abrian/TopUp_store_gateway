use sqlx::MySqlPool;
use sqlx::Row;
use std::time::Duration;
use tracing;

pub async fn start_expired_cleaner_task(db: MySqlPool) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            tracing::info!("[CRON WORKER] Cleaning expired unpaid transactions & deposits...");

            // Expire per-order (bukan bulk UPDATE) agar restock voucher tidak dobel:
            // baris dipilih SEKALI lalu langsung ditandai expired pada order yang sama.
            let stale = sqlx::query(
                "SELECT order_id, COALESCE(voucher, '') AS voucher FROM transaction WHERE payment_status = 'unpaid' AND created_at < DATE_SUB(NOW(), INTERVAL 15 MINUTE)"
            )
            .fetch_all(&db)
            .await
            .unwrap_or_default();

            for row in stale {
                let order_id: String = row.try_get("order_id").unwrap_or_default();
                let voucher: String = row.try_get("voucher").unwrap_or_default();

                // Guard idempoten: hanya transisi unpaid → expired yang diproses
                let res = sqlx::query(
                    "UPDATE transaction SET status = 'error', payment_status = 'expired', note = 'Expired otomatis sistem', updated_at = NOW() WHERE order_id = ? AND payment_status = 'unpaid'"
                )
                .bind(&order_id)
                .execute(&db)
                .await;

                if let Ok(r) = res {
                    if r.rows_affected() > 0 && !voucher.is_empty() {
                        crate::domain::orders::pricing::restock_voucher(&db, &voucher).await;
                    }
                }
            }

            let res_depo = sqlx::query(
                "UPDATE deposit SET status = 'canceled' WHERE status IN ('unpaid', 'pending') AND (date < DATE_SUB(NOW(), INTERVAL 15 MINUTE) OR created_at < DATE_SUB(NOW(), INTERVAL 15 MINUTE))"
            )
            .execute(&db)
            .await;

            if let Ok(res) = res_depo {
                if res.rows_affected() > 0 {
                    tracing::info!("[CRON WORKER] Expired {} pending deposits.", res.rows_affected());
                }
            }
        }
    });
}
