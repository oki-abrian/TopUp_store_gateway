use sqlx::MySqlPool;
use std::time::Duration;
use tracing;

pub async fn start_expired_cleaner_task(db: MySqlPool) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            tracing::info!("[CRON WORKER] Cleaning expired unpaid transactions & deposits...");

            let res_trx = sqlx::query(
                "UPDATE transaction SET status = 'error', payment_status = 'expired', note = 'Expired otomatis sistem' WHERE payment_status = 'unpaid' AND created_at < DATE_SUB(NOW(), INTERVAL 15 MINUTE)"
            )
            .execute(&db)
            .await;

            if let Ok(res) = res_trx {
                if res.rows_affected() > 0 {
                    tracing::info!("[CRON WORKER] Expired {} unpaid transactions.", res.rows_affected());
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
