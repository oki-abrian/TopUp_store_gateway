use serde::Deserialize;
use sqlx::MySqlPool;

#[derive(Debug, Deserialize)]
pub struct AdminUpdateTransactionStatusRequest {
    pub invoice: String,
    pub status: String, // pending, process, success, error, system
}

#[derive(Debug, Deserialize)]
pub struct AdminUpdatePaymentStatusRequest {
    pub invoice: String,
    pub status: String, // unpaid, paid, cancel, refund
}

#[derive(Debug, Deserialize)]
pub struct AdminUpdateTransactionNoteRequest {
    pub invoice: String,
    pub note: String,
}

pub async fn update_transaction_status(db: &MySqlPool, req: AdminUpdateTransactionStatusRequest) -> Result<(), crate::error::AppError> {
    sqlx::query("UPDATE transaction SET status = ?, updated_at = NOW() WHERE order_id = ?")
        .bind(&req.status)
        .bind(&req.invoice)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn update_payment_status(db: &MySqlPool, req: AdminUpdatePaymentStatusRequest) -> Result<(), crate::error::AppError> {
    sqlx::query("UPDATE transaction SET payment_status = ?, updated_at = NOW() WHERE order_id = ?")
        .bind(&req.status)
        .bind(&req.invoice)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn update_transaction_note(db: &MySqlPool, req: AdminUpdateTransactionNoteRequest) -> Result<(), crate::error::AppError> {
    sqlx::query("UPDATE transaction SET note = ?, updated_at = NOW() WHERE order_id = ?")
        .bind(&req.note)
        .bind(&req.invoice)
        .execute(db)
        .await?;
    Ok(())
}
