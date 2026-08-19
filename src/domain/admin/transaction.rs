use serde::Deserialize;
use sqlx::MySqlPool;

#[derive(Debug, Deserialize)]
pub struct AdminUpdateTransactionStatusRequest {
    pub invoice: String,
    pub status: String, // pending, processing, success, error
}

pub async fn update_transaction_status(db: &MySqlPool, req: AdminUpdateTransactionStatusRequest) -> Result<(), crate::error::AppError> {
    sqlx::query("UPDATE transaction SET status = ? WHERE web_invoice = ?")
        .bind(&req.status)
        .bind(&req.invoice)
        .execute(db)
        .await?;
        
    // Optionally trigger WhatsApp notification
    
    Ok(())
}
