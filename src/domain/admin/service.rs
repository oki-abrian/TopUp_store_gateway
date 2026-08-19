use crate::error::AppError;
use crate::models::User;
use serde::{Deserialize, Serialize};
use sqlx::{MySql, MySqlPool};

#[derive(Debug, Deserialize)]
pub struct AdminCreateServiceRequest {
    pub code: String,
    pub name: String,
    pub game: String,
    pub type_name: String,
    pub price: f64,
    pub member_profit: f64,
    pub reseller_profit: f64,
    pub provider: String,
}

#[derive(Debug, Deserialize)]
pub struct AdminUpdateUserBalanceRequest {
    pub username: String,
    pub action: String, // add or deduct
    pub amount: f64,
    pub note: String,
}

#[derive(Debug, Deserialize)]
pub struct AdminLockAccountRequest {
    pub username: String,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct FinancialSummary {
    pub total_transactions: i64,
    pub total_sales_volume: f64,
    pub total_gross_profit: f64,
    pub pending_orders_count: i64,
    pub success_orders_count: i64,
}

pub async fn admin_create_service(db: &MySqlPool, req: AdminCreateServiceRequest) -> Result<bool, AppError> {
    sqlx::query(
        "INSERT INTO service (code, name, game, type, price, member, reseller, provider, status) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'available')"
    )
    .bind(&req.code)
    .bind(&req.name)
    .bind(&req.game)
    .bind(&req.type_name)
    .bind(req.price)
    .bind(req.member_profit)
    .bind(req.reseller_profit)
    .bind(&req.provider)
    .execute(db)
    .await?;

    Ok(true)
}

pub async fn admin_adjust_user_balance(
    db: &MySqlPool,
    req: AdminUpdateUserBalanceRequest,
) -> Result<f64, AppError> {
    let mut tx = db.begin().await?;

    let user_opt: Option<User> = sqlx::query_as::<MySql, User>("SELECT * FROM users WHERE username = ? FOR UPDATE")
        .bind(&req.username)
        .fetch_optional(&mut *tx)
        .await?;

    let user = user_opt.ok_or(AppError::UserNotFound)?;

    let (new_balance, mutation_type) = if req.action == "add" {
        (user.balance + req.amount, "+")
    } else {
        (user.balance - req.amount, "-")
    };

    sqlx::query("UPDATE users SET balance = ? WHERE username = ?")
        .bind(new_balance)
        .bind(&req.username)
        .execute(&mut *tx)
        .await?;

    sqlx::query("INSERT INTO mutation (username, type, amount, note, date_cr) VALUES (?, ?, ?, ?, NOW())")
        .bind(&req.username)
        .bind(mutation_type)
        .bind(req.amount)
        .bind(&req.note)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(new_balance)
}

pub async fn admin_lock_user_account(db: &MySqlPool, req: AdminLockAccountRequest) -> Result<bool, AppError> {
    sqlx::query("DELETE FROM users_lock WHERE user = ?")
        .bind(&req.username)
        .execute(db)
        .await?;

    sqlx::query("INSERT INTO users_lock (user, reason, date_cr) VALUES (?, ?, NOW())")
        .bind(&req.username)
        .bind(&req.reason)
        .execute(db)
        .await?;

    Ok(true)
}

pub async fn admin_get_financial_summary(db: &MySqlPool) -> Result<FinancialSummary, AppError> {
    let total_transactions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transaction")
        .fetch_one(db)
        .await
        .unwrap_or(0);
    let total_sales_volume: f64 = sqlx::query_scalar("SELECT COALESCE(SUM(price),0) FROM transaction WHERE status = 'success'")
        .fetch_one(db)
        .await
        .unwrap_or(0.0);
    let total_gross_profit: f64 = sqlx::query_scalar("SELECT COALESCE(SUM(profit),0) FROM transaction WHERE status = 'success'")
        .fetch_one(db)
        .await
        .unwrap_or(0.0);
    let pending_orders_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transaction WHERE status IN ('pending','process','system')")
        .fetch_one(db)
        .await
        .unwrap_or(0);
    let success_orders_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transaction WHERE status = 'success'")
        .fetch_one(db)
        .await
        .unwrap_or(0);

    Ok(FinancialSummary {
        total_transactions,
        total_sales_volume,
        total_gross_profit,
        pending_orders_count,
        success_orders_count,
    })
}
