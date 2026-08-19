use crate::error::AppError;
use crate::models::{ApiOrderDataResponse, ApiOrderRequest, ApiOrderResponse, Flashsale, Service, User, UserApi};
use crate::providers::digiflazz::DigiFlazzClient;
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

    let order_id = format!("ORD{}{}", chrono::Utc::now().format("%Y%M%S"), rand::random::<u16>());

    sqlx::query("UPDATE users SET balance = balance - ? WHERE username = ?")
        .bind(total_price)
        .bind(&user.username)
        .execute(&mut *tx)
        .await?;

    let note = format!("Order API :: {}", order_id);
    sqlx::query("INSERT INTO mutation (username, type, amount, note, date_cr) VALUES (?, '-', ?, ?, NOW())")
        .bind(&user.username)
        .bind(total_price)
        .bind(&note)
        .execute(&mut *tx)
        .await?;

    sqlx::query(
        "INSERT INTO transaction (order_id, order_tid, user, code, service_name, game, target, price, profit, status, payment_status, provider, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'process', 'paid', ?, NOW(), NOW())"
    )
    .bind(&order_id)
    .bind(&order_id)
    .bind(&user.username)
    .bind(&service.code)
    .bind(&service.name)
    .bind(&service.game)
    .bind(&req.target)
    .bind(total_price)
    .bind(profit)
    .bind(&service.provider)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    if service.provider == "DIGI" {
        let digi_username = std::env::var("DIGIFLAZZ_USERNAME").unwrap_or_default();
        let digi_apikey = std::env::var("DIGIFLAZZ_APIKEY").unwrap_or_default();
        let digi_client = DigiFlazzClient::new(digi_username, digi_apikey);

        let topup_res = digi_client
            .topup(&state.http_client, &service.code, &req.target, &order_id)
            .await;

        match topup_res {
            Ok(res) if res.success => {
                sqlx::query("UPDATE transaction SET order_tid = ?, status = 'process' WHERE order_id = ?")
                    .bind(&res.trxid)
                    .bind(&order_id)
                    .execute(&state.db)
                    .await?;
            }
            Ok(res) => {
                let mut refund_tx = state.db.begin().await?;
                sqlx::query("UPDATE users SET balance = balance + ? WHERE username = ?")
                    .bind(total_price)
                    .bind(&user.username)
                    .execute(&mut *refund_tx)
                    .await?;
                sqlx::query("UPDATE transaction SET status = 'error' WHERE order_id = ?")
                    .bind(&order_id)
                    .execute(&mut *refund_tx)
                    .await?;
                refund_tx.commit().await?;

                return Err(AppError::ProviderError(res.message));
            }
            Err(e) => return Err(e),
        }
    }

    Ok(ApiOrderResponse {
        result: true,
        data: Some(ApiOrderDataResponse {
            order_id: order_id.clone(),
            data: req.target,
            code: service.code,
            service: service.name,
            status: "process".to_string(),
            note: "Berhasil dibayar".to_string(),
            price: total_price,
        }),
        message: "Pesanan berhasil diproses".to_string(),
    })
}
