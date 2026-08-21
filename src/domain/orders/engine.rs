use crate::error::AppError;
use crate::models::{ApiOrderDataResponse, ApiOrderRequest, ApiOrderResponse, Flashsale, Service, User, UserApi};
use crate::state::AppState;
use md5::{Digest, Md5};
use sqlx::MySql;

pub fn verify_signature(uid: &str, ukey: &str, sign: &str) -> bool {
    let raw = format!("{}{}", uid, ukey);
    let mut hasher = Md5::new();
    hasher.update(raw.as_bytes());
    let expected = format!("{:x}", hasher.finalize());
    expected.eq_ignore_ascii_case(sign)
}

pub fn check_ip_whitelist(client_ip: &str, whitelist: &str) -> bool {
    if whitelist.trim().is_empty() || whitelist == "*" {
        return true;
    }
    whitelist.split(',').any(|ip| ip.trim() == client_ip)
}

pub async fn process_api_order(
    state: &AppState,
    client_ip: &str,
    req: ApiOrderRequest,
) -> Result<ApiOrderResponse, AppError> {
    let api_user_opt: Option<UserApi> = sqlx::query_as::<MySql, UserApi>("SELECT * FROM users_api WHERE ukey = ?")
        .bind(&req.key)
        .fetch_optional(&state.db)
        .await?;

    let api_user = api_user_opt.ok_or(AppError::InvalidCredentials)?;

    if !verify_signature(&api_user.uid, &api_user.ukey, &req.sign) {
        return Err(AppError::InvalidApiSignature);
    }
    if !check_ip_whitelist(client_ip, &api_user.whitelist) {
        return Err(AppError::IpNotPermitted(client_ip.to_string()));
    }

    let mut tx = state.db.begin().await?;

    let user_opt: Option<User> = sqlx::query_as::<MySql, User>("SELECT * FROM users WHERE username = ? FOR UPDATE")
        .bind(&api_user.user)
        .fetch_optional(&mut *tx)
        .await?;

    let user = user_opt.ok_or(AppError::UserNotFound)?;

    let service_opt: Option<Service> = sqlx::query_as::<MySql, Service>(
        "SELECT * FROM service WHERE code = ? AND status = 'available'",
    )
    .bind(&req.service)
    .fetch_optional(&mut *tx)
    .await?;

    let service = service_opt.ok_or(AppError::ServiceNotFound)?;

    let profit = match user.level.as_str() {
        "Reseller" | "Admin" => service.reseller,
        _ => service.member,
    };
    let mut total_price = service.price + profit;

    let flashsale: Option<Flashsale> = sqlx::query_as::<MySql, Flashsale>(
        "SELECT * FROM flashsale WHERE code = ? AND status = '1'",
    )
    .bind(&service.code)
    .fetch_optional(&mut *tx)
    .await?;

    if let Some(fs) = flashsale {
        total_price -= fs.amount;
    }

    let required_hold = state.config.get_hold_balance(&user.level);
    if user.balance < total_price {
        return Err(AppError::InsufficientBalance);
    }
    if user.balance < (total_price + required_hold) {
        return Err(AppError::HoldBalanceViolation { required_hold });
    }

    // ============================================================
    // DUA FASE — otoritas penuh di Server Topup (web TIDAK dipercaya):
    // Web hanya membuat baris order UNPAID lalu broadcast [API_ORDER].
    // Potong saldo, validasi harga final, dan topup DILAKUKAN SERVER TOPUP.
    // Reseller memantau status lewat endpoint status (polling).
    // ============================================================
    let order_id = format!("ORD{}{}", chrono::Utc::now().format("%Y%m%d%H%M%S"), rand::random::<u16>());

    sqlx::query(
        "INSERT INTO transaction (order_id, order_tid, user, code, service_name, game, target, price, profit, status, payment_status, provider, created_at, updated_at) VALUES (?, '', ?, ?, ?, ?, ?, ?, ?, 'pending', 'unpaid', ?, NOW(), NOW())"
    )
    .bind(&order_id)
    .bind(&user.username)
    .bind(&service.code)
    .bind(&service.name)
    .bind(&service.game)
    .bind(&req.target)
    .bind(total_price)
    .bind(profit)
    .bind(&service.provider)
    .execute(&state.db)
    .await?;

    let clean_svc_name = service.name.replace(' ', "_").replace(';', "");
    let msg = format!(
        "[API_ORDER] {} {} {} {} {} {} {} -",
        order_id, user.username, service.code, req.target, service.provider, total_price, clean_svc_name
    );
    let _ = crate::domain::telegram::sender::send_report_to_fulfillment(&msg).await;

    Ok(ApiOrderResponse {
        result: true,
        data: Some(ApiOrderDataResponse {
            order_id: order_id.clone(),
            data: req.target,
            code: service.code,
            service: service.name,
            status: "pending".to_string(),
            note: "Menunggu verifikasi harga oleh Server Topup".to_string(),
            price: total_price,
        }),
        message: "Pesanan diterima — menunggu verifikasi Server Topup. Pantau status via endpoint status.".to_string(),
    })
}
