use axum::{
    extract::{Query, State},
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use rust_backend::{
    checkers::{duniagames::DuniaGamesChecker, pubg::PubgChecker, smileone_mlbb::SmileOneChecker},
    config::Config,
    domain::{
        account::service::{generate_api_keys, get_user_mutations, get_user_profile, get_user_transactions, update_api_settings, UpdateProfileRequest, ChangePasswordRequest, UpdateApiSettingsRequest, DepositRequest},
        admin::service::{admin_adjust_user_balance, admin_create_service, admin_get_financial_summary, admin_lock_user_account, AdminCreateServiceRequest, AdminLockAccountRequest, AdminUpdateUserBalanceRequest},
        admin::transaction::{update_payment_status, update_transaction_note, update_transaction_status, AdminUpdatePaymentStatusRequest, AdminUpdateTransactionNoteRequest, AdminUpdateTransactionStatusRequest},
        auth::{models::{LoginRequest, RegisterRequest}, service::{login_user, register_user}},
    },
    error::AppError,
    models::Service,
    payments::{
        duitku::{DuitkuGateway, DuitkuWebhookPayload},
        paydisini::{PaydisiniGateway, PaydisiniWebhookPayload},
        tokopay::{TokopayGateway, TokopayWebhookPayload},
        tripay::{TripayGateway, TripayWebhookPayload},
    },
    state::AppState,
};
use md5::{Digest, Md5};
use serde_json::json;
use serde::Deserialize;
use sqlx::{mysql::MySqlPoolOptions, MySql};
use std::net::SocketAddr;
use tower_http::services::ServeDir;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

fn md5_hash(s: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(s.as_bytes());
    format!("{:x}", hasher.finalize())
}

// ============================================================
// ANTI-BRUTEFORCE RATE LIMITER (sliding window, in-memory)
// ============================================================

static RATE_BUCKETS: std::sync::LazyLock<dashmap::DashMap<String, Vec<i64>>> =
    std::sync::LazyLock::new(|| dashmap::DashMap::new());

static SWEEP_TICK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Eviksi berkala: buang key yang isinya sudah basi (anti memory growth).
fn maybe_sweep_buckets(now: i64) {
    if SWEEP_TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 1024 != 0 {
        return;
    }
    RATE_BUCKETS.retain(|_, v| {
        v.retain(|ts| now - *ts < 3600);
        !v.is_empty()
    });
}

/// Sliding-window limiter: false = DITOLAK (melebihi `max` dalam `window_secs`).
fn rate_limit(key: &str, max: usize, window_secs: i64) -> bool {
    let now = chrono::Utc::now().timestamp();
    maybe_sweep_buckets(now);
    let mut entry = RATE_BUCKETS.entry(key.to_string()).or_default();
    entry.retain(|ts| now - *ts < window_secs);
    if entry.len() >= max {
        return false;
    }
    entry.push(now);
    true
}

/// Hitung kegagalan (mis. kode voucher salah). Return false bila sudah melewati batas.
fn record_failure(key: &str, max: usize, window_secs: i64) -> bool {
    let now = chrono::Utc::now().timestamp();
    maybe_sweep_buckets(now);
    let mut entry = RATE_BUCKETS.entry(key.to_string()).or_default();
    entry.retain(|ts| now - *ts < window_secs);
    if entry.len() >= max {
        return false;
    }
    entry.push(now);
    true
}

fn reset_failures(key: &str) {
    RATE_BUCKETS.remove(key);
}

// ============================================================
// REKONSILIASI PEMBAYARAN (at-least-once delivery)
// Notifikasi "order sudah dibayar" ke fulfillment dikirim ULANG
// tiap 60 detik sampai fulfillment membalas [STATUS_UPDATE] untuk
// order tersebut. Mencegah kasus: user bayar tapi pesan hilang.
// ============================================================
static PENDING_PAID: std::sync::LazyLock<dashmap::DashMap<String, (i64, u32)>> =
    std::sync::LazyLock::new(|| dashmap::DashMap::new());
const PAID_RESEND_MAX: u32 = 60; // 60x x 60s = 1 jam, lalu menyerah + alarm log

// Cache invoice gateway per order (idempoten): refresh/tab ganda mendapat
// TAGIHAN YANG SAMA, bukan tagihan baru di gateway (anti bayar 2x).
static INVOICE_CACHE: std::sync::LazyLock<dashmap::DashMap<String, (i64, serde_json::Value)>> =
    std::sync::LazyLock::new(|| dashmap::DashMap::new());

async fn notify_payment_to_fulfillment(order_id: &str) {
    let now = chrono::Utc::now().timestamp();
    match PENDING_PAID.get_mut(order_id) {
        Some(mut v) => {
            v.0 = now;
            v.1 += 1;
        }
        None => {
            PENDING_PAID.insert(order_id.to_string(), (now, 1));
        }
    }
    // Format sama dengan webhook lama (bare OID → dibungkus [REPORT] oleh sender)
    if let Err(e) = rust_backend::domain::telegram::sender::send_report_to_fulfillment(order_id).await {
        tracing::error!("[PAID RECONCILER] Gagal enqueue notif pembayaran {}: {}", order_id, e);
    }
}

pub(crate) async fn start_paid_reconciler(db: sqlx::MySqlPool) {
    tokio::spawn(async move {
        tracing::info!("[PAID RECONCILER] Worker aktif — kirim ulang notif bayar tiap 60 detik sampai di-ACK.");
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            let now = chrono::Utc::now().timestamp();
            for kv in PENDING_PAID.iter() {
                let oid = kv.key().clone();
                let (last_sent, attempts) = *kv.value();
                if now - last_sent < 60 {
                    continue;
                }
                drop(kv);
                // Cek kondisi terkini di DB web
                let row = sqlx::query("SELECT status, payment_status FROM transaction WHERE order_id = ? LIMIT 1")
                    .bind(&oid)
                    .fetch_optional(&db)
                    .await
                    .ok()
                    .flatten();
                if let Some(r) = row {
                    use sqlx::Row;
                    let status: String = r.try_get("status").unwrap_or_default();
                    let pay: String = r.try_get("payment_status").unwrap_or_default();
                    if status == "success" || status == "error" {
                        PENDING_PAID.remove(&oid); // sudah tuntas / gagal final
                        continue;
                    }
                    if pay == "unpaid" {
                        PENDING_PAID.remove(&oid); // invoice batal/belum jadi — bukan urusan topup
                        continue;
                    }
                } else {
                    PENDING_PAID.remove(&oid);
                    continue;
                }
                if attempts >= PAID_RESEND_MAX {
                    tracing::error!("[PAID RECONCILIATION ALERT] Order {} dibayar tapi TIDAK pernah diproses fulfillment setelah {} percobaan! PERLU TINDAKAN MANUAL.", oid, attempts);
                    PENDING_PAID.remove(&oid);
                    continue;
                }
                tracing::warn!("[PAID RECONCILER] Kirim ulang notif pembayaran {} (percobaan {}/{})", oid, attempts + 1, PAID_RESEND_MAX);
                notify_payment_to_fulfillment(&oid).await;
            }
        }
    });
}

fn client_ip_from_headers(headers: &HeaderMap) -> String {
    headers.get("x-forwarded-for")
        .and_then(|h| h.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| headers.get("x-real-ip").and_then(|h| h.to_str().ok()))
        .unwrap_or("unknown")
        .to_string()
}

#[derive(Deserialize)]
pub struct MLCheckQuery {
    pub user_id: String,
    pub zone_id: String,
}

#[derive(Deserialize)]
pub struct GameCheckQuery {
    pub user_id: String,
    pub zone_id: Option<String>,
    pub game_code: String,
}

#[derive(Deserialize)]
pub struct PubgCheckQuery {
    pub char_id: String,
}

#[derive(Deserialize)]
pub struct UserQuery {
    pub username: String,
}

#[derive(Deserialize)]
pub struct CreateInvoiceRequest {
    pub service_code: String,
    pub target: String,
    pub payment_gateway: String, // TRIPAY, DUITKU, TOKOPAY, PAYDISINI
    pub payment_channel: String,
    pub customer_name: String,
    pub customer_email: String,
    pub customer_phone: String,
    pub username: Option<String>,
    #[serde(default)]
    pub voucher: Option<String>,
    /// Mode "tempel": pakai order yang sudah dibuat oleh /api/v1/order/submit
    /// (tanpa duplikasi baris transaksi). Invoice gateway memakai order_id ini.
    #[serde(default)]
    pub existing_order_id: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct OrderVerifyOtpRequest {
    pub order_id: String,
    pub username: String,
    pub otp: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "store_gateway=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Config::from_env();
    tracing::info!("Starting [SERVICE 1: STORE_GATEWAY] on port {}...", config.server_port);

    let db_pool = MySqlPoolOptions::new()
        .max_connections(20)
        .connect_lazy(&config.database_url)?;

    let app_state = AppState::new(db_pool.clone(), config.clone());

    // START WEB STATUS CALLBACK LISTENER (Menangkap status sukses DigiFlazz & SN dari Server Topup)
    start_web_callback_listener(db_pool.clone()).await;
    start_paid_reconciler(db_pool.clone()).await;

    // INIT MTPROTO TELEGRAM CLIENT
    if let Err(e) = rust_backend::domain::telegram::sender::init_telegram_sender().await {
        tracing::warn!("Failed to initialize Telegram MTProto: {}", e);
    }

    let app = Router::new()
        .route("/health", get(health_check))
        .route("/invoices", get(serve_invoices_page))
        .route("/invoices/:oid", get(serve_faktur_page))
        .route("/product/:type/category/:slug", get(serve_order_page))
        .route("/blog/:slug", get(serve_blog_page))
        .route("/auth/login", get(serve_login_page))
        .route("/auth/register", get(serve_register_page))
        .route("/auth/forgot-password", get(serve_forgot_page))
        .route("/account", get(serve_account_page))
        .route("/account/", get(serve_account_page))
        .route("/admin", get(serve_admin_page))
        .route("/admin/", get(serve_admin_page))
        .route("/page/region", get(serve_region_page))
        .route("/page/pricelist", get(serve_pricelist_page))
        .route("/page/privacy-policy", get(serve_privacy_page))
        .route("/page/terms-and-condition", get(serve_terms_page))
        .route("/page/review", get(serve_review_page))
        .route("/leaderboard", get(serve_leaderboard_page))
        .route("/order/verify-otp", get(serve_verify_otp_page))
        .route("/deposit/invoices/:did", get(serve_deposit_faktur_page))
        .route("/api/service", get(handle_list_services))
        .route("/api/check-ml", get(handle_check_ml))
        .route("/api/check-game", get(handle_check_game))
        .route("/api/check-pubg", get(handle_check_pubg))
        .route("/api/auth/login", post(handle_login))
        .route("/api/auth/register", post(handle_register))
        .route("/api/auth/register-step2", post(handle_register_step2))
        .route("/api/auth/register-resend", post(handle_register_resend))
        .route("/api/config/turnstile", get(handle_config_turnstile))
        .route("/api/account/profile", get(handle_profile))
        .route("/api/account/dashboard", get(handle_account_dashboard))
        .route("/api/account/mutations", get(handle_mutations))
        .route("/api/account/transactions", get(handle_transactions))
        .route("/api/account/generate-keys", post(handle_generate_keys))
        .route("/api/account/update-profile", post(handle_update_profile))
        .route("/api/account/change-password", post(handle_change_password))
        .route("/api/account/api-settings", get(handle_api_settings))
        .route("/api/account/update-api", post(handle_update_api_settings))
        .route("/api/account/activity", get(handle_activity))
        .route("/api/account/deposit/methods", get(handle_deposit_methods))
        .route("/api/account/deposit/history", get(handle_deposit_history))
        .route("/api/account/deposit/create", post(handle_deposit_create))
        .route("/api/admin/service/create", post(handle_admin_create_service))
        .route("/api/admin/service/update", post(handle_admin_update_service))
        .route("/api/admin/service/delete", post(handle_admin_delete_service))
        .route("/api/admin/user/balance", post(handle_admin_adjust_balance))
        .route("/api/admin/user/lock", post(handle_admin_lock_account))
        .route("/api/admin/user/unlock", post(handle_admin_unlock_account))
        .route("/api/admin/user/create", post(handle_admin_create_user))
        .route("/api/admin/user/update", post(handle_admin_update_user))
        .route("/api/admin/users/locked", get(handle_admin_locked_users))
        .route("/api/admin/mutations", get(handle_admin_mutations))
        .route("/api/admin/deposits", get(handle_admin_deposits))
        .route("/api/admin/deposit/status", post(handle_admin_deposit_status))
        .route("/api/admin/vouchers", get(handle_admin_vouchers))
        .route("/api/admin/voucher/create", post(handle_admin_voucher_create))
        .route("/api/admin/voucher/delete", post(handle_admin_voucher_delete))
        .route("/api/admin/flashsale", get(handle_admin_flashsale))
        .route("/api/admin/flashsale/create", post(handle_admin_flashsale_create))
        .route("/api/admin/flashsale/toggle", post(handle_admin_flashsale_toggle))
        .route("/api/admin/flashsale/delete", post(handle_admin_flashsale_delete))
        .route("/api/admin/banners", get(handle_admin_banners))
        .route("/api/admin/banner/create", post(handle_admin_banner_create))
        .route("/api/admin/banner/delete", post(handle_admin_banner_delete))
        .route("/api/admin/blog", get(handle_admin_blog_list))
        .route("/api/admin/blog/save", post(handle_admin_blog_save))
        .route("/api/admin/blog/delete", post(handle_admin_blog_delete))
        .route("/api/admin/providers", get(handle_admin_providers))
        .route("/api/admin/provider/edit", post(handle_admin_provider_edit))
        .route("/api/admin/categories", get(handle_admin_categories))
        .route("/api/admin/category/save", post(handle_admin_category_save))
        .route("/api/admin/category/delete", post(handle_admin_category_delete))
        .route("/api/admin/tabs", get(handle_admin_tabs))
        .route("/api/admin/tab/save", post(handle_admin_tab_save))
        .route("/api/admin/tab/delete", post(handle_admin_tab_delete))
        .route("/api/admin/config/popup", get(handle_admin_conf_popup_get_only).post(handle_admin_conf_popup_post))
        .route("/api/admin/config/social", get(handle_admin_conf_social_get).post(handle_admin_conf_social_post))
        .route("/api/admin/config/colors", get(handle_admin_conf_colors_get).post(handle_admin_conf_colors_post))
        .route("/api/admin/config/pages", get(handle_admin_conf_pages_get).post(handle_admin_conf_pages_post))
        .route("/api/admin/reports/sales", get(handle_admin_report_sales))
        .route("/api/admin/reports/profit", get(handle_admin_report_profit))
        .route("/api/admin/reports/summary", get(handle_admin_report_summary))
        .route("/api/admin/error-logs", get(handle_admin_error_logs))
        .route("/api/admin/dashboard-stats", get(handle_admin_dashboard_stats))
        .route("/api/admin/manual-orders", get(handle_admin_manual_orders))
        .route("/api/admin/manual-order/resend", post(handle_admin_manual_resend))
        .route("/api/admin/financial-summary", get(handle_admin_financial_summary))
        .route("/api/admin/services", get(handle_admin_list_services))
        .route("/api/admin/transactions", get(handle_admin_list_transactions))
        .route("/api/admin/transaction/detail", get(handle_admin_transaction_detail))
        .route("/api/admin/transaction/update-status", post(handle_admin_update_transaction_status))
        .route("/api/admin/transaction/update-payment-status", post(handle_admin_update_payment_status))
        .route("/api/admin/transaction/update-note", post(handle_admin_update_transaction_note))
        .route("/api/admin/users", get(handle_admin_list_users))
        .route("/api/admin/config/website", get(handle_admin_config_website_get).post(handle_admin_config_website_post))
        .route("/api/admin/transaction/update-status", post(handle_admin_update_transaction_status))
        .route("/api/invoice/create", post(handle_create_invoice))
        .route("/api/invoice/verify-otp", post(handle_verify_otp))
        .route("/webhook/tripay", post(handle_tripay_webhook))
        .route("/webhook/duitku", post(handle_duitku_webhook))
        .route("/webhook/tokopay", post(handle_tokopay_webhook))
        .route("/webhook/paydisini", post(handle_paydisini_webhook))
        .route("/api/v1/page-data", get(handle_page_data))
        .route("/api/v1/search", get(handle_search))
        .route("/api/v1/product/:type/:slug", get(handle_product))
        .route("/api/v1/invoices", get(handle_invoices))
        .route("/api/v1/invoice/:oid", get(handle_invoice_detail))
        .route("/api/v1/blog/:slug", get(handle_blog))
        .route("/api/v1/leaderboard", get(handle_leaderboard))
        .route("/api/v1/pricelist/:code", get(handle_pricelist))
        .route("/api/v1/payment-methods", get(handle_payment_methods))
        .route("/api/v1/order/submit", post(handle_submit_order))
        .route("/api/v1/voucher/check", post(handle_voucher_check))
        .route("/api/v1/review/submit", post(handle_review_submit))
        .route("/api/v1/reviews", get(handle_reviews))
        .route("/api/v1/deposit/:did", get(handle_deposit_detail))
        .route("/api/v1/forgot-password", post(handle_forgot_password))
        .route("/api/v1/external/service", post(handle_external_service))
        .route("/api/v1/external/status", post(handle_external_status))
        .route("/api/v1/external/profile", post(handle_external_profile))
        .route("/api/v1/external/order", post(handle_external_order))
        .nest_service("/library", ServeDir::new("public/library"))
        .nest_service("/imagehome", ServeDir::new("public/imagehome"))
        .fallback_service(ServeDir::new("public"))
        .with_state(app_state);

    let addr: SocketAddr = format!("{}:{}", config.server_host, config.server_port).parse()?;
    tracing::info!("[SERVICE 1: STORE_GATEWAY] Server listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

// ==========================================
// PUBLIC WEBHOOK HANDLERS (TELEGRAM REPORTING)
// ==========================================

fn is_safe_id(id: &str) -> bool {
    id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

async fn handle_tripay_webhook(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    bytes: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, AppError> {
    let raw_payload = std::str::from_utf8(&bytes).unwrap_or_default();
    
    // Parse json
    let payload: TripayWebhookPayload = match serde_json::from_str(raw_payload) {
        Ok(p) => p,
        Err(_) => return Ok(Json(json!({ "success": false, "message": "Invalid JSON" }))),
    };

    if !is_safe_id(&payload.merchant_ref) {
        tracing::warn!("[WEBHOOK] Tripay Blocked: Suspicious characters in Order ID");
        return Ok(Json(json!({ "success": false, "message": "Blocked for security reasons" })));
    }

    // Get signature header
    let signature_header = headers.get("x-callback-signature").and_then(|h| h.to_str().ok()).unwrap_or_default();

    // Fetch Tripay credentials
    use sqlx::Row;
    let provider_query = "SELECT userid FROM provider WHERE code = 'TRIPAY' LIMIT 1";
    let secret_key = if let Ok(Some(row)) = sqlx::query(provider_query).fetch_optional(&state.db).await {
        row.try_get::<String, _>("userid").unwrap_or_default()
    } else {
        "".to_string()
    };

    // Generate signature
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;
    let is_valid = if let Ok(mut mac) = HmacSha256::new_from_slice(secret_key.as_bytes()) {
        mac.update(raw_payload.as_bytes());
        let expected = hex::encode(mac.finalize().into_bytes());
        expected.eq_ignore_ascii_case(signature_header)
    } else {
        false
    };

    if !is_valid {
        tracing::warn!("[WEBHOOK] Invalid Tripay signature! Spoof attempt rejected.");
        return Ok(Json(json!({ "success": false, "message": "Invalid signature" })));
    }

    tracing::info!("[WEBHOOK] Tripay Webhook for Order: {}, Status: {}", payload.merchant_ref, payload.status);

    if payload.status != "PAID" {
        return Ok(Json(json!({ "success": true, "message": "Ignored non-PAID status" })));
    }

    if payload.merchant_ref.starts_with("DP") {
        let mut tx = match state.db.begin().await {
            Ok(t) => t,
            Err(_) => return Ok(Json(json!({ "success": false, "message": "DB Error" }))),
        };

        let update_res = sqlx::query("UPDATE deposit SET status = 'paid' WHERE deposit_id = ? AND status = 'unpaid'")
            .bind(&payload.merchant_ref)
            .execute(&mut *tx)
            .await;

        if let Ok(res) = update_res {
            if res.rows_affected() > 0 {
                if let Ok(row) = sqlx::query("SELECT amount, username FROM deposit WHERE deposit_id = ?").bind(&payload.merchant_ref).fetch_one(&mut *tx).await {
                    let amount: f64 = row.try_get("amount").unwrap_or_default();
                    let username: String = row.try_get("username").unwrap_or_default();

                    let _ = sqlx::query("UPDATE users SET balance = balance + ? WHERE username = ?")
                        .bind(amount)
                        .bind(username)
                        .execute(&mut *tx)
                        .await;
                }
                let _ = tx.commit().await;
            } else {
                let _ = tx.rollback().await;
            }
        } else {
            let _ = tx.rollback().await;
        }
        let _ = sqlx::query("UPDATE transaction SET payment_status = 'paid', updated_at = NOW() WHERE order_id = ? AND payment_status = 'unpaid'")
            .bind(&payload.merchant_ref)
            .execute(&state.db)
            .await;
        notify_payment_to_fulfillment(&payload.merchant_ref).await;
    }

    Ok(Json(json!({ "success": true })))
}

async fn handle_duitku_webhook(
    State(state): State<AppState>,
    Json(payload): Json<DuitkuWebhookPayload>,
) -> Result<Json<serde_json::Value>, AppError> {
    
    if !is_safe_id(&payload.merchantOrderId) {
        tracing::warn!("[WEBHOOK] Duitku Blocked: Suspicious characters in Order ID");
        return Ok(Json(json!({ "success": false, "message": "Blocked for security reasons" })));
    }

    // Fetch Duitku credentials
    use sqlx::Row;
    let provider_query = "SELECT merchant, apikey FROM provider WHERE code = 'DUITKU' LIMIT 1";
    let (merchant, secret_key) = if let Ok(Some(row)) = sqlx::query(provider_query).fetch_optional(&state.db).await {
        let m: String = row.try_get("merchant").unwrap_or_default();
        let s: String = row.try_get("apikey").unwrap_or_default();
        (m, s)
    } else {
        ("".to_string(), "".to_string())
    };

    // Verify signature: md5(merchantCode + amount + merchantOrderId + apiKey)
    let raw = format!("{}{}{}{}", merchant, payload.amount, payload.merchantOrderId, secret_key);
    let expected = md5_hash(&raw);
    
    if payload.signature.to_lowercase() != expected.to_lowercase() {
        tracing::warn!("[WEBHOOK] Invalid Duitku signature! Spoof attempt rejected.");
        return Ok(Json(json!({ "success": false, "message": "Invalid signature" })));
    }

    tracing::info!("[WEBHOOK] Duitku Webhook for Order: {}, Result: {}", payload.merchantOrderId, payload.resultCode);

    if payload.resultCode != "00" {
        return Ok(Json(json!({ "success": true, "message": "Payment failed or canceled" })));
    }

    if payload.merchantOrderId.starts_with("DP") {
        use sqlx::Row;
        let mut tx = match state.db.begin().await {
            Ok(t) => t,
            Err(_) => return Ok(Json(json!({ "success": false, "message": "DB Error" }))),
        };

        let update_res = sqlx::query("UPDATE deposit SET status = 'paid' WHERE deposit_id = ? AND status = 'unpaid'")
            .bind(&payload.merchantOrderId)
            .execute(&mut *tx)
            .await;

        if let Ok(res) = update_res {
            if res.rows_affected() > 0 {
                if let Ok(row) = sqlx::query("SELECT amount, username FROM deposit WHERE deposit_id = ?").bind(&payload.merchantOrderId).fetch_one(&mut *tx).await {
                    let amount: f64 = row.try_get("amount").unwrap_or_default();
                    let username: String = row.try_get("username").unwrap_or_default();

                    let _ = sqlx::query("UPDATE users SET balance = balance + ? WHERE username = ?")
                        .bind(amount)
                        .bind(username)
                        .execute(&mut *tx)
                        .await;
                }
                let _ = tx.commit().await;
            } else {
                let _ = tx.rollback().await;
            }
        } else {
            let _ = tx.rollback().await;
        }
    } else {
        let _ = sqlx::query("UPDATE transaction SET payment_status = 'paid', updated_at = NOW() WHERE order_id = ? AND payment_status = 'unpaid'")
            .bind(&payload.merchantOrderId)
            .execute(&state.db)
            .await;
        notify_payment_to_fulfillment(&payload.merchantOrderId).await;
    }

    Ok(Json(json!({ "success": true })))
}

async fn handle_tokopay_webhook(
    State(state): State<AppState>,
    Json(payload): Json<TokopayWebhookPayload>,
) -> Result<Json<serde_json::Value>, AppError> {
    
    if !is_safe_id(&payload.reff_id) {
        tracing::warn!("[WEBHOOK] Tokopay Blocked: Suspicious characters in Order ID");
        return Ok(Json(json!({ "success": false, "message": "Blocked for security reasons" })));
    }

    // Fetch Tokopay credentials
    use sqlx::Row;
    let provider_query = "SELECT merchant, apikey FROM provider WHERE code = 'TOKOPAY' LIMIT 1";
    let (merchant, secret_key) = if let Ok(Some(row)) = sqlx::query(provider_query).fetch_optional(&state.db).await {
        let m: String = row.try_get("merchant").unwrap_or_default();
        let s: String = row.try_get("apikey").unwrap_or_default();
        (m, s)
    } else {
        ("".to_string(), "".to_string())
    };

    // Verify signature: md5(merchant:secret:reff_id)
    let raw = format!("{}:{}:{}", merchant, secret_key, payload.reff_id);
    let expected = md5_hash(&raw);
    
    if payload.signature != expected {
        tracing::warn!("[WEBHOOK] Invalid Tokopay signature! Spoof attempt rejected.");
        return Ok(Json(json!({ "success": false, "message": "Invalid signature" })));
    }

    tracing::info!("[WEBHOOK] Tokopay Webhook for Order: {}, Status: {}", payload.reff_id, payload.status);

    if payload.status.to_lowercase() != "success" {
        return Ok(Json(json!({ "status": true, "message": "Ignored" })));
    }

    if payload.reff_id.starts_with("DP") {
        use sqlx::Row;
        let mut tx = match state.db.begin().await {
            Ok(t) => t,
            Err(_) => return Ok(Json(json!({ "success": false, "message": "DB Error" }))),
        };

        let update_res = sqlx::query("UPDATE deposit SET status = 'paid' WHERE deposit_id = ? AND status = 'unpaid'")
            .bind(&payload.reff_id)
            .execute(&mut *tx)
            .await;

        if let Ok(res) = update_res {
            if res.rows_affected() > 0 {
                // Fetch to add balance
                if let Ok(row) = sqlx::query("SELECT amount, username FROM deposit WHERE deposit_id = ?").bind(&payload.reff_id).fetch_one(&mut *tx).await {
                    let amount: f64 = row.try_get("amount").unwrap_or_default();
                    let username: String = row.try_get("username").unwrap_or_default();

                    let _ = sqlx::query("UPDATE users SET balance = balance + ? WHERE username = ?")
                        .bind(amount)
                        .bind(username)
                        .execute(&mut *tx)
                        .await;
                }
                let _ = tx.commit().await;
            } else {
                let _ = tx.rollback().await;
            }
        } else {
            let _ = tx.rollback().await;
        }
    } else {
        // Regular Order
        let _ = sqlx::query("UPDATE transaction SET payment_status = 'paid', updated_at = NOW() WHERE order_id = ? AND payment_status = 'unpaid'")
            .bind(&payload.reff_id)
            .execute(&state.db)
            .await;
        notify_payment_to_fulfillment(&payload.reff_id).await;
    }

    Ok(Json(json!({ "success": true })))
}

async fn handle_paydisini_webhook(
    State(state): State<AppState>,
    Json(payload): Json<PaydisiniWebhookPayload>,
) -> Result<Json<serde_json::Value>, AppError> {
    
    if !is_safe_id(&payload.unique_code) {
        tracing::warn!("[WEBHOOK] Paydisini Blocked: Suspicious characters in Order ID");
        return Ok(Json(json!({ "success": false, "message": "Blocked for security reasons" })));
    }

    // Fetch Paydisini credentials
    use sqlx::Row;
    let provider_query = "SELECT apikey FROM provider WHERE code = 'PAYDISINI' LIMIT 1";
    let secret_key = if let Ok(Some(row)) = sqlx::query(provider_query).fetch_optional(&state.db).await {
        row.try_get::<String, _>("apikey").unwrap_or_default()
    } else {
        "".to_string()
    };

    // Verify signature: md5(apikey + unique_code + 'CallbackStatus')
    let raw = format!("{}{}{}", secret_key, payload.unique_code, "CallbackStatus");
    let expected = md5_hash(&raw);
    
    if payload.signature.to_lowercase() != expected.to_lowercase() {
        tracing::warn!("[WEBHOOK] Invalid Paydisini signature! Spoof attempt rejected.");
        return Ok(Json(json!({ "success": false, "message": "Invalid signature" })));
    }

    tracing::info!("[WEBHOOK] Paydisini Webhook for Order: {}, Status: {}", payload.unique_code, payload.status);

    if payload.status.to_lowercase() != "success" {
        return Ok(Json(json!({ "success": true, "message": "Ignored" })));
    }

    if payload.unique_code.starts_with("DP") {
        let mut tx = match state.db.begin().await {
            Ok(t) => t,
            Err(_) => return Ok(Json(json!({ "success": false, "message": "DB Error" }))),
        };

        let update_res = sqlx::query("UPDATE deposit SET status = 'paid' WHERE deposit_id = ? AND status = 'unpaid'")
            .bind(&payload.unique_code)
            .execute(&mut *tx)
            .await;

        if let Ok(res) = update_res {
            if res.rows_affected() > 0 {
                if let Ok(row) = sqlx::query("SELECT amount, username FROM deposit WHERE deposit_id = ?").bind(&payload.unique_code).fetch_one(&mut *tx).await {
                    let amount: f64 = row.try_get("amount").unwrap_or_default();
                    let username: String = row.try_get("username").unwrap_or_default();

                    let _ = sqlx::query("UPDATE users SET balance = balance + ? WHERE username = ?")
                        .bind(amount)
                        .bind(username)
                        .execute(&mut *tx)
                        .await;
                }
                let _ = tx.commit().await;
            } else {
                let _ = tx.rollback().await;
            }
        } else {
            let _ = tx.rollback().await;
        }
    } else {
        let _ = sqlx::query("UPDATE transaction SET payment_status = 'paid', updated_at = NOW() WHERE order_id = ? AND payment_status = 'unpaid'")
            .bind(&payload.unique_code)
            .execute(&state.db)
            .await;
        notify_payment_to_fulfillment(&payload.unique_code).await;
    }

    Ok(Json(json!({ "success": true })))
}

async fn health_check() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "service": "Aruteru Store Gateway (Service 1)" }))
}

// Serve a static HTML page from public/ (for pretty URLs mirroring PHP routes)
async fn serve_static_page(axum::extract::Path(file): axum::extract::Path<String>) -> Result<axum::response::Response, AppError> {
    let path = format!("public/{}.html", file);
    match tokio::fs::read(&path).await {
        Ok(bytes) => Ok(axum::response::Response::builder()
            .header("Content-Type", "text/html; charset=utf-8")
            .body(axum::body::Body::from(bytes))
            .unwrap()),
        Err(_) => Err(AppError::ServiceNotFound),
    }
}

async fn serve_invoices_page() -> Result<axum::response::Response, AppError> {
    serve_static_page(axum::extract::Path("invoice".to_string())).await
}
async fn serve_faktur_page() -> Result<axum::response::Response, AppError> {
    serve_static_page(axum::extract::Path("faktur".to_string())).await
}
async fn serve_order_page() -> Result<axum::response::Response, AppError> {
    serve_static_page(axum::extract::Path("order".to_string())).await
}
async fn serve_blog_page() -> Result<axum::response::Response, AppError> {
    serve_static_page(axum::extract::Path("blog".to_string())).await
}
async fn serve_login_page() -> Result<axum::response::Response, AppError> {
    serve_static_page(axum::extract::Path("login".to_string())).await
}
async fn serve_register_page() -> Result<axum::response::Response, AppError> {
    serve_static_page(axum::extract::Path("register".to_string())).await
}
async fn serve_forgot_page() -> Result<axum::response::Response, AppError> {
    serve_static_page(axum::extract::Path("forgot".to_string())).await
}
async fn serve_account_page() -> Result<axum::response::Response, AppError> {
    serve_static_page(axum::extract::Path("account".to_string())).await
}
async fn serve_admin_page() -> Result<axum::response::Response, AppError> {
    serve_static_page(axum::extract::Path("admin".to_string())).await
}
async fn serve_region_page() -> Result<axum::response::Response, AppError> {
    serve_static_page(axum::extract::Path("region".to_string())).await
}
async fn serve_pricelist_page() -> Result<axum::response::Response, AppError> {
    serve_static_page(axum::extract::Path("pricelist".to_string())).await
}
async fn serve_privacy_page() -> Result<axum::response::Response, AppError> {
    serve_static_page(axum::extract::Path("privacy".to_string())).await
}
async fn serve_terms_page() -> Result<axum::response::Response, AppError> {
    serve_static_page(axum::extract::Path("terms".to_string())).await
}
async fn serve_review_page() -> Result<axum::response::Response, AppError> {
    serve_static_page(axum::extract::Path("review".to_string())).await
}
async fn serve_leaderboard_page() -> Result<axum::response::Response, AppError> {
    serve_static_page(axum::extract::Path("leaderboard".to_string())).await
}
async fn serve_verify_otp_page() -> Result<axum::response::Response, AppError> {
    serve_static_page(axum::extract::Path("verify-otp".to_string())).await
}
async fn serve_deposit_faktur_page() -> Result<axum::response::Response, AppError> {
    serve_static_page(axum::extract::Path("deposit-faktur".to_string())).await
}

async fn handle_list_services(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    let services = sqlx::query_as::<MySql, Service>(
        "SELECT * FROM service WHERE status = 'available' ORDER BY id ASC",
    )
    .fetch_all(&state.db)
    .await?;

    Ok(Json(json!({
        "result": true,
        "data": services,
        "message": "Data layanan berhasil didapatkan"
    })))
}

async fn handle_check_ml(
    State(state): State<AppState>,
    Query(query): Query<MLCheckQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let res = SmileOneChecker::check_role(&state.http_client, &query.user_id, &query.zone_id).await?;
    Ok(Json(json!(res)))
}

async fn handle_check_game(
    State(state): State<AppState>,
    Query(query): Query<GameCheckQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let zone = query.zone_id.unwrap_or_default();
    let res = DuniaGamesChecker::check(&state.http_client, &query.user_id, &zone, &query.game_code).await?;
    Ok(Json(json!(res)))
}

async fn handle_check_pubg(
    State(state): State<AppState>,
    Query(query): Query<PubgCheckQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let res = PubgChecker::check(&state.http_client, &query.char_id).await?;
    Ok(Json(json!(res)))
}

use rust_backend::domain::auth::models::{VerifyOtpRequest, ResendOtpRequest, PendingRegistration};
use rust_backend::domain::auth::service::{verify_turnstile, send_mpwa_otp};
use std::time::{SystemTime, UNIX_EPOCH};

async fn get_turnstile_secret(db: &sqlx::MySqlPool) -> Result<String, AppError> {
    let row: Option<(String,)> = sqlx::query_as("SELECT content FROM config WHERE name = 'captcha' AND parameter = '3'")
        .fetch_optional(db)
        .await?;
    Ok(row.map(|r| r.0).unwrap_or_default())
}

async fn handle_config_turnstile(State(state): State<AppState>) -> Result<Json<serde_json::Value>, AppError> {
    let row: Option<(String,)> = sqlx::query_as("SELECT content FROM config WHERE name = 'captcha' AND parameter = '2'")
        .fetch_optional(&state.db)
        .await?;
    let sitekey = row.map(|r| r.0).unwrap_or_default();
    Ok(Json(json!({ "result": true, "sitekey": sitekey })))
}

async fn handle_login(
    State(state): State<AppState>,
    Json(payload): Json<LoginRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if let Some(token) = &payload.turnstile_token {
        let secret = get_turnstile_secret(&state.db).await?;
        if !secret.is_empty() {
            let is_valid = verify_turnstile(&secret, token, "127.0.0.1").await?;
            if !is_valid {
                return Err(AppError::InternalError("Captcha tidak valid.".to_string()));
            }
        }
    }

    let (user, token) = login_user(&state.db, &payload.username, &payload.password, "Axum-Rust", "127.0.0.1").await?;
    Ok(Json(json!({
        "result": true,
        "token": token,
        "username": user.username,
        "level": user.level,
        "message": "Login berhasil"
    })))
}

async fn handle_register(
    State(state): State<AppState>,
    Json(payload): Json<RegisterRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if let Some(token) = &payload.turnstile_token {
        let secret = get_turnstile_secret(&state.db).await?;
        if !secret.is_empty() {
            let is_valid = verify_turnstile(&secret, token, "127.0.0.1").await?;
            if !is_valid {
                return Err(AppError::InternalError("Captcha tidak valid.".to_string()));
            }
        }
    }

    // Attempt to normalize and check exists without doing the actual DB insert first
    use rust_backend::domain::auth::service::normalize_phone;
    let normalized_phone = normalize_phone(&payload.phone);
    let existing: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE username = ? OR email = ? OR phone = ?")
        .bind(&payload.username)
        .bind(&payload.email)
        .bind(&normalized_phone)
        .fetch_optional(&state.db)
        .await?;

    if existing.is_some() {
        return Err(AppError::InternalError("Username, email, atau no HP sudah terdaftar.".to_string()));
    }

    let otp = format!("{:06}", rand::random::<u32>() % 1000000);
    let session_id = format!("REG_{}{}", chrono::Utc::now().timestamp(), rand::random::<u32>());
    
    // Send OTP via MPWA (from connect.php logic / env config)
    let api_key = std::env::var("MPWA_API_KEY").unwrap_or_else(|_| "Gn4YPpt4r5srg8M5Vco9TUBQhZPmF5".to_string());
    let sender = std::env::var("MPWA_SENDER_PHONE").unwrap_or_else(|_| "089667912348".to_string());
    let _admin_phone = std::env::var("MPWA_ADMIN_PHONE").unwrap_or_else(|_| "082132175370".to_string());
    
    // Format message like PHP `strtr(base64_decode(conf('notification',1)),['{{ OTP }}' => otp])`
    // We'll fetch it from DB
    let notif_row: Option<(String,)> = sqlx::query_as("SELECT content FROM config WHERE name = 'notification' AND parameter = '1'")
        .fetch_optional(&state.db)
        .await?;
    
    let mut message = String::from("Kode OTP Anda: {{ OTP }}. Jangan berikan kepada siapapun.");
    if let Some((b64_content,)) = notif_row {
        use base64::{Engine as _, engine::general_purpose};
        if let Ok(decoded) = general_purpose::STANDARD.decode(b64_content) {
            if let Ok(s) = String::from_utf8(decoded) {
                message = s;
            }
        }
    }
    let message = message.replace("{{ OTP }}", &otp);
    
    let mpwa_sent = send_mpwa_otp(&api_key, &sender, &normalized_phone, &message).await;
    if mpwa_sent.is_err() || !mpwa_sent.unwrap() {
        return Err(AppError::InternalError("Gagal mengirim OTP ke WhatsApp. Coba lagi.".to_string()));
    }

    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    // Clean up stale registration sessions (> 15 min) to prevent memory leak (BUG-15)
    state.pending_registers.retain(|_, v| now - v.created_at < 900);

    state.pending_registers.insert(session_id.clone(), PendingRegistration {
        name: payload.name.clone(),
        username: payload.username.clone(),
        email: payload.email.clone(),
        phone: payload.phone.clone(),
        password: payload.password.clone(),
        otp,
        created_at: now,
    });

    Ok(Json(json!({
        "result": true,
        "session_id": session_id,
        "message": "OTP berhasil dikirim"
    })))
}

async fn handle_register_step2(
    State(state): State<AppState>,
    Json(payload): Json<VerifyOtpRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut to_remove = false;
    let mut user_details = None;
    
    if let Some(entry) = state.pending_registers.get(&payload.session_id) {
        if entry.otp != payload.otp {
            return Err(AppError::InternalError("Kode OTP salah.".to_string()));
        }
        to_remove = true;
        user_details = Some(entry.clone());
    } else {
        return Err(AppError::InternalError("Sesi pendaftaran tidak valid atau sudah kadaluarsa.".to_string()));
    }

    if to_remove {
        state.pending_registers.remove(&payload.session_id);
    }

    if let Some(pending) = user_details {
        let name = pending.name.unwrap_or_default();
        let user = register_user(&state.db, &name, &pending.username, &pending.email, &pending.phone, &pending.password).await?;
        
        // Notify Admin
        let admin_msg = format!("🆕 New User Registration Alert 🆕\n\nUsername: {}\nEmail: {}\nPhone: {}\n\n✅ Account created successfully", 
                                pending.username, pending.email, pending.phone);
        let api_key = std::env::var("MPWA_API_KEY").unwrap_or_else(|_| "Gn4YPpt4r5srg8M5Vco9TUBQhZPmF5".to_string());
        let sender = std::env::var("MPWA_SENDER_PHONE").unwrap_or_else(|_| "089667912348".to_string());
        let admin_phone = std::env::var("MPWA_ADMIN_PHONE").unwrap_or_else(|_| "082132175370".to_string());
        let _ = send_mpwa_otp(&api_key, &sender, &admin_phone, &admin_msg).await;

        Ok(Json(json!({
            "result": true,
            "username": user.username,
            "message": "Registrasi berhasil"
        })))
    } else {
        Err(AppError::InternalError("Sesi gagal.".to_string()))
    }
}

async fn handle_register_resend(
    State(state): State<AppState>,
    Json(payload): Json<ResendOtpRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let mut updated_entry = None;

    if let Some(mut entry) = state.pending_registers.get_mut(&payload.session_id) {
        if now - entry.created_at < 120 {
            return Err(AppError::InternalError(format!("Mohon tunggu {} detik untuk kirim ulang OTP.", 120 - (now - entry.created_at))));
        }
        
        let new_otp = format!("{:06}", rand::random::<u32>() % 1000000);
        entry.otp = new_otp.clone();
        entry.created_at = now;
        
        updated_entry = Some((entry.phone.clone(), new_otp));
    } else {
        return Err(AppError::InternalError("Sesi tidak valid.".to_string()));
    }
    
    if let Some((phone, otp)) = updated_entry {
        let normalized_phone = rust_backend::domain::auth::service::normalize_phone(&phone);
        let notif_row: Option<(String,)> = sqlx::query_as("SELECT content FROM config WHERE name = 'notification' AND parameter = '1'").fetch_optional(&state.db).await?;
        let mut message = String::from("Kode OTP Anda: {{ OTP }}");
        if let Some((b64_content,)) = notif_row {
            use base64::{Engine as _, engine::general_purpose};
            if let Ok(decoded) = general_purpose::STANDARD.decode(b64_content) {
                if let Ok(s) = String::from_utf8(decoded) {
                    message = s;
                }
            }
        }
        let message = message.replace("{{ OTP }}", &otp);
        let api_key = std::env::var("MPWA_API_KEY").unwrap_or_else(|_| "Gn4YPpt4r5srg8M5Vco9TUBQhZPmF5".to_string());
        let sender = std::env::var("MPWA_SENDER_PHONE").unwrap_or_else(|_| "089667912348".to_string());
        let mpwa_sent = send_mpwa_otp(&api_key, &sender, &normalized_phone, &message).await;
        if mpwa_sent.is_err() || !mpwa_sent.unwrap() {
            return Err(AppError::InternalError("Gagal mengirim OTP ke WhatsApp. Coba lagi.".to_string()));
        }
        
        return Ok(Json(json!({
            "result": true,
            "message": "OTP baru telah dikirim."
        })));
    }
    
    Err(AppError::InternalError("Error".to_string()))
}

async fn handle_account_dashboard(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<UserQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_session(&state, &headers, Some(&query.username)).await?;
    let user = get_user_profile(&state.db, &query.username).await?;
    let balance = user.balance;
    let level = user.level;

    let month = chrono::Local::now().format("%m").to_string();
    let year = chrono::Local::now().format("%Y").to_string();
    let last_month = chrono::Local::now().checked_sub_signed(chrono::Duration::days(30)).unwrap_or_else(chrono::Local::now).format("%m").to_string();
    let last_year = chrono::Local::now().checked_sub_signed(chrono::Duration::days(30)).unwrap_or_else(chrono::Local::now).format("%Y").to_string();

    async fn count_one(db: &sqlx::MySqlPool, q: &str, user: &str) -> i64 {
        sqlx::query_scalar::<MySql, i64>(q).bind(user).fetch_one(db).await.unwrap_or(0)
    }
    async fn sum_one(db: &sqlx::MySqlPool, q: &str, month: &str, year: &str, user: &str) -> f64 {
        sqlx::query_scalar::<MySql, f64>(q)
            .bind(month).bind(year).bind(user)
            .fetch_one(db).await.unwrap_or(0.0)
    }

    let total_order = count_one(&state.db, "SELECT COUNT(*) FROM transaction WHERE status = 'success' AND user = ?", &query.username).await;
    let total_deposit = count_one(&state.db, "SELECT COUNT(*) FROM deposit WHERE status = 'paid' AND username = ?", &query.username).await;
    let order_today = count_one(&state.db, "SELECT COUNT(*) FROM transaction WHERE status = 'success' AND user = ? AND DATE(date_cr) = CURDATE()", &query.username).await;
    let order_last_day = count_one(&state.db, "SELECT COUNT(*) FROM transaction WHERE status = 'success' AND user = ? AND DATE(date_cr) = DATE_SUB(CURDATE(), INTERVAL 1 DAY)", &query.username).await;

    let revenue_order_this_month = sum_one(&state.db, "SELECT COALESCE(SUM(price),0) FROM transaction WHERE status = 'success' AND MONTH(date_cr) = ? AND YEAR(date_cr) = ? AND user = ?", &month, &year, &query.username).await;
    let revenue_order_last_month = sum_one(&state.db, "SELECT COALESCE(SUM(price),0) FROM transaction WHERE status = 'success' AND MONTH(date_cr) = ? AND YEAR(date_cr) = ? AND user = ?", &last_month, &last_year, &query.username).await;
    let revenue_depo_this_month = sum_one(&state.db, "SELECT COALESCE(SUM(amount),0) FROM deposit WHERE status = 'paid' AND MONTH(date_cr) = ? AND YEAR(date_cr) = ? AND username = ?", &month, &year, &query.username).await;
    let revenue_depo_last_month = sum_one(&state.db, "SELECT COALESCE(SUM(amount),0) FROM deposit WHERE status = 'paid' AND MONTH(date_cr) = ? AND YEAR(date_cr) = ? AND username = ?", &last_month, &last_year, &query.username).await;

    Ok(Json(json!({
        "result": true,
        "data": {
            "username": query.username,
            "balance": balance,
            "level": level,
            "total_order": total_order,
            "total_deposit": total_deposit,
            "order_today": order_today,
            "order_last_day": order_last_day,
            "revenue_order_this_month": revenue_order_this_month,
            "revenue_order_last_month": revenue_order_last_month,
            "revenue_depo_this_month": revenue_depo_this_month,
            "revenue_depo_last_month": revenue_depo_last_month
        }
    })))
}

async fn handle_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<UserQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_session(&state, &headers, Some(&query.username)).await?;
    let profile = get_user_profile(&state.db, &query.username).await?;
    Ok(Json(json!({ "result": true, "data": profile })))
}

async fn handle_mutations(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<UserQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_session(&state, &headers, Some(&query.username)).await?;
    let mutations = get_user_mutations(&state.db, &query.username).await?;
    Ok(Json(json!({ "result": true, "data": mutations })))
}

async fn handle_transactions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<UserQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_session(&state, &headers, Some(&query.username)).await?;
    let trxs = get_user_transactions(&state.db, &query.username).await?;
    Ok(Json(json!({ "result": true, "data": trxs })))
}

async fn handle_generate_keys(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(query): Json<UserQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_session(&state, &headers, Some(&query.username)).await?;
    let api_keys = generate_api_keys(&state.db, &query.username).await?;
    Ok(Json(json!({ "result": true, "data": api_keys })))
}

async fn handle_update_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<UpdateProfileRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_session(&state, &headers, Some(&payload.username)).await?;
    sqlx::query("UPDATE users SET name = ?, email = ?, phone = ? WHERE username = ?")
        .bind(&payload.name)
        .bind(&payload.email)
        .bind(&payload.phone)
        .bind(&payload.username)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "result": true, "message": "Profil berhasil diperbarui" })))
}

async fn handle_change_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<ChangePasswordRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_session(&state, &headers, Some(&payload.username)).await?;
    use bcrypt::{hash, verify, DEFAULT_COST};
    use sqlx::Row;
    
    let user_row = sqlx::query("SELECT password FROM users WHERE username = ?")
        .bind(&payload.username)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::UserNotFound)?;
        
    let hashed_pw: String = user_row.try_get("password").unwrap_or_default();
    
    if !verify(&payload.old, &hashed_pw).unwrap_or(false) {
        return Ok(Json(json!({ "result": false, "message": "Sandi lama tidak sesuai" })));
    }
    
    let new_hashed = hash(&payload.new, DEFAULT_COST).map_err(|e| AppError::InternalError(e.to_string()))?;
    
    sqlx::query("UPDATE users SET password = ? WHERE username = ?")
        .bind(&new_hashed)
        .bind(&payload.username)
        .execute(&state.db)
        .await?;
        
    Ok(Json(json!({ "result": true, "message": "Sandi berhasil diperbarui" })))
}

async fn handle_api_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<UserQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_session(&state, &headers, Some(&query.username)).await?;
    let api_keys: Option<rust_backend::models::UserApi> = sqlx::query_as::<MySql, rust_backend::models::UserApi>("SELECT * FROM users_api WHERE user = ?")
        .bind(&query.username)
        .fetch_optional(&state.db)
        .await?;
    
    if let Some(keys) = api_keys {
        Ok(Json(json!({ "result": true, "data": keys })))
    } else {
        Ok(Json(json!({ "result": false, "message": "API keys tidak ditemukan" })))
    }
}

async fn handle_update_api_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<UpdateApiSettingsRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_session(&state, &headers, Some(&payload.username)).await?;
    update_api_settings(&state.db, &payload.username.clone(), payload).await?;
    Ok(Json(json!({ "result": true, "message": "Pengaturan API berhasil disimpan" })))
}

async fn handle_activity(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<UserQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_session(&state, &headers, Some(&query.username)).await?;
    use sqlx::Row;
    
    // 1. Try querying users_cookie for active sessions
    let cookie_rows = sqlx::query("SELECT id, active as date, ua as action, ip FROM users_cookie WHERE username = ? ORDER BY id DESC LIMIT 20")
        .bind(&query.username)
        .fetch_all(&state.db)
        .await;

    if let Ok(rows) = cookie_rows {
        if !rows.is_empty() {
            let data: Vec<serde_json::Value> = rows.into_iter().map(|r| {
                json!({
                    "id": r.try_get::<i32, _>("id").map(|v| v as u64).or_else(|_| r.try_get::<u64, _>("id")).unwrap_or_default(),
                    "date": r.try_get::<String, _>("date").unwrap_or_default(),
                    "action": r.try_get::<String, _>("action").unwrap_or_else(|_| "Login Sesi Web".to_string()),
                    "ip": r.try_get::<String, _>("ip").unwrap_or_else(|_| "127.0.0.1".to_string())
                })
            }).collect();
            return Ok(Json(json!({ "result": true, "data": data })));
        }
    }

    // 2. Fallback to mutation activity
    let mut_rows = sqlx::query("SELECT id, date_cr as date, note as action FROM mutation WHERE username = ? ORDER BY id DESC LIMIT 20")
        .bind(&query.username)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let data: Vec<serde_json::Value> = mut_rows.into_iter().map(|r| {
        json!({
            "id": r.try_get::<i32, _>("id").map(|v| v as u64).or_else(|_| r.try_get::<u64, _>("id")).unwrap_or_default(),
            "date": r.try_get::<String, _>("date").unwrap_or_default(),
            "action": r.try_get::<String, _>("action").unwrap_or_else(|_| "Aktivitas Akun".to_string()),
            "ip": "127.0.0.1"
        })
    }).collect();

    Ok(Json(json!({ "result": true, "data": data })))
}

async fn handle_deposit_methods(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_session(&state, &headers, None).await?;
    use sqlx::Row;

    // Real methods from `metode` table (parity with PHP account/deposit/ajax.php).
    // Deposit excludes the 'app' (saldo) type.
    let rows = sqlx::query("SELECT * FROM metode WHERE status = 'on' AND type != 'app' ORDER BY type ASC, id ASC")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let list: Vec<serde_json::Value> = rows.iter().map(|r| {
        let code = r.try_get::<String, _>("code").unwrap_or_default();
        let name = r.try_get::<String, _>("name").unwrap_or_default();
        let mtype = r.try_get::<String, _>("type").unwrap_or_default();
        let min_val = r.try_get::<f64, _>("min").unwrap_or(10000.0);
        let max_val = r.try_get::<f64, _>("max").unwrap_or(5000000.0);
        let fee_using = r.try_get::<String, _>("fee_using").unwrap_or_else(|_| "merchant".to_string());
        let fee_type = r.try_get::<String, _>("fee_type").unwrap_or_else(|_| "+".to_string());
        let percent = r.try_get::<f64, _>("percent").unwrap_or(0.0);
        let flat = r.try_get::<f64, _>("flat").unwrap_or(0.0);
        let image = r.try_get::<String, _>("image").unwrap_or_default();
        let provider = r.try_get::<String, _>("provider").unwrap_or_else(|_| "PAYDISINI".to_string());
        json!({
            "code": code,
            "name": name,
            "type": mtype,
            "min": min_val,
            "max": max_val,
            "fee_using": fee_using,
            "fee_type": fee_type,
            "percent": percent,
            "flat": flat,
            "image": image,
            "provider": provider,
        })
    }).collect();

    Ok(Json(json!({ "result": true, "data": list })))
}

async fn handle_deposit_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<UserQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_session(&state, &headers, Some(&query.username)).await?;
    // Query deposit table
    use sqlx::Row;
    let rows = sqlx::query("SELECT id, deposit_id, method, amount, fee, balance, status, date FROM deposit WHERE username = ? ORDER BY id DESC LIMIT 50")
        .bind(&query.username)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let data: Vec<serde_json::Value> = rows.into_iter().map(|r| {
        let did: String = r.try_get::<String, _>("deposit_id").unwrap_or_else(|_| r.try_get::<i64, _>("id").unwrap_or_default().to_string());
        json!({
            "id": r.try_get::<i32, _>("id").unwrap_or_default().to_string(),
            "deposit_id": did,
            "method": r.try_get::<String, _>("method").unwrap_or_default(),
            "amount": r.try_get::<f64, _>("amount").unwrap_or_default(),
            "fee": r.try_get::<f64, _>("fee").unwrap_or(0.0),
            "uniq": r.try_get::<f64, _>("balance").unwrap_or(0.0),
            "status": r.try_get::<String, _>("status").unwrap_or_default(),
            "date_cr": r.try_get::<String, _>("date").unwrap_or_default()
        })
    }).collect();

    Ok(Json(json!({ "result": true, "data": data })))
}

async fn handle_deposit_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<DepositRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_session(&state, &headers, Some(&payload.username)).await?;
    let deposit_id = rust_backend::domain::account::service::create_deposit(&state.db, payload).await?;
    Ok(Json(json!({ "result": true, "message": "Berhasil membuat deposit", "deposit_id": deposit_id, "invoice_id": deposit_id })))
}

async fn handle_admin_create_service(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<AdminCreateServiceRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    admin_create_service(&state.db, payload).await?;
    Ok(Json(json!({ "result": true, "message": "Layanan berhasil ditambahkan" })))
}

async fn handle_admin_adjust_balance(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<AdminUpdateUserBalanceRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    let new_balance = admin_adjust_user_balance(&state.db, payload).await?;
    Ok(Json(json!({ "result": true, "new_balance": new_balance, "message": "Saldo berhasil disesuaikan" })))
}

async fn handle_admin_lock_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<AdminLockAccountRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    admin_lock_user_account(&state.db, payload).await?;
    Ok(Json(json!({ "result": true, "message": "Akun berhasil dikunci" })))
}

async fn handle_admin_financial_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    let summary = admin_get_financial_summary(&state.db).await?;
    Ok(Json(json!({ "result": true, "data": summary })))
}

async fn handle_admin_list_services(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    use sqlx::Row;
    let rows = sqlx::query("SELECT * FROM service ORDER BY id DESC LIMIT 200")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let data: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|r| {
            json!({
                "id": r.try_get::<i32,_>("id").unwrap_or_default(),
                "code": r.try_get::<String,_>("code").unwrap_or_default(),
                "name": r.try_get::<String,_>("name").unwrap_or_default(),
                "game": r.try_get::<String,_>("game").unwrap_or_default(),
                "price": r.try_get::<f64,_>("price").unwrap_or_default(),
                "member": r.try_get::<f64,_>("member").unwrap_or_default(),
                "reseller": r.try_get::<f64,_>("reseller").unwrap_or_default(),
                "provider": r.try_get::<String,_>("provider").unwrap_or_default(),
                "status": r.try_get::<String,_>("status").unwrap_or_default(),
            })
        })
        .collect();

    Ok(Json(json!({ "result": true, "data": data })))
}

async fn handle_admin_list_transactions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    use sqlx::Row;
    let rows = sqlx::query("SELECT * FROM transaction ORDER BY id DESC LIMIT 200")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let data: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|r| {
            let created: Option<chrono::NaiveDateTime> = r.try_get("created_at").unwrap_or(None);
            json!({
                "id": r.try_get::<i32,_>("id").unwrap_or_default(),
                "order_id": r.try_get::<String,_>("order_id").unwrap_or_default(),
                "user": r.try_get::<String,_>("user").unwrap_or_default(),
                "service_name": r.try_get::<String,_>("service_name").unwrap_or_default(),
                "target": r.try_get::<String,_>("target").unwrap_or_default(),
                "price": r.try_get::<f64,_>("price").unwrap_or_default(),
                "status": r.try_get::<String,_>("status").unwrap_or_default(),
                "status_payment": r.try_get::<String,_>("payment_status").unwrap_or_default(),
                "created_at": created.map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_default(),
            })
        })
        .collect();

    Ok(Json(json!({ "result": true, "data": data })))
}

async fn handle_admin_list_users(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    use sqlx::Row;
    let rows = sqlx::query("SELECT id, username, name, email, phone, balance, level, date_cr FROM users ORDER BY id DESC LIMIT 200")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let data: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|r| {
            json!({
                "id": r.try_get::<i32,_>("id").unwrap_or_default(),
                "username": r.try_get::<String,_>("username").unwrap_or_default(),
                "name": r.try_get::<String,_>("name").unwrap_or_default(),
                "email": r.try_get::<String,_>("email").unwrap_or_default(),
                "phone": r.try_get::<String,_>("phone").unwrap_or_default(),
                "balance": r.try_get::<f64,_>("balance").unwrap_or_default(),
                "level": r.try_get::<String,_>("level").unwrap_or_default(),
                "date_cr": r.try_get::<String,_>("date_cr").unwrap_or_default(),
            })
        })
        .collect();

    Ok(Json(json!({ "result": true, "data": data })))
}

// ============================================================
// SERVER-SIDE SESSION GUARD (parity with PHP library/session)
// ============================================================

fn bearer_token(headers: &HeaderMap) -> String {
    headers.get("Authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
        .to_string()
}

/// Validates the SESS_ token against users_cookie.
/// Returns (username, level). When `expected_user` is Some, the session user
/// must match it unless the session belongs to an Admin.
async fn require_session(state: &AppState, headers: &HeaderMap, expected_user: Option<&str>) -> Result<(String, String), AppError> {
    use sqlx::Row;
    let token = bearer_token(headers);
    if token.is_empty() {
        return Err(AppError::InvalidCredentials);
    }
    let row = sqlx::query(
        "SELECT c.username, COALESCE(u.level, 'Member') AS level FROM users_cookie c LEFT JOIN users u ON u.username = c.username WHERE c.cookie = ? AND (c.expired IS NULL OR c.expired > NOW()) LIMIT 1",
    )
    .bind(&token)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::InvalidCredentials)?;

    let username: String = row.try_get("username").unwrap_or_default();
    let level: String = row.try_get("level").unwrap_or_else(|_| "Member".to_string());

    if let Some(expected) = expected_user {
        if username != expected && level != "Admin" {
            return Err(AppError::AccountLocked("Akses ditolak.".to_string()));
        }
    }
    Ok((username, level))
}

async fn require_admin(state: &AppState, headers: &HeaderMap) -> Result<String, AppError> {
    let (user, level) = require_session(state, headers, None).await?;
    if level != "Admin" {
        return Err(AppError::AccountLocked("Akses khusus admin.".to_string()));
    }
    Ok(user)
}

// ============================================================
// EXTERNAL API (mirror api/*.php) — key + sign auth
// ============================================================

async fn verify_api_key(state: &AppState, key: &str, sign: &str) -> Result<sqlx::mysql::MySqlRow, AppError> {
    use sqlx::Row;
    let row = sqlx::query("SELECT * FROM users_api WHERE ukey = ? LIMIT 1")
        .bind(key)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::ServiceNotFound)?;

    let uid: String = row.try_get("uid").unwrap_or_default();
    let ukey: String = row.try_get("ukey").unwrap_or_default();

    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(format!("{}{}", uid, ukey));
    let expected = format!("{:x}", hasher.finalize());

    if sign != expected {
        return Err(AppError::ServerNotFound);
    }
    Ok(row)
}

#[derive(Deserialize)]
pub struct ExternalServiceRequest {
    pub key: String,
    pub sign: String,
    #[serde(default)]
    pub filter_type: String,
    #[serde(default)]
    pub filter_value: String,
    #[serde(default)]
    pub filter_status: String,
}

async fn handle_external_service(
    State(state): State<AppState>,
    Json(req): Json<ExternalServiceRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    use sqlx::Row;
    let _api = verify_api_key(&state, &req.key, &req.sign).await?;

    let mut query_builder: sqlx::QueryBuilder<'_, sqlx::MySql> = sqlx::QueryBuilder::new("SELECT * FROM service WHERE sub != ''");
    if req.filter_type == "type" && !req.filter_value.is_empty() {
        query_builder.push(" AND type = ");
        query_builder.push_bind(&req.filter_value);
    }
    if req.filter_type == "brand" && !req.filter_value.is_empty() {
        query_builder.push(" AND game = ");
        query_builder.push_bind(&req.filter_value);
    }
    if !req.filter_status.is_empty() {
        query_builder.push(" AND status = ");
        query_builder.push_bind(&req.filter_status);
    }
    query_builder.push(" ORDER BY name ASC, price ASC");

    let rows = query_builder.build().fetch_all(&state.db).await.unwrap_or_default();
    if rows.is_empty() {
        return Ok(Json(json!({ "result": false, "data": null, "message": "Layanan tidak ditemukan." })));
    }

    let data: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|r| {
            let price: f64 = r.try_get("price").unwrap_or_default();
            let member: f64 = r.try_get("member").unwrap_or_default();
            let reseller: f64 = r.try_get("reseller").unwrap_or_default();
            json!({
                "code": r.try_get::<String,_>("code").unwrap_or_default(),
                "category": r.try_get::<String,_>("game").unwrap_or_default(),
                "name": r.try_get::<String,_>("name").unwrap_or_default(),
                "type": r.try_get::<String,_>("type").unwrap_or_default(),
                "price": {
                    "guest": price as i64,
                    "member": (price + member) as i64,
                    "reseller": (price + reseller) as i64
                },
                "varian": "",
                "status": r.try_get::<String,_>("status").unwrap_or_default(),
                "update_at": r.try_get::<String,_>("date_up").unwrap_or_default(),
            })
        })
        .collect();

    Ok(Json(json!({ "result": true, "data": data })))
}

#[derive(Deserialize)]
pub struct ExternalStatusRequest {
    pub key: String,
    pub sign: String,
    #[serde(default)]
    pub order_id: String,
    #[serde(default)]
    pub limit: Option<i64>,
}

async fn handle_external_status(
    State(state): State<AppState>,
    Json(req): Json<ExternalStatusRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    use sqlx::Row;
    let api = verify_api_key(&state, &req.key, &req.sign).await?;
    let username: String = api.try_get("user").unwrap_or_default();

    let mut query_builder: sqlx::QueryBuilder<'_, sqlx::MySql> = sqlx::QueryBuilder::new("SELECT * FROM transaction WHERE user = ");
    query_builder.push_bind(&username);
    if !req.order_id.is_empty() {
        query_builder.push(" AND order_id = ");
        query_builder.push_bind(&req.order_id);
    }
    query_builder.push(" ORDER BY id DESC");
    if let Some(l) = req.limit {
        let safe_limit = l.clamp(1, 100);
        query_builder.push(" LIMIT ");
        query_builder.push_bind(safe_limit);
    }

    let rows = query_builder.build().fetch_all(&state.db).await.unwrap_or_default();
    if rows.is_empty() {
        return Ok(Json(json!({ "result": false, "data": null, "message": "Transaksi tidak ditemukan." })));
    }

    let data: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|r| {
            json!({
                "order_id": r.try_get::<String,_>("order_id").unwrap_or_default(),
                "data": r.try_get::<String,_>("data").unwrap_or_default(),
                "code": r.try_get::<String,_>("code").unwrap_or_default(),
                "service": r.try_get::<String,_>("name").unwrap_or_default(),
                "status": r.try_get::<String,_>("status").unwrap_or_default(),
                "note": r.try_get::<String,_>("note").unwrap_or_default(),
                "price": r.try_get::<f64,_>("price").unwrap_or_default() as i64,
            })
        })
        .collect();

    Ok(Json(json!({ "result": true, "data": data })))
}

async fn handle_create_invoice(
    State(state): State<AppState>,
    Json(req): Json<CreateInvoiceRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    use sqlx::Row;

    // MODE "existing_order": invoice gateway menempel ke order yang SUDAH ada
    // (dibuat oleh /api/v1/order/submit) — tanpa duplikasi baris transaksi.
    let existing: Option<sqlx::mysql::MySqlRow> = match &req.existing_order_id {
        Some(oid) if !oid.is_empty() => {
            sqlx::query("SELECT order_id, code, service_name, game, target, user, price, payment_code, status, payment_status FROM transaction WHERE order_id = ? LIMIT 1")
                .bind(oid)
                .fetch_optional(&state.db)
                .await?
        }
        _ => None,
    };

    // ANTI DOUBLE-INVOICE: order yang sudah dibayar / sedang diproses
    // tidak boleh dibuatkan tagihan kedua.
    if let Some(ref row) = existing {
        let pay_status: String = row.try_get("payment_status").unwrap_or_default();
        let trx_status: String = row.try_get("status").unwrap_or_default();
        if pay_status == "paid" || matches!(trx_status.as_str(), "success" | "processing" | "process") {
            return Ok(Json(json!({
                "result": false,
                "message": "Order ini sudah dibayar / sedang diproses — tagihan baru ditolak."
            })));
        }
    }

    let (order_id, total_price, inv_name, inv_game, inv_target): (String, f64, String, String, String) = if let Some(ref row) = existing {
        (
            row.try_get::<String, _>("order_id").unwrap_or_default(),
            row.try_get::<f64, _>("price").unwrap_or(0.0),
            row.try_get::<String, _>("service_name").unwrap_or_default(),
            row.try_get::<String, _>("game").unwrap_or_default(),
            row.try_get::<String, _>("target").unwrap_or_default(),
        )
    } else {
        let service = sqlx::query_as::<MySql, Service>(
            "SELECT * FROM service WHERE code = ? AND status = 'available'",
        )
        .bind(&req.service_code)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::ServiceNotFound)?;

        let username = req.username.clone().unwrap_or_else(|| "GUEST".to_string());
        let is_member = username != "GUEST" && username != "-" && !username.is_empty();
        // Margin mengikuti level user (paritas dengan fulfillment)
        let profit = if is_member {
            let level: String = sqlx::query_scalar("SELECT COALESCE(level, 'Member') FROM users WHERE username = ? LIMIT 1")
                .bind(&username)
                .fetch_optional(&state.db)
                .await
                .ok()
                .flatten()
                .unwrap_or_else(|| "Member".to_string());
            if level == "Reseller" || level == "Admin" { service.reseller } else { service.member }
        } else {
            0.0
        };
        let total_price = service.price + profit;
        let order_id = format!("ORD{}{}", chrono::Utc::now().format("%Y%m%d%H%M%S"), rand::random::<u16>());

        sqlx::query(
            "INSERT INTO transaction (order_id, order_tid, user, code, service_name, game, target, price, profit, status, payment_status, provider, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', 'unpaid', ?, NOW(), NOW())"
        )
        .bind(&order_id)
        .bind(&order_id)
        .bind(&username)
        .bind(&service.code)
        .bind(&service.name)
        .bind(&service.game)
        .bind(&req.target)
        .bind(total_price)
        .bind(profit)
        .bind(&service.provider)
        .execute(&state.db)
        .await?;

        (order_id, total_price, service.name.clone(), service.game.clone(), req.target.clone())
    };

    let username = req.username.clone().unwrap_or_else(|| "GUEST".to_string());
    let voucher_token = req.voucher.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("-");

    // MODE AUTO: gateway & channel diambil dari metode yang dipilih user saat submit
    // (tersimpan di transaction.payment_code). APP/saldo → SALDO.
    let mut effective_gateway = req.payment_gateway.to_uppercase();
    let mut effective_channel = req.payment_channel.clone();
    if effective_gateway == "AUTO" {
        if let Some(ref row) = existing {
            let pay_code: String = row.try_get("payment_code").unwrap_or_default();
            if pay_code.is_empty() {
                return Ok(Json(json!({ "result": false, "message": "Menunggu konfirmasi pesanan..." })));
            }
            let mrow = sqlx::query("SELECT provider, code, type FROM metode WHERE code = ? LIMIT 1")
                .bind(&pay_code)
                .fetch_optional(&state.db)
                .await?
                .ok_or(AppError::ServiceNotFound)?;
            let m_provider: String = mrow.try_get("provider").unwrap_or_else(|_| "X".to_string());
            let m_code: String = mrow.try_get("code").unwrap_or_default();
            let m_type: String = mrow.try_get("type").unwrap_or_default();
            if m_provider == "APP" || m_type == "app" {
                effective_gateway = "SALDO".to_string();
            } else {
                effective_gateway = m_provider;
            }
            effective_channel = m_code;
        } else {
            return Ok(Json(json!({ "result": false, "message": "Order tidak ditemukan" })));
        }
    }

    let return_url = "https://aruterushoppu.com/order/status";
    let public_base = std::env::var("PUBLIC_BASE_URL")
        .unwrap_or_else(|_| format!("http://127.0.0.1:{}", state.config.server_port));

    let clean_svc_name = inv_name.replace(' ', "_").replace(';', "");
    // Saat mode existing_order, [NEW_ORDER] sudah dikirim oleh /api/v1/order/submit
    // (lengkap dengan voucher) — jangan broadcast ganda.
    let msg_text = format!(
        "[NEW_ORDER] {} {} {} {} {} {} {} {}",
        order_id, username, req.service_code, inv_target, "-", total_price, clean_svc_name, voucher_token
    );
    let broadcast_new_order = existing.is_none();

    // Idempotensi: kalau invoice untuk (order, gateway) ini sudah pernah dibuat
    // dan belum lewat 24 jam → kembalikan TAGIHAN YANG SAMA.
    let inv_cache_key = format!("{}|{}", order_id, effective_gateway);
    // Eviksi: buang entri kadaluarsa (>24 jam) saat peta membengkak
    if INVOICE_CACHE.len() > 10_000 {
        let now_ts = chrono::Utc::now().timestamp();
        INVOICE_CACHE.retain(|_, v| now_ts - v.0 < 86_400);
    }
    if existing.is_some() {
        if let Some(kv) = INVOICE_CACHE.get(&inv_cache_key) {
            let (ts, ref cached) = *kv.value();
            if chrono::Utc::now().timestamp() - ts < 86_400 {
                let mut resp = cached.clone();
                resp["message"] = json!("Invoice sudah ada — menggunakan tagihan yang sama (idempoten).");
                return Ok(Json(resp));
            }
        }
    }

    match effective_gateway.as_str() {
        "TRIPAY" => {
            let merchant_code = std::env::var("TRIPAY_MERCHANT_CODE").unwrap_or_default();
            let api_key = std::env::var("TRIPAY_API_KEY").unwrap_or_default();
            let private_key = std::env::var("TRIPAY_PRIVATE_KEY").unwrap_or_default();
            let tripay = TripayGateway::new(merchant_code, api_key, private_key, false);

            let inv = tripay
                .create_transaction(
                    &state.http_client,
                    &effective_channel,
                    &order_id,
                    total_price,
                    &inv_name,
                    &inv_target,
                    &req.customer_name,
                    &req.customer_email,
                    &req.customer_phone,
                    return_url,
                )
                .await?;

            if broadcast_new_order {
                let _ = rust_backend::domain::telegram::sender::send_report_to_fulfillment(&msg_text).await;
            }

            let resp = json!({
                "result": true,
                "data": {
                    "order_id": order_id,
                    "payment_gateway": "TRIPAY",
                    "checkout_url": inv.checkout_url,
                    "qr_string": inv.qr_string,
                    "pay_code": inv.pay_code,
                    "amount": inv.amount
                },
                "message": "Invoice Tripay berhasil dibuat dengan rincian barang"
            });
            INVOICE_CACHE.insert(inv_cache_key, (chrono::Utc::now().timestamp(), resp.clone()));
            return Ok(Json(resp));
        }
        "DUITKU" => {
            let merchant_code = std::env::var("DUITKU_MERCHANT_CODE").unwrap_or_default();
            let api_key = std::env::var("DUITKU_API_KEY").unwrap_or_default();
            let duitku = DuitkuGateway::new(merchant_code, api_key, false);

            let callback_url = format!("{}/webhook/duitku", public_base);
            let inv = duitku
                .create_transaction(
                    &state.http_client,
                    &effective_channel,
                    &order_id,
                    total_price as u64,
                    &inv_name,
                    &inv_target,
                    &req.customer_name,
                    &req.customer_email,
                    &req.customer_phone,
                    return_url,
                    &callback_url,
                )
                .await?;

            if broadcast_new_order {
                let _ = rust_backend::domain::telegram::sender::send_report_to_fulfillment(&msg_text).await;
            }

            let resp = json!({
                "result": true,
                "data": {
                    "order_id": order_id,
                    "payment_gateway": "DUITKU",
                    "payment_url": inv.paymentUrl,
                    "qr_string": inv.qrString,
                    "amount": total_price
                },
                "message": "Invoice Duitku berhasil dibuat dengan rincian barang"
            });
            INVOICE_CACHE.insert(inv_cache_key, (chrono::Utc::now().timestamp(), resp.clone()));
            return Ok(Json(resp));
        }
        "TOKOPAY" => {
            let merchant_id = std::env::var("TOKOPAY_MERCHANT_ID").unwrap_or_default();
            let secret_key = std::env::var("TOKOPAY_SECRET_KEY").unwrap_or_default();
            let tokopay = TokopayGateway::new(merchant_id, secret_key);

            let inv = tokopay
                .create_transaction(
                    &state.http_client,
                    &effective_channel,
                    &order_id,
                    total_price as u64,
                    &inv_game,
                    &inv_name,
                    &inv_target,
                    &req.customer_name,
                    &req.customer_email,
                    &req.customer_phone,
                    return_url,
                )
                .await?;

            if broadcast_new_order {
                let _ = rust_backend::domain::telegram::sender::send_report_to_fulfillment(&msg_text).await;
            }

            let resp = json!({
                "result": true,
                "data": {
                    "order_id": order_id,
                    "payment_gateway": "TOKOPAY",
                    "pay_url": inv.pay_url,
                    "qr_link": inv.qr_link,
                    "amount": total_price
                },
                "message": "Invoice Tokopay berhasil dibuat dengan rincian barang"
            });
            INVOICE_CACHE.insert(inv_cache_key, (chrono::Utc::now().timestamp(), resp.clone()));
            return Ok(Json(resp));
        }
        "PAYDISINI" => {
            let api_key = std::env::var("PAYDISINI_API_KEY").unwrap_or_default();
            let merchant_id = std::env::var("PAYDISINI_MERCHANT_ID").unwrap_or_default();
            let paydisini = PaydisiniGateway::new(api_key, merchant_id);

            let inv = paydisini
                .create_transaction(
                    &state.http_client,
                    &effective_channel,
                    &order_id,
                    total_price as u64,
                    &inv_name,
                    &inv_target,
                    &req.customer_email,
                    &req.customer_phone,
                    return_url,
                )
                .await?;

            if broadcast_new_order {
                let _ = rust_backend::domain::telegram::sender::send_report_to_fulfillment(&msg_text).await;
            }

            let resp = json!({
                "result": true,
                "data": {
                    "order_id": order_id,
                    "payment_gateway": "PAYDISINI",
                    "checkout_url": inv.checkout_url,
                    "qr_content": inv.qr_content,
                    "pay_code": inv.pay_code,
                    "amount": total_price
                },
                "message": "Invoice Paydisini berhasil dibuat dengan rincian barang"
            });
            INVOICE_CACHE.insert(inv_cache_key, (chrono::Utc::now().timestamp(), resp.clone()));
            return Ok(Json(resp));
        }
        "SALDO" => {
            if username == "GUEST" || username.is_empty() {
                return Err(AppError::InternalError("Harap login untuk menggunakan Saldo".to_string()));
            }

            // Web Server purely sends REQ_OTP to Fulfillment. It doesn't generate OTP or check balance itself!
            // Fulfillment menghitung harga otoritatifnya sendiri (base+margin+flashsale-voucher).
            let msg = format!("[REQ_OTP] {} {} {} {} {} {}", order_id, username, req.service_code, inv_target, total_price, voucher_token);
            let _ = rust_backend::domain::telegram::sender::send_report_to_fulfillment(&msg).await;

            Ok(Json(json!({
                "result": true,
                "data": {
                    "order_id": order_id,
                    "payment_gateway": "SALDO",
                    "checkout_url": format!("/order/verify-otp?order_id={}", order_id),
                    "amount": total_price
                },
                "message": "Permintaan OTP telah dikirim ke WhatsApp Anda"
            })))
        }
        _ => Err(AppError::ServerNotFound),
    }
}

async fn handle_verify_otp(
    State(_state): State<AppState>,
    Json(req): Json<OrderVerifyOtpRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if req.username.is_empty() || req.order_id.is_empty() || req.otp.is_empty() {
        return Err(AppError::InternalError("Data tidak lengkap".to_string()));
    }

    let msg = format!("[VERIFY] {} {} {}", req.order_id, req.username, req.otp);
    let _ = rust_backend::domain::telegram::sender::send_report_to_fulfillment(&msg).await;

    Ok(Json(json!({
        "result": true,
        "message": "OTP sedang divalidasi oleh sistem..."
    })))
}

// -----------------------------------------
// CSR PAGE DATA HANDLER
// -----------------------------------------

#[derive(serde::Serialize)]
pub struct BannerData {
    pub id: i32,
    pub content: String,
}

#[derive(serde::Serialize)]
pub struct TabData {
    pub id: i32,
    pub name: String,
    pub icon: String,
    #[serde(rename = "type")]
    pub type_name: String,
}

#[derive(serde::Serialize)]
pub struct CategoryData {
    pub id: i32,
    pub name: String,
    pub owner: String,
    pub image: String,
    #[serde(rename = "type")]
    pub type_name: String,
    pub slug: String,
}

#[derive(serde::Serialize)]
pub struct NewsData {
    pub id: i32,
    pub title: String,
    pub banner: String,
    pub slug: String,
    pub summary: String,
}

#[derive(serde::Serialize)]
pub struct FlashsaleData {
    pub id: i32,
    pub code: String,
    pub amount: f64,
    pub service_name: String,
    pub service_image: String,
    pub category_name: String,
    pub category_image: String,
    pub category_type: String,
    pub category_slug: String,
    pub real_price: f64,
    pub show_price: f64,
}

#[derive(serde::Serialize)]
pub struct SiteConfigData {
    pub flashsale_time: String,
    pub colors: [String; 4],
    pub social: [String; 5],
    pub popup: PopupData,
    pub footer_enabled: bool,
    pub ui_category: bool,
    pub pages: [String; 2],
}

#[derive(serde::Serialize)]
pub struct PopupData {
    pub show: bool,
    pub image: String,
    pub subtitle: String,
    pub content: String, // base64, frontend decodes
}

fn conf_cell(rows: &[sqlx::mysql::MySqlRow], code: &str, col: &str) -> String {
    use sqlx::Row;
    for r in rows {
        let c: String = r.try_get("code").unwrap_or_default();
        if c == code {
            return r.try_get(col).unwrap_or_default();
        }
    }
    String::new()
}

fn extract_image_url(image_json_str: &str) -> String {
    const DEFAULT_IMAGE: &str = "/library/assets/guest/images/game/default-image.png";

    if image_json_str.is_empty() || image_json_str == "-" {
        return DEFAULT_IMAGE.to_string();
    }
    // The `image` column stores JSON like {"image":"https://..."}; mirror PHP's
    // data_json($image,'image'). Fall back to the raw string when it is not
    // valid JSON (e.g. a plain URL) so images still resolve.
    match serde_json::from_str::<serde_json::Value>(image_json_str) {
        Ok(v) => {
            if let Some(img) = v.get("image").and_then(|i| i.as_str()) {
                if img.is_empty() || img == "-" {
                    return DEFAULT_IMAGE.to_string();
                }
                return img.to_string();
            }
            DEFAULT_IMAGE.to_string()
        }
        Err(_) => image_json_str.to_string(),
    }
}

fn slugify(s: &str) -> String {
    s.to_lowercase().replace(" ", "-")
}

async fn handle_page_data(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    // 0. Fetch conf table (flashsale, color, social, popup, footer)
    let conf_rows = sqlx::query(r#"SELECT code, c1, c2, c3, c4, c5 FROM conf"#)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let flashsale_time = conf_cell(&conf_rows, "flashsale", "c1");
    let colors = [
        conf_cell(&conf_rows, "color", "c2"),
        conf_cell(&conf_rows, "color", "c1"),
        conf_cell(&conf_rows, "color", "c3"),
        conf_cell(&conf_rows, "color", "c4"),
    ];
    let social = [
        conf_cell(&conf_rows, "social", "c1"),
        conf_cell(&conf_rows, "social", "c2"),
        conf_cell(&conf_rows, "social", "c3"),
        conf_cell(&conf_rows, "social", "c4"),
        conf_cell(&conf_rows, "social", "c5"),
    ];
    let popup_show = conf_cell(&conf_rows, "popup", "c4");
    let popup = PopupData {
        show: popup_show == "display",
        image: conf_cell(&conf_rows, "popup", "c1"),
        subtitle: conf_cell(&conf_rows, "popup", "c2"),
        content: conf_cell(&conf_rows, "popup", "c3"),
    };
    let footer_enabled = conf_cell(&conf_rows, "footer", "c1") != "false";
    let ui_category = conf_cell(&conf_rows, "ui_category", "c1") == "true";
    let pages = [
        conf_cell(&conf_rows, "pages", "c1"),
        conf_cell(&conf_rows, "pages", "c2"),
    ];

    // 1. Fetch Banners
    let banners_rows = sqlx::query(r#"SELECT id, content FROM banner ORDER BY id ASC"#)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    
    let banners: Vec<BannerData> = banners_rows
        .into_iter()
        .map(|r| {
            use sqlx::Row;
            BannerData {
                id: r.try_get("id").unwrap_or_default(),
                content: r.try_get("content").unwrap_or_default(),
            }
        })
        .collect();

    // 2. Fetch Tabs
    let tabs_rows = sqlx::query(r#"SELECT id, name, icon, type FROM tabs ORDER BY `order` ASC"#)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    
    let tabs: Vec<TabData> = tabs_rows
        .into_iter()
        .map(|r| {
            use sqlx::Row;
            TabData {
                id: r.try_get("id").unwrap_or_default(),
                name: r.try_get("name").unwrap_or_default(),
                icon: r.try_get("icon").unwrap_or_default(),
                type_name: r.try_get("type").unwrap_or_default(),
            }
        })
        .collect();

    // 3. Fetch Popular Categories
    let pop_rows = sqlx::query(r#"SELECT id, name, owner, image, type FROM category WHERE popular = '1' ORDER BY `order` ASC"#)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    
    let popular_games: Vec<CategoryData> = pop_rows
        .into_iter()
        .map(|r| {
            use sqlx::Row;
            let name: String = r.try_get("name").unwrap_or_default();
            CategoryData {
                id: r.try_get("id").unwrap_or_default(),
                name: name.clone(),
                owner: r.try_get("owner").unwrap_or_default(),
                image: extract_image_url(&r.try_get::<String, _>("image").unwrap_or_default()),
                type_name: r.try_get("type").unwrap_or_default(),
                slug: name.to_lowercase().replace(" ", "-"),
            }
        })
        .collect();

    // 4. Fetch All Categories mapped by tab type
    let mut categories_map = std::collections::HashMap::new();
    let cat_rows = sqlx::query(r#"SELECT id, name, owner, image, type FROM category ORDER BY name ASC"#)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    
    for r in cat_rows {
        use sqlx::Row;
        let name: String = r.try_get("name").unwrap_or_default();
        let type_name: String = r.try_get("type").unwrap_or_default();
        let cat = CategoryData {
            id: r.try_get("id").unwrap_or_default(),
            name: name.clone(),
            owner: r.try_get("owner").unwrap_or_default(),
            image: extract_image_url(&r.try_get::<String, _>("image").unwrap_or_default()),
            type_name: type_name.clone(),
            slug: name.to_lowercase().replace(" ", "-"),
        };
        categories_map
            .entry(type_name)
            .or_insert_with(Vec::new)
            .push(cat);
    }

    // 5. Fetch News
    let news_rows = sqlx::query(r#"SELECT id, title, banner, content FROM blog ORDER BY id ASC"#)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    
    let news: Vec<NewsData> = news_rows
        .into_iter()
        .map(|r| {
            use sqlx::Row;
            let title: String = r.try_get("title").unwrap_or_default();
            // Content is base64 encoded. Pass raw to frontend.
            let content: String = r.try_get("content").unwrap_or_default();
            
            NewsData {
                id: r.try_get("id").unwrap_or_default(),
                title: title.clone(),
                banner: r.try_get("banner").unwrap_or_default(),
                slug: title.to_lowercase().replace(" ", "-"),
                summary: content, // frontend will decode base64
            }
        })
        .collect();

    // 6. Fetch Flashsale (mirror app/guest/flashsale.php)
    let mut flashsales: Vec<FlashsaleData> = Vec::new();
    let flash_rows = sqlx::query(r#"SELECT id, code, amount FROM flashsale WHERE status = '1' ORDER BY amount DESC"#)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    for fr in flash_rows {
        use sqlx::Row;
        let fs_code: String = fr.try_get("code").unwrap_or_default();
        let fs_amount: f64 = fr.try_get("amount").unwrap_or_default();
        let fs_id: i32 = fr.try_get("id").unwrap_or_default();

        // service by code (status available)
        let service_row = sqlx::query(
            r#"SELECT name, game, price FROM service WHERE code = ? AND status = 'available' LIMIT 1"#,
        )
        .bind(&fs_code)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);

        if let Some(sr) = service_row {
            let srv_name: String = sr.try_get("name").unwrap_or_default();
            let srv_game: String = sr.try_get("game").unwrap_or_default();
            let srv_price: f64 = sr.try_get("price").unwrap_or_default();

            let real_price = srv_price;
            let show_price = real_price - fs_amount;

            // category by game name
            let cat_row = sqlx::query(
                r#"SELECT name, type, image FROM category WHERE name = ? LIMIT 1"#,
            )
            .bind(&srv_game)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);

            if let Some(cr) = cat_row {
                let cat_name: String = cr.try_get("name").unwrap_or_default();
                let cat_type: String = cr.try_get("type").unwrap_or_default();
                let cat_image: String = cr.try_get("image").unwrap_or_default();

                flashsales.push(FlashsaleData {
                    id: fs_id,
                    code: fs_code,
                    amount: fs_amount,
                    service_name: srv_name,
                    service_image: String::new(),
                    category_name: cat_name.clone(),
                    category_image: extract_image_url(&cat_image),
                    category_type: cat_type,
                    category_slug: slugify(&cat_name),
                    real_price,
                    show_price,
                });
            }
        }
    }

    let config = SiteConfigData {
        flashsale_time,
        colors,
        social,
        popup,
        footer_enabled,
        ui_category,
        pages,
    };

    Ok(Json(json!({
        "banners": banners,
        "tabs": tabs,
        "popularGames": popular_games,
        "categories": categories_map,
        "news": news,
        "flashsale": flashsales,
        "config": config
    })))
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: String,
}

async fn handle_search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Mirror library/ajax/searching-game.php: category name LIKE, limit 15
    let search_term = format!("%{}%", query.q.trim());
    let rows = sqlx::query(
        r#"SELECT id, name, owner, image, type FROM category WHERE LOWER(name) LIKE LOWER(?) ORDER BY name ASC LIMIT 15"#,
    )
    .bind(&search_term)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let results: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|r| {
            use sqlx::Row;
            let name: String = r.try_get("name").unwrap_or_default();
            json!({
                "id": r.try_get::<i32, _>("id").unwrap_or_default(),
                "name": name.clone(),
                "owner": r.try_get::<String, _>("owner").unwrap_or_default(),
                "image": extract_image_url(&r.try_get::<String, _>("image").unwrap_or_default()),
                "type": r.try_get::<String, _>("type").unwrap_or_default(),
                "slug": slugify(&name),
            })
        })
        .collect();

    Ok(Json(json!({ "result": true, "data": results })))
}

// ============================================================
// PUBLIC PAGE DATA HANDLERS
// ============================================================

fn extract_banner_url(image_json_str: &str) -> String {
    const DEFAULT_BANNER: &str = "/library/assets/guest/images/banner/default-banner.png";
    if image_json_str.is_empty() || image_json_str == "-" {
        return DEFAULT_BANNER.to_string();
    }
    match serde_json::from_str::<serde_json::Value>(image_json_str) {
        Ok(v) => {
            if let Some(b) = v.get("banner").and_then(|x| x.as_str()) {
                if b.is_empty() || b == "-" {
                    return DEFAULT_BANNER.to_string();
                }
                return b.to_string();
            }
            DEFAULT_BANNER.to_string()
        }
        Err(_) => image_json_str.to_string(),
    }
}

async fn handle_product(
    State(state): State<AppState>,
    axum::extract::Path((type_name, slug)): axum::extract::Path<(String, String)>,
) -> Result<Json<serde_json::Value>, AppError> {
    use sqlx::Row;

    // In this DB schema, category is identified by name (+ type), not code.
    let code = slug.replace("-", " ");

    let cat_row = sqlx::query("SELECT * FROM category WHERE name = ? AND type = ? LIMIT 1")
        .bind(&code)
        .bind(&type_name)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::ServiceNotFound)?;

    let name: String = cat_row.try_get("name").unwrap_or_default();
    let owner: String = cat_row.try_get("owner").unwrap_or_default();
    let image_raw: String = cat_row.try_get("image").unwrap_or_default();
    let cat_id: i32 = cat_row.try_get("id").unwrap_or_default();

    let category_image = extract_image_url(&image_raw);
    let category_banner = extract_banner_url(&image_raw);

    // Services for this category (guest price = price; schema has no "tamu" column)
    let service_rows = sqlx::query(
        "SELECT id, code, name, game, price, member, reseller, provider, status FROM service WHERE game = ? AND status = 'available' ORDER BY price ASC",
    )
    .bind(&code)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let mut services = Vec::new();
    for sr in service_rows {
        let srv_code: String = sr.try_get("code").unwrap_or_default();
        let srv_name: String = sr.try_get("name").unwrap_or_default();
        let srv_price: f64 = sr.try_get("price").unwrap_or_default();
        let srv_member: f64 = sr.try_get("member").unwrap_or_default();
        let srv_reseller: f64 = sr.try_get("reseller").unwrap_or_default();

        // flashsale discount
        let flash_row = sqlx::query("SELECT amount FROM flashsale WHERE code = ? AND status = '1' LIMIT 1")
            .bind(&srv_code)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);
        let discount: f64 = flash_row
            .as_ref()
            .and_then(|r| r.try_get("amount").ok())
            .unwrap_or(0.0);

        let real_price = srv_price;
        services.push(json!({
            "id": sr.try_get::<i32,_>("id").unwrap_or_default(),
            "code": srv_code,
            "name": srv_name,
            "sub": "",
            "price": srv_price,
            "price_tamu": real_price,
            "price_member": srv_price + srv_member,
            "price_reseller": srv_price + srv_reseller,
            "show_price": real_price - discount,
            "discount": discount,
            "image": "",
            "provider": sr.try_get::<String,_>("provider").unwrap_or_default(),
        }));
    }

    // Reviews: read from `ulasan` when the table exists (graceful empty otherwise)
    let review_rows = sqlx::query(
        "SELECT username, order_id, rating, message, date_cr FROM ulasan WHERE category = ? ORDER BY id DESC LIMIT 5",
    )
    .bind(&code)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let reviews: Vec<serde_json::Value> = review_rows
        .iter()
        .map(|r| {
            let oid: String = r.try_get("order_id").unwrap_or_default();
            json!({
                "user": r.try_get::<String, _>("username").unwrap_or_default(),
                "order_id": oid,
                "rating": r.try_get::<i64, _>("rating").unwrap_or_default(),
                "message": r.try_get::<String, _>("message").unwrap_or_default(),
                "date_cr": r.try_get::<String, _>("date_cr").unwrap_or_default(),
            })
        })
        .collect();

    let (total_review, average_rating, star_counts) = if sqlx::query("SELECT 1 FROM ulasan LIMIT 1")
        .fetch_optional(&state.db)
        .await
        .map(|r| r.is_some())
        .unwrap_or(false)
    {
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ulasan WHERE category = ?")
            .bind(&code)
            .fetch_one(&state.db)
            .await
            .unwrap_or(0);
        let avg: f64 = sqlx::query_scalar("SELECT COALESCE(AVG(rating),0) FROM ulasan WHERE category = ?")
            .bind(&code)
            .fetch_one(&state.db)
            .await
            .unwrap_or(0.0);
        let mut counts = vec![0i64; 5];
        for star in 1..=5i64 {
            counts[(star - 1) as usize] = sqlx::query_scalar("SELECT COUNT(*) FROM ulasan WHERE category = ? AND rating = ?")
                .bind(&code)
                .bind(star)
                .fetch_one(&state.db)
                .await
                .unwrap_or(0);
        }
        (total, avg, counts)
    } else {
        (0i64, 0.0f64, vec![0i64; 5])
    };
    let progress: Vec<f64> = if total_review > 0 {
        star_counts.iter().map(|c| (*c as f64 / total_review as f64) * 100.0).collect()
    } else {
        vec![0.0; 5]
    };

    // Dynamic account form + notes from category row (parity with PHP data_form builder)
    let data_form_raw: String = cat_row.try_get("data_form").unwrap_or_default();
    let form_fields: serde_json::Value = serde_json::from_str::<serde_json::Value>(&data_form_raw).unwrap_or(json!([]));
    let note_b64: String = cat_row.try_get("note").unwrap_or_default();
    let target_note: String = cat_row.try_get("target_note").unwrap_or_default();
    let apk_raw: String = cat_row.try_get("apk").unwrap_or_default();
    let prefix: String = cat_row.try_get("prefix").unwrap_or_default();

    Ok(Json(json!({
        "result": true,
        "data": {
            "id": cat_id,
            "name": name,
            "owner": owner,
            "type": type_name,
            "code": code,
            "slug": slugify(&name),
            "image": category_image,
            "banner": category_banner,
            "note": try_decode_base64(&note_b64),
            "target_note": target_note,
            "apk": apk_raw == "1" || apk_raw.eq_ignore_ascii_case("true"),
            "prefix": prefix,
            "data_form": form_fields,
            "status": "available",
            "services": services,
            "reviews": reviews,
            "total_review": total_review,
            "average_rating": average_rating,
            "star_progress": progress,
            "star_counts": star_counts
        }
    })))
}

async fn handle_invoices(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    use sqlx::Row;
    let rows = sqlx::query(
        "SELECT order_id, target, price, status, payment_status, created_at FROM transaction ORDER BY id DESC LIMIT 10",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let data: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|r| {
            let created: Option<chrono::NaiveDateTime> = r.try_get("created_at").unwrap_or(None);
            json!({
                "order_id": r.try_get::<String,_>("order_id").unwrap_or_default(),
                "target": r.try_get::<String,_>("target").unwrap_or_default(),
                "price": r.try_get::<f64,_>("price").unwrap_or_default(),
                "status": r.try_get::<String,_>("status").unwrap_or_default(),
                "status_payment": r.try_get::<String,_>("payment_status").unwrap_or_default(),
                "created_at": created.map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_default(),
            })
        })
        .collect();

    Ok(Json(json!({ "result": true, "data": data })))
}

#[derive(Deserialize)]
pub struct ForgotPasswordRequest {
    pub phone: String,
}

async fn handle_forgot_password(
    State(state): State<AppState>,
    Json(req): Json<ForgotPasswordRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    use sqlx::Row;

    let phone = req.phone.trim().to_string();
    if phone.is_empty() {
        return Ok(Json(json!({ "result": false, "message": "Nomor WhatsApp tidak boleh kosong." })));
    }

    let user_row = sqlx::query("SELECT * FROM users WHERE phone = ? LIMIT 1")
        .bind(&phone)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::UserNotFound)?;

    let _username: String = user_row.try_get("username").unwrap_or_default();

    // Generate random 10-char password
    let charset: Vec<char> = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789".chars().collect();
    let new_password: String = (0..10).map(|_| charset[rand::random::<usize>() % charset.len()]).collect();
    let hashed = bcrypt::hash(&new_password, bcrypt::DEFAULT_COST).unwrap_or_default();

    sqlx::query("UPDATE users SET password = ? WHERE phone = ?")
        .bind(&hashed)
        .bind(&phone)
        .execute(&state.db)
        .await?;

    // Send WhatsApp notification via universal WA Gateway (OpenWA / MPWA compatible)
    let mpwa_token = std::env::var("MPWA_API_KEY").unwrap_or_default();
    let mpwa_sender = std::env::var("MPWA_SENDER_PHONE").unwrap_or_default();
    if !mpwa_token.is_empty() {
        let msg = format!("Password akun Aruteru Shoppu Anda telah direset.\nPassword baru: {}", new_password);
        let _ = rust_backend::domain::auth::service::send_whatsapp_message(&mpwa_token, &mpwa_sender, &phone, &msg).await;
    }

    Ok(Json(json!({ "result": true, "message": "Password baru telah dikirim ke WhatsApp Anda." })))
}

#[derive(Deserialize)]
pub struct SubmitReviewRequest {
    pub order_id: String,
    pub rating: i64,
    pub message: String,
}

async fn handle_review_submit(
    State(state): State<AppState>,
    Json(req): Json<SubmitReviewRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    use sqlx::Row;

    if req.rating < 1 || req.rating > 5 {
        return Ok(Json(json!({ "result": false, "message": "Kesalahan Rating." })));
    }
    if req.message.trim().is_empty() {
        return Ok(Json(json!({ "result": false, "message": "Pesan tidak boleh kosong." })));
    }

    // Must be a successful, paid transaction
    let trx_row = sqlx::query("SELECT * FROM transaction WHERE order_id = ? AND status = 'success' AND payment_status = 'paid' LIMIT 1")
        .bind(&req.order_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::ServiceNotFound)?;

    let user: String = trx_row.try_get("user").unwrap_or_default();
    let username = if user.is_empty() || user == "-" { "Anonim".to_string() } else { user };
    let category: String = trx_row.try_get("game").unwrap_or_default();
    let name: String = trx_row.try_get("service_name").unwrap_or_default();
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

    // Persist into `ulasan` when the table exists (no DDL is ever executed).
    let has_table = sqlx::query("SELECT 1 FROM ulasan LIMIT 1")
        .fetch_optional(&state.db)
        .await
        .map(|r| r.is_some())
        .unwrap_or(false);

    if !has_table {
        return Ok(Json(json!({ "result": false, "message": "Fitur ulasan belum aktif." })));
    }

    // Prevent duplicate review for the same order
    let already: Option<(i64,)> = sqlx::query_as("SELECT id FROM ulasan WHERE order_id = ? LIMIT 1")
        .bind(&req.order_id)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);
    if already.is_some() {
        return Ok(Json(json!({ "result": false, "message": "Anda sudah memberi ulasan untuk pesanan ini." })));
    }

    let insert = sqlx::query("INSERT INTO ulasan (username, order_id, category, name, rating, message, date_cr) VALUES (?, ?, ?, ?, ?, ?, ?)")
        .bind(&username)
        .bind(&req.order_id)
        .bind(&category)
        .bind(&name)
        .bind(req.rating)
        .bind(&req.message)
        .bind(&now)
        .execute(&state.db)
        .await;

    match insert {
        Ok(_) => Ok(Json(json!({ "result": true, "message": "Terima kasih sudah memberi ulasan." }))),
        Err(e) => {
            tracing::warn!("[REVIEW] insert failed: {}", e);
            Ok(Json(json!({ "result": false, "message": "Gagal menyimpan ulasan." })))
        }
    }
}

#[derive(Deserialize)]
pub struct ReviewsQuery {
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

async fn handle_reviews(
    State(state): State<AppState>,
    Query(q): Query<ReviewsQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    use sqlx::Row;
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    let offset = q.offset.unwrap_or(0).max(0);

    // Graceful: if the ulasan table doesn't exist yet, return empty list.
    let rows = sqlx::query(
        "SELECT id, username, order_id, category, name, rating, message, date_cr FROM ulasan ORDER BY id DESC LIMIT ? OFFSET ?",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let data: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|r| {
            json!({
                "id": r.try_get::<i64, _>("id").unwrap_or_default(),
                "user": r.try_get::<String, _>("username").unwrap_or_default(),
                "order_id": r.try_get::<String, _>("order_id").unwrap_or_default(),
                "category": r.try_get::<String, _>("category").unwrap_or_default(),
                "name": r.try_get::<String, _>("name").unwrap_or_default(),
                "rating": r.try_get::<i64, _>("rating").unwrap_or_default(),
                "message": r.try_get::<String, _>("message").unwrap_or_default(),
                "date_cr": r.try_get::<String, _>("date_cr").unwrap_or_default(),
            })
        })
        .collect();

    Ok(Json(json!({ "result": true, "data": data })))
}

#[derive(Deserialize)]
pub struct SubmitOrderRequest {
    pub product: i64,          // service id
    pub method: String,        // metode code
    pub target: String,        // "userid - server" or comma-joined fields
    pub nickname: String,
    pub whaphone: String,
    pub username: Option<String>,
    pub voucher: Option<String>,
    pub csrf_token: Option<String>,
}

async fn handle_submit_order(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SubmitOrderRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    use sqlx::Row;

    // ANTI-SPAM: maksimal 6 order/menit per IP
    let ip = client_ip_from_headers(&headers);
    if !rate_limit(&format!("order:{}", ip), 6, 60) {
        return Ok(Json(json!({ "result": false, "message": "Terlalu banyak pesanan dalam waktu singkat. Tunggu sebentar." })));
    }

    // Service lookup
    let srv_row = sqlx::query("SELECT * FROM service WHERE id = ? AND status = 'available' LIMIT 1")
        .bind(req.product)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::ServiceNotFound)?;

    let srv_price: f64 = srv_row.try_get("price").unwrap_or_default();
    let srv_member: f64 = srv_row.try_get("member").unwrap_or_default();
    let srv_reseller: f64 = srv_row.try_get("reseller").unwrap_or_default();
    let srv_provider: String = srv_row.try_get("provider").unwrap_or_default();
    let srv_code: String = srv_row.try_get("code").unwrap_or_default();
    let srv_name: String = srv_row.try_get("name").unwrap_or_default();
    let srv_game: String = srv_row.try_get("game").unwrap_or_default();

    // Member vs Guest pricing — margin mengikuti LEVEL user (paritas dengan fulfillment)
    let lann_user = req.username.as_deref().unwrap_or("-").to_string();
    let is_member = lann_user != "-" && lann_user != "GUEST" && !lann_user.is_empty();
    let profit = if is_member {
        let level: String = sqlx::query_scalar("SELECT COALESCE(level, 'Member') FROM users WHERE username = ? LIMIT 1")
            .bind(&lann_user)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "Member".to_string());
        if level == "Reseller" || level == "Admin" { srv_reseller } else { srv_member }
    } else {
        0.0
    };
    let real_price = srv_price + profit;

    // Flashsale discount
    let mut flash_discount = 0.0f64;
    let f_row = sqlx::query("SELECT * FROM flashsale WHERE code = ? AND status = '1' LIMIT 1")
        .bind(&srv_code)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);
    if let Some(fr) = &f_row {
        flash_discount = fr.try_get("amount").unwrap_or_default();
    }

    // Voucher (exclusive with flashsale, parity with PHP order-action.php)
    let mut voucher_discount = 0.0f64;
    let mut voucher_snapshot = String::new();
    if let Some(vcode) = req.voucher.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if flash_discount > 0.0 {
            return Ok(Json(json!({ "result": false, "message": "Voucher tidak bisa digunakan untuk produk flashsale." })));
        }
        let v_row = sqlx::query("SELECT * FROM voucher WHERE voucher = ? LIMIT 1")
            .bind(vcode)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);
        let v = match v_row {
            Some(v) => v,
            None => {
                // ANTI-PROBING: kode salah di submit juga dihitung
                if !record_failure(&format!("vfail:{}", ip), 5, 600) {
                    return Ok(Json(json!({ "result": false, "message": "Akses voucher dibatasi sementara karena terlalu banyak kode salah." })));
                }
                return Ok(Json(json!({ "result": false, "message": "Kode voucher tidak valid." })));
            }
        };
        use sqlx::Row;
        let stock: i64 = v.try_get("stock").unwrap_or(0);
        if stock <= 0 {
            return Ok(Json(json!({ "result": false, "message": "Stok voucher sudah habis." })));
        }
        let v_category: String = v.try_get("code").unwrap_or_default();
        if !v_category.is_empty() && v_category != srv_game {
            return Ok(Json(json!({ "result": false, "message": "Voucher tidak berlaku untuk produk ini." })));
        }
        let minimum: f64 = v.try_get("minimum").unwrap_or(0.0);
        let price_before_voucher = (real_price - flash_discount).max(0.0);
        if minimum > 0.0 && price_before_voucher < minimum {
            return Ok(Json(json!({ "result": false, "message": format!("Minimum belanja Rp{} untuk memakai voucher ini.", minimum) })));
        }
        voucher_discount = v.try_get("discount").unwrap_or(0.0);
        voucher_snapshot = format!("{}###{}###{}", vcode, voucher_discount, price_before_voucher);
    }

    // Enforce non-negative price (BUG-14)
    let total_price = (real_price - flash_discount - voucher_discount).max(0.0);

    // Provider check (best-effort; provider table may be empty in this schema)
    let _prov_row = sqlx::query("SELECT * FROM provider WHERE code = ? LIMIT 1")
        .bind(&srv_provider)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);

    // Validation
    if req.target.trim().is_empty() || req.target.trim() == "," {
        return Ok(Json(json!({ "result": false, "message": "Data tujuan tidak boleh kosong." })));
    }
    if req.whaphone.trim().is_empty() {
        return Ok(Json(json!({ "result": false, "message": "No. WhatsApp tidak boleh kosong." })));
    }

    // Build order id
    let order_id = format!("ORD{}{}", chrono::Utc::now().format("%Y%m%d%H%M%S"), rand::random::<u16>());

    sqlx::query(
        "INSERT INTO transaction (order_id, order_tid, user, code, service_name, game, target, price, profit, status, payment_status, provider, created_at, updated_at) VALUES (?, '', ?, ?, ?, ?, ?, ?, ?, 'pending', 'unpaid', ?, NOW(), NOW())"
    )
    .bind(&order_id)
    .bind(&lann_user)
    .bind(&srv_code)
    .bind(&srv_name)
    .bind(&srv_game)
    .bind(&req.target)
    .bind(total_price)
    .bind(profit)
    .bind(&srv_provider)
    .execute(&state.db)
    .await?;

    // Optional snapshot columns (voucher/nickname/phone) — ignored when absent.
    // CATATAN: stok voucher TIDAK dipotong di sini — otoritas ada di Server Topup
    // (fulfillment memotong stok saat order LUNAS, dari DB-nya sendiri).
    if !voucher_snapshot.is_empty() {
        let _ = sqlx::query("UPDATE transaction SET voucher = ? WHERE order_id = ?")
            .bind(&voucher_snapshot)
            .bind(&order_id)
            .execute(&state.db)
            .await;
    }
    let _ = sqlx::query("UPDATE transaction SET nickname = ?, phone = ? WHERE order_id = ?")
        .bind(&req.nickname)
        .bind(&req.whaphone)
        .bind(&order_id)
        .execute(&state.db)
        .await;

    // Simpan metode pembayaran pilihan user — dipakai invoice/create mode AUTO
    // setelah order di-ACCEPT fulfillment.
    let _ = sqlx::query("UPDATE transaction SET payment_code = ? WHERE order_id = ?")
        .bind(&req.method)
        .bind(&order_id)
        .execute(&state.db)
        .await;

    // Broadcast NEW_ORDER to Topup Server via Encrypted MPSC Queue.
    // Harga final TETAP dihitung ulang oleh fulfillment dari DB-nya sendiri;
    // price & name di sini hanya fallback, voucher diteruskan agar validasi otoritatif.
    let voucher_token = req.voucher.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("-");
    let msg_text = format!(
        "[NEW_ORDER] {} {} {} {} {} {} {} {}",
        order_id, lann_user, srv_code, req.target, srv_provider, total_price, srv_name.replace(' ', "_"), voucher_token
    );
    let _ = rust_backend::domain::telegram::sender::send_report_to_fulfillment(&msg_text).await;

    let _ = (srv_reseller,);

    Ok(Json(json!({
        "result": true,
        "order_id": order_id,
        "total": total_price,
        "message": "Pesanan berhasil dibuat"
    })))
}

async fn handle_payment_methods(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    use sqlx::Row;

    // Kueri dinamis langsung dari tabel metode di database MySQL lokal
    let rows = sqlx::query("SELECT * FROM metode WHERE status = 'on' ORDER BY id ASC")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let list: Vec<serde_json::Value> = rows.iter().map(|r| {
        let code = r.try_get::<String, _>("code").unwrap_or_default();
        let name = r.try_get::<String, _>("name").unwrap_or_default();
        let mtype = r.try_get::<String, _>("type").unwrap_or_default();
        let min_val = r.try_get::<f64, _>("min").unwrap_or(1000.0);
        let max_val = r.try_get::<f64, _>("max").unwrap_or(10000000.0);
        let fee_using = r.try_get::<String, _>("fee_using").unwrap_or_else(|_| "merchant".to_string());
        let fee_type = r.try_get::<String, _>("fee_type").unwrap_or_else(|_| "+".to_string());
        let percent = r.try_get::<f64, _>("percent").unwrap_or(0.0);
        let flat = r.try_get::<f64, _>("flat").unwrap_or(0.0);
        let image = r.try_get::<String, _>("image").unwrap_or_default();
        let status = r.try_get::<String, _>("status").unwrap_or_else(|_| "on".to_string());
        let provider = r.try_get::<String, _>("provider").unwrap_or_else(|_| "TRIPAY".to_string());
        let is_qris = code.to_lowercase().contains("qris") || mtype.to_lowercase().contains("saldo");
        
        json!({
            "code": code,
            "name": name,
            "type": mtype,
            "min": min_val,
            "max": max_val,
            "fee_using": fee_using,
            "fee_type": fee_type,
            "percent": percent,
            "flat": flat,
            "image": image,
            "status": status,
            "provider": provider,
            "best_price": is_qris
        })
    }).collect();

    Ok(Json(json!({ "result": true, "data": list })))
}

async fn handle_invoice_detail(
    State(state): State<AppState>,
    axum::extract::Path(oid): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    use sqlx::Row;

    let trx_row = sqlx::query("SELECT * FROM transaction WHERE order_id = ? LIMIT 1")
        .bind(&oid)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::ServiceNotFound)?;

    let category_name: String = trx_row.try_get("game").unwrap_or_default();
    let price: f64 = trx_row.try_get("price").unwrap_or_default();
    let code: String = trx_row.try_get("code").unwrap_or_default();
    let provider: String = trx_row.try_get("provider").unwrap_or_default();
    let note: String = trx_row.try_get("note").unwrap_or_default();
    let status: String = trx_row.try_get("status").unwrap_or_default();
    let payment_status: String = trx_row.try_get("payment_status").unwrap_or_default();
    let created_at: Option<chrono::NaiveDateTime> = trx_row.try_get("created_at").unwrap_or(None);
    let updated_at: Option<chrono::NaiveDateTime> = trx_row.try_get("updated_at").unwrap_or(None);

    let mut category_image = "/library/assets/guest/images/game/default-image.png".to_string();
    if !category_name.is_empty() {
        let cat_row = sqlx::query("SELECT image FROM category WHERE name = ? LIMIT 1")
            .bind(&category_name)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);
        if let Some(cr) = cat_row {
            category_image = extract_image_url(&cr.try_get::<String, _>("image").unwrap_or_default());
        }
    }

    // Determine method name and payment action
    let is_qris = provider == "QR_LANN" || code.contains("QRIS") || note.to_uppercase().contains("QRIS") || provider == "TOKOPAY";
    let (metode_name, payment_type, payment_action, payment_guide) = if is_qris {
        (
            "QRIS (Semua Pembayaran)".to_string(),
            "QRIS".to_string(),
            format!("https://api.qrserver.com/v1/create-qr-code/?size=250x250&data={}", oid),
            "Buka aplikasi E-Wallet / Mobile Banking, pilih Scan QRIS, lalu lakukan pembayaran sesuai total tagihan.".to_string(),
        )
    } else if provider == "APP" || note.contains("Saldo") || note.starts_with("OTP:") {
        (
            "Saldo Akun".to_string(),
            "Saldo".to_string(),
            "Potong Saldo Akun".to_string(),
            "Pembayaran menggunakan saldo akun member.".to_string(),
        )
    } else {
        (
            format!("Payment Gateway ({})", if provider.is_empty() { "Automated" } else { &provider }),
            "Gateway".to_string(),
            "".to_string(),
            "Selesaikan pembayaran sesuai instruksi invoice.".to_string(),
        )
    };

    Ok(Json(json!({
        "result": true,
        "data": {
            "order_id": oid,
            "category": category_name,
            "service_name": trx_row.try_get::<String,_>("service_name").unwrap_or_default(),
            "service_code": code,
            "target": trx_row.try_get::<String,_>("target").unwrap_or_default(),
            "user": trx_row.try_get::<String,_>("user").unwrap_or_default(),
            "data": trx_row.try_get::<String,_>("target").unwrap_or_default(),
            "nickname": "",
            "metode": metode_name,
            "payment_action": payment_action,
            "payment_type": payment_type,
            "payment_provider": provider,
            "payment_guide": payment_guide,
            "trxfrom": "WEB",
            "price": price,
            "fee": 0.0,
            "uniq": 0.0,
            "total": price,
            "status": status,
            "status_payment": payment_status,
            "provider": provider,
            "created_at": created_at.map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_default(),
            "updated_at": updated_at.map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_default(),
            "category_image": category_image,
            "has_review": false
        }
    })))
}

async fn handle_blog(
    State(state): State<AppState>,
    axum::extract::Path(slug): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    use sqlx::Row;
    let title = slug.replace("-", " ");
    let row = sqlx::query("SELECT * FROM blog WHERE title = ? LIMIT 1")
        .bind(&title)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::ServiceNotFound)?;

    let content_raw: String = row.try_get("content").unwrap_or_default();

    Ok(Json(json!({
        "result": true,
        "data": {
            "title": row.try_get::<String,_>("title").unwrap_or_default(),
            "banner": row.try_get::<String,_>("banner").unwrap_or_default(),
            "content": content_raw,
            "content_html": try_decode_base64(&content_raw),
        }
    })))
}

fn try_decode_base64(s: &str) -> String {
    use base64::Engine;
    match base64::engine::general_purpose::STANDARD.decode(s) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
        Err(_) => s.to_string(),
    }
}

async fn handle_leaderboard(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let today_start = format!("{} 00:00:00", today);
    let today_end = format!("{} 23:59:59", today);
    let week_ago = chrono::Local::now().checked_sub_signed(chrono::Duration::days(7)).unwrap_or_else(chrono::Local::now).format("%Y-%m-%d 00:00:00").to_string();
    let month_ago = chrono::Local::now().checked_sub_signed(chrono::Duration::days(30)).unwrap_or_else(chrono::Local::now).format("%Y-%m-%d 00:00:00").to_string();
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

    async fn board(db: &sqlx::MySqlPool, from: &str, to: &str, limit: i32) -> Vec<serde_json::Value> {
        use sqlx::Row;
        let rows = sqlx::query(
            "SELECT CASE WHEN t.user IS NULL OR t.user = '' OR t.user = '-' THEN 'Anonim' ELSE t.user END as user, COUNT(*) as transactions, SUM(t.price) as total_spent FROM transaction t WHERE t.status = 'success' AND t.date_cr >= ? AND t.date_cr <= ? GROUP BY t.user ORDER BY total_spent DESC LIMIT ?",
        )
        .bind(from)
        .bind(to)
        .bind(limit)
        .fetch_all(db)
        .await
        .unwrap_or_default();

        rows.into_iter()
            .map(|r| {
                json!({
                    "user": r.try_get::<String,_>("user").unwrap_or_else(|_| "Anonim".to_string()),
                    "transactions": r.try_get::<i64,_>("transactions").unwrap_or_default(),
                    "total_spent": r.try_get::<f64,_>("total_spent").unwrap_or_default(),
                })
            })
            .collect()
    }

    let daily = board(&state.db, &today_start, &today_end, 3).await;
    let weekly = board(&state.db, &week_ago, &now, 5).await;
    let monthly = board(&state.db, &month_ago, &now, 10).await;

    Ok(Json(json!({
        "result": true,
        "data": { "daily": daily, "weekly": weekly, "monthly": monthly }
    })))
}

async fn handle_pricelist(
    State(state): State<AppState>,
    axum::extract::Path(code): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    use sqlx::Row;
    let code = code.replace("-", " ");

    let rows = sqlx::query("SELECT * FROM service WHERE game = ? ORDER BY price ASC")
        .bind(&code)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let mut data = Vec::new();
    for r in rows {
        let srv_code: String = r.try_get("code").unwrap_or_default();
        let flash_row = sqlx::query("SELECT amount FROM flashsale WHERE code = ? AND status = '1' LIMIT 1")
            .bind(&srv_code)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);
        let discount: f64 = flash_row
            .as_ref()
            .and_then(|fr| fr.try_get("amount").ok())
            .unwrap_or(0.0);

        let price: f64 = r.try_get("price").unwrap_or_default();
        let member: f64 = r.try_get("member").unwrap_or_default();
        let reseller: f64 = r.try_get("reseller").unwrap_or_default();

        data.push(json!({
            "code": srv_code,
            "name": r.try_get::<String,_>("name").unwrap_or_default(),
            "price_tamu": price - discount,
            "price_member": price + member - discount,
            "price_reseller": price + reseller - discount,
            "status": r.try_get::<String,_>("status").unwrap_or_default(),
        }));
    }

    Ok(Json(json!({ "result": true, "data": data })))
}

async fn handle_admin_config_website_get(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    let data = rust_backend::domain::admin::config::get_website_config(&state.db).await?;
    Ok(Json(json!({ "result": true, "data": data })))
}

async fn handle_admin_config_website_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<rust_backend::domain::admin::config::AdminConfigWebsiteRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    rust_backend::domain::admin::config::update_website_config(&state.db, payload).await?;
    Ok(Json(json!({ "result": true, "message": "Konfigurasi website berhasil diperbarui" })))
}

async fn handle_admin_update_transaction_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<AdminUpdateTransactionStatusRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    update_transaction_status(&state.db, payload).await?;
    Ok(Json(json!({ "result": true, "message": "Status transaksi berhasil diperbarui" })))
}

async fn handle_admin_update_payment_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<AdminUpdatePaymentStatusRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    update_payment_status(&state.db, payload).await?;
    Ok(Json(json!({ "result": true, "message": "Status pembayaran berhasil diperbarui" })))
}

async fn handle_admin_update_transaction_note(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<AdminUpdateTransactionNoteRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    update_transaction_note(&state.db, payload).await?;
    Ok(Json(json!({ "result": true, "message": "Catatan transaksi berhasil diperbarui" })))
}

// ============================================================
// ADMIN EXTENDED API (parity with PHP admin panel)
// ============================================================

#[derive(Deserialize)]
pub struct AdminServiceUpdateRequest {
    pub id: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub price: f64,
    #[serde(default)]
    pub member: f64,
    #[serde(default)]
    pub reseller: f64,
    #[serde(default)]
    pub status: String,
}

async fn handle_admin_update_service(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<AdminServiceUpdateRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    sqlx::query("UPDATE service SET name = ?, price = ?, member = ?, reseller = ?, status = ?, date_up = NOW() WHERE id = ?")
        .bind(&p.name).bind(p.price).bind(p.member).bind(p.reseller).bind(&p.status).bind(p.id)
        .execute(&state.db).await?;
    Ok(Json(json!({ "result": true, "message": "Layanan berhasil diperbarui" })))
}

#[derive(Deserialize)]
pub struct AdminIdRequest { pub id: i64 }

async fn handle_admin_delete_service(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<AdminIdRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    sqlx::query("DELETE FROM service WHERE id = ?").bind(p.id).execute(&state.db).await?;
    Ok(Json(json!({ "result": true, "message": "Layanan berhasil dihapus" })))
}

#[derive(Deserialize)]
pub struct AdminUnlockRequest { pub username: String }

async fn handle_admin_unlock_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<AdminUnlockRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    sqlx::query("DELETE FROM users_lock WHERE user = ?").bind(&p.username).execute(&state.db).await?;
    Ok(Json(json!({ "result": true, "message": "Akun berhasil dibuka" })))
}

#[derive(Deserialize)]
pub struct AdminCreateUserRequest {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub phone: String,
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub balance: f64,
}

async fn handle_admin_create_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<AdminCreateUserRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    let exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE username = ? LIMIT 1")
        .bind(&p.username).fetch_optional(&state.db).await?;
    if exists.is_some() {
        return Ok(Json(json!({ "result": false, "message": "Username sudah terdaftar" })));
    }
    let hashed = bcrypt::hash(&p.password, bcrypt::DEFAULT_COST).map_err(|e| AppError::InternalError(e.to_string()))?;
    let level = if p.level.is_empty() { "Member".to_string() } else { p.level };
    sqlx::query("INSERT INTO users (username, password, name, email, phone, balance, level, date_cr, date_up) VALUES (?, ?, ?, ?, ?, ?, ?, NOW(), NOW())")
        .bind(&p.username).bind(&hashed).bind(&p.name).bind(&p.email).bind(&p.phone)
        .bind(p.balance).bind(&level)
        .execute(&state.db).await?;
    let uid = format!("UID{}", chrono::Utc::now().timestamp_millis());
    let ukey = format!("KEY{}", rand::random::<u32>());
    let _ = sqlx::query("INSERT INTO users_api (user, uid, ukey, whitelist, callback) VALUES (?, ?, ?, '*', '')")
        .bind(&p.username).bind(&uid).bind(&ukey).execute(&state.db).await;
    Ok(Json(json!({ "result": true, "message": "User berhasil ditambahkan" })))
}

#[derive(Deserialize)]
pub struct AdminUpdateUserRequest {
    pub username: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
}

async fn handle_admin_update_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<AdminUpdateUserRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    if let Some(v) = &p.name { let _ = sqlx::query("UPDATE users SET name = ? WHERE username = ?").bind(v).bind(&p.username).execute(&state.db).await; }
    if let Some(v) = &p.email { let _ = sqlx::query("UPDATE users SET email = ? WHERE username = ?").bind(v).bind(&p.username).execute(&state.db).await; }
    if let Some(v) = &p.phone { let _ = sqlx::query("UPDATE users SET phone = ? WHERE username = ?").bind(v).bind(&p.username).execute(&state.db).await; }
    if let Some(v) = &p.level { let _ = sqlx::query("UPDATE users SET level = ? WHERE username = ?").bind(v).bind(&p.username).execute(&state.db).await; }
    if let Some(v) = &p.status { let _ = sqlx::query("UPDATE users SET status = ? WHERE username = ?").bind(v).bind(&p.username).execute(&state.db).await; }
    if let Some(v) = &p.password {
        if !v.is_empty() {
            let hashed = bcrypt::hash(v, bcrypt::DEFAULT_COST).map_err(|e| AppError::InternalError(e.to_string()))?;
            let _ = sqlx::query("UPDATE users SET password = ? WHERE username = ?").bind(&hashed).bind(&p.username).execute(&state.db).await;
        }
    }
    Ok(Json(json!({ "result": true, "message": "User berhasil diperbarui" })))
}

async fn handle_admin_locked_users(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    use sqlx::Row;
    let rows = sqlx::query("SELECT user, reason, date_cr FROM users_lock ORDER BY id DESC LIMIT 200")
        .fetch_all(&state.db).await.unwrap_or_default();
    let data: Vec<serde_json::Value> = rows.into_iter().map(|r| json!({
        "user": r.try_get::<String, _>("user").unwrap_or_default(),
        "reason": r.try_get::<String, _>("reason").unwrap_or_default(),
        "date_cr": r.try_get::<String, _>("date_cr").unwrap_or_default(),
    })).collect();
    Ok(Json(json!({ "result": true, "data": data })))
}

async fn handle_admin_mutations(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    use sqlx::Row;
    let rows = sqlx::query("SELECT username, type, amount, note, date_cr FROM mutation ORDER BY id DESC LIMIT 200")
        .fetch_all(&state.db).await.unwrap_or_default();
    let data: Vec<serde_json::Value> = rows.into_iter().map(|r| json!({
        "username": r.try_get::<String, _>("username").unwrap_or_default(),
        "type": r.try_get::<String, _>("type").unwrap_or_else(|_| r.try_get::<String, _>("type_name").unwrap_or_default()),
        "amount": r.try_get::<f64, _>("amount").unwrap_or_default(),
        "note": r.try_get::<String, _>("note").unwrap_or_default(),
        "date_cr": r.try_get::<String, _>("date_cr").unwrap_or_default(),
    })).collect();
    Ok(Json(json!({ "result": true, "data": data })))
}

async fn handle_admin_deposits(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    use sqlx::Row;
    let rows = sqlx::query("SELECT id, deposit_id, username, method, name, amount, fee, balance, status, provider, date FROM deposit ORDER BY id DESC LIMIT 200")
        .fetch_all(&state.db).await.unwrap_or_default();
    let data: Vec<serde_json::Value> = rows.into_iter().map(|r| json!({
        "id": r.try_get::<i64, _>("id").unwrap_or_default(),
        "deposit_id": r.try_get::<String, _>("deposit_id").unwrap_or_default(),
        "username": r.try_get::<String, _>("username").unwrap_or_default(),
        "method": r.try_get::<String, _>("method").unwrap_or_default(),
        "name": r.try_get::<String, _>("name").unwrap_or_default(),
        "amount": r.try_get::<f64, _>("amount").unwrap_or_default(),
        "fee": r.try_get::<f64, _>("fee").unwrap_or(0.0),
        "uniq": r.try_get::<f64, _>("balance").unwrap_or(0.0),
        "status": r.try_get::<String, _>("status").unwrap_or_default(),
        "provider": r.try_get::<String, _>("provider").unwrap_or_default(),
        "date": r.try_get::<String, _>("date").unwrap_or_default(),
    })).collect();
    Ok(Json(json!({ "result": true, "data": data })))
}

#[derive(Deserialize)]
pub struct AdminDepositStatusRequest { pub deposit_id: String, pub status: String }

async fn handle_admin_deposit_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<AdminDepositStatusRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    use sqlx::Row;
    // Guarded update: only transitions from a non-paid state can credit balance.
    let res = sqlx::query("UPDATE deposit SET status = ? WHERE deposit_id = ? AND status <> ?")
        .bind(&p.status).bind(&p.deposit_id).bind(&p.status)
        .execute(&state.db).await?;
    if p.status == "paid" && res.rows_affected() > 0 {
        if let Ok(Some(row)) = sqlx::query("SELECT username, amount, balance FROM deposit WHERE deposit_id = ?")
            .bind(&p.deposit_id).fetch_optional(&state.db).await
        {
            let username: String = row.try_get("username").unwrap_or_default();
            let amount: f64 = row.try_get("amount").unwrap_or(0.0);
            let uniq: f64 = row.try_get("balance").unwrap_or(0.0);
            let credit = amount + uniq;
            if credit > 0.0 && !username.is_empty() {
                let _ = sqlx::query("UPDATE users SET balance = balance + ? WHERE username = ?")
                    .bind(credit).bind(&username).execute(&state.db).await;
                let _ = sqlx::query("INSERT INTO mutation (username, amount, type, note, date_cr) VALUES (?, ?, '+', ?, NOW())")
                    .bind(&username).bind(credit).bind(format!("Deposit disetujui manual: {}", p.deposit_id))
                    .execute(&state.db).await;
            }
        }
    }
    Ok(Json(json!({ "result": true, "message": "Status deposit diperbarui" })))
}

// ---------- Vouchers ----------

async fn handle_admin_vouchers(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    use sqlx::Row;
    let rows = sqlx::query("SELECT id, code, voucher, discount, minimum, stock, date_cr FROM voucher ORDER BY id DESC LIMIT 200")
        .fetch_all(&state.db).await.unwrap_or_default();
    let data: Vec<serde_json::Value> = rows.into_iter().map(|r| json!({
        "id": r.try_get::<i64, _>("id").unwrap_or_default(),
        "category": r.try_get::<String, _>("code").unwrap_or_default(),
        "code": r.try_get::<String, _>("voucher").unwrap_or_default(),
        "discount": r.try_get::<f64, _>("discount").unwrap_or_default(),
        "minimum": r.try_get::<f64, _>("minimum").unwrap_or_default(),
        "stock": r.try_get::<i64, _>("stock").unwrap_or_default(),
        "date_cr": r.try_get::<String, _>("date_cr").unwrap_or_default(),
    })).collect();
    Ok(Json(json!({ "result": true, "data": data })))
}

#[derive(Deserialize)]
pub struct AdminVoucherCreateRequest {
    pub category: String,
    pub code: String,
    pub discount: f64,
    #[serde(default)]
    pub minimum: f64,
    #[serde(default)]
    pub stock: i64,
}

async fn handle_admin_voucher_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<AdminVoucherCreateRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    let res = sqlx::query("INSERT INTO voucher (code, voucher, discount, minimum, stock, date_cr) VALUES (?, ?, ?, ?, ?, NOW())")
        .bind(&p.category).bind(&p.code).bind(p.discount).bind(p.minimum).bind(p.stock)
        .execute(&state.db).await;
    match res {
        Ok(_) => Ok(Json(json!({ "result": true, "message": "Voucher berhasil dibuat" }))),
        Err(_) => Ok(Json(json!({ "result": false, "message": "Tabel voucher belum tersedia di database web" }))),
    }
}

async fn handle_admin_voucher_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<AdminIdRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    sqlx::query("DELETE FROM voucher WHERE id = ?").bind(p.id).execute(&state.db).await?;
    Ok(Json(json!({ "result": true, "message": "Voucher dihapus" })))
}

// ---------- Flashsale ----------

async fn handle_admin_flashsale(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    use sqlx::Row;
    let rows = sqlx::query(
        "SELECT f.id, f.code, f.amount, f.status, COALESCE(s.name, '') AS service_name FROM flashsale f LEFT JOIN service s ON s.code = f.code ORDER BY f.id DESC LIMIT 200",
    ).fetch_all(&state.db).await.unwrap_or_default();
    let data: Vec<serde_json::Value> = rows.into_iter().map(|r| json!({
        "id": r.try_get::<i64, _>("id").unwrap_or_default(),
        "code": r.try_get::<String, _>("code").unwrap_or_default(),
        "service_name": r.try_get::<String, _>("service_name").unwrap_or_default(),
        "amount": r.try_get::<f64, _>("amount").unwrap_or_default(),
        "status": r.try_get::<String, _>("status").unwrap_or_default(),
    })).collect();
    Ok(Json(json!({ "result": true, "data": data })))
}

#[derive(Deserialize)]
pub struct AdminFlashsaleCreateRequest { pub code: String, pub amount: f64 }

async fn handle_admin_flashsale_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<AdminFlashsaleCreateRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    sqlx::query("INSERT INTO flashsale (code, amount, status) VALUES (?, ?, '1')")
        .bind(&p.code).bind(p.amount).execute(&state.db).await?;
    Ok(Json(json!({ "result": true, "message": "Flashsale ditambahkan" })))
}

async fn handle_admin_flashsale_toggle(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<AdminIdRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    sqlx::query("UPDATE flashsale SET status = IF(status = '1', '0', '1') WHERE id = ?")
        .bind(p.id).execute(&state.db).await?;
    Ok(Json(json!({ "result": true, "message": "Status flashsale diubah" })))
}

async fn handle_admin_flashsale_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<AdminIdRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    sqlx::query("DELETE FROM flashsale WHERE id = ?").bind(p.id).execute(&state.db).await?;
    Ok(Json(json!({ "result": true, "message": "Flashsale dihapus" })))
}

// ---------- Banners ----------

async fn handle_admin_banners(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    use sqlx::Row;
    let rows = sqlx::query("SELECT id, content FROM banner ORDER BY id DESC LIMIT 100")
        .fetch_all(&state.db).await.unwrap_or_default();
    let data: Vec<serde_json::Value> = rows.into_iter().map(|r| json!({
        "id": r.try_get::<i64, _>("id").unwrap_or_default(),
        "content": r.try_get::<String, _>("content").unwrap_or_default(),
    })).collect();
    Ok(Json(json!({ "result": true, "data": data })))
}

#[derive(Deserialize)]
pub struct AdminBannerCreateRequest { pub content: String }

async fn handle_admin_banner_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<AdminBannerCreateRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    sqlx::query("INSERT INTO banner (content) VALUES (?)").bind(&p.content).execute(&state.db).await?;
    Ok(Json(json!({ "result": true, "message": "Banner ditambahkan" })))
}

async fn handle_admin_banner_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<AdminIdRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    sqlx::query("DELETE FROM banner WHERE id = ?").bind(p.id).execute(&state.db).await?;
    Ok(Json(json!({ "result": true, "message": "Banner dihapus" })))
}

// ---------- Blog ----------

async fn handle_admin_blog_list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    use sqlx::Row;
    let rows = sqlx::query("SELECT * FROM blog ORDER BY id DESC LIMIT 100")
        .fetch_all(&state.db).await.unwrap_or_default();
    let data: Vec<serde_json::Value> = rows.into_iter().map(|r| {
        let content_raw: String = r.try_get("content").unwrap_or_default();
        json!({
            "id": r.try_get::<i64, _>("id").unwrap_or_default(),
            "title": r.try_get::<String, _>("title").unwrap_or_default(),
            "banner": r.try_get::<String, _>("banner").unwrap_or_default(),
            "content": try_decode_base64(&content_raw),
        })
    }).collect();
    Ok(Json(json!({ "result": true, "data": data })))
}

#[derive(Deserialize)]
pub struct AdminBlogSaveRequest {
    #[serde(default)]
    pub id: Option<i64>,
    pub title: String,
    #[serde(default)]
    pub banner: String,
    pub content: String,
}

async fn handle_admin_blog_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<AdminBlogSaveRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(p.content.as_bytes());
    if let Some(id) = p.id {
        sqlx::query("UPDATE blog SET title = ?, banner = ?, content = ? WHERE id = ?")
            .bind(&p.title).bind(&p.banner).bind(&encoded).bind(id).execute(&state.db).await?;
        Ok(Json(json!({ "result": true, "message": "Artikel diperbarui" })))
    } else {
        sqlx::query("INSERT INTO blog (title, banner, content) VALUES (?, ?, ?)")
            .bind(&p.title).bind(&p.banner).bind(&encoded).execute(&state.db).await?;
        Ok(Json(json!({ "result": true, "message": "Artikel ditambahkan" })))
    }
}

async fn handle_admin_blog_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<AdminIdRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    sqlx::query("DELETE FROM blog WHERE id = ?").bind(p.id).execute(&state.db).await?;
    Ok(Json(json!({ "result": true, "message": "Artikel dihapus" })))
}

// ---------- Providers ----------

async fn handle_admin_providers(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    use sqlx::Row;
    let rows = sqlx::query("SELECT * FROM provider ORDER BY id ASC")
        .fetch_all(&state.db).await.unwrap_or_default();
    let data: Vec<serde_json::Value> = rows.into_iter().map(|r| json!({
        "id": r.try_get::<i64, _>("id").unwrap_or_default(),
        "code": r.try_get::<String, _>("code").unwrap_or_default(),
        "name": r.try_get::<String, _>("name").unwrap_or_default(),
        "userid": r.try_get::<String, _>("userid").unwrap_or_default(),
        "apikey": r.try_get::<String, _>("apikey").unwrap_or_default(),
        "merchant": r.try_get::<String, _>("merchant").unwrap_or_default(),
        "link": r.try_get::<String, _>("link").unwrap_or_default(),
        "mode": r.try_get::<String, _>("mode").unwrap_or_default(),
    })).collect();
    Ok(Json(json!({ "result": true, "data": data })))
}

#[derive(Deserialize)]
pub struct AdminProviderEditRequest {
    pub code: String,
    #[serde(default)]
    pub userid: Option<String>,
    #[serde(default)]
    pub apikey: Option<String>,
    #[serde(default)]
    pub merchant: Option<String>,
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
}

async fn handle_admin_provider_edit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<AdminProviderEditRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    if let Some(v) = &p.userid { let _ = sqlx::query("UPDATE provider SET userid = ? WHERE code = ?").bind(v).bind(&p.code).execute(&state.db).await; }
    if let Some(v) = &p.apikey { let _ = sqlx::query("UPDATE provider SET apikey = ? WHERE code = ?").bind(v).bind(&p.code).execute(&state.db).await; }
    if let Some(v) = &p.merchant { let _ = sqlx::query("UPDATE provider SET merchant = ? WHERE code = ?").bind(v).bind(&p.code).execute(&state.db).await; }
    if let Some(v) = &p.link { let _ = sqlx::query("UPDATE provider SET link = ? WHERE code = ?").bind(v).bind(&p.code).execute(&state.db).await; }
    if let Some(v) = &p.mode { let _ = sqlx::query("UPDATE provider SET mode = ? WHERE code = ?").bind(v).bind(&p.code).execute(&state.db).await; }
    Ok(Json(json!({ "result": true, "message": "Provider diperbarui" })))
}

// ---------- Categories & Tabs ----------

async fn handle_admin_categories(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    use sqlx::Row;
    let rows = sqlx::query("SELECT * FROM category ORDER BY name ASC")
        .fetch_all(&state.db).await.unwrap_or_default();
    let data: Vec<serde_json::Value> = rows.into_iter().map(|r| {
        let df: String = r.try_get("data_form").unwrap_or_default();
        json!({
            "id": r.try_get::<i64, _>("id").unwrap_or_default(),
            "name": r.try_get::<String, _>("name").unwrap_or_default(),
            "owner": r.try_get::<String, _>("owner").unwrap_or_default(),
            "type": r.try_get::<String, _>("type").unwrap_or_default(),
            "image": r.try_get::<String, _>("image").unwrap_or_default(),
            "popular": r.try_get::<String, _>("popular").unwrap_or_default(),
            "prefix": r.try_get::<String, _>("prefix").unwrap_or_default(),
            "data_form": serde_json::from_str::<serde_json::Value>(&df).unwrap_or(json!([])),
            "note": r.try_get::<String, _>("note").unwrap_or_default(),
            "target_note": r.try_get::<String, _>("target_note").unwrap_or_default(),
            "apk": r.try_get::<String, _>("apk").unwrap_or_default(),
        })
    }).collect();
    Ok(Json(json!({ "result": true, "data": data })))
}

#[derive(Deserialize)]
pub struct AdminCategorySaveRequest {
    #[serde(default)]
    pub id: Option<i64>,
    pub name: String,
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub category_type: String,
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub popular: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub data_form: Option<serde_json::Value>,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub target_note: String,
    #[serde(default)]
    pub apk: String,
}

async fn handle_admin_category_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<AdminCategorySaveRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    let df = p.data_form.clone().map(|v| v.to_string()).unwrap_or_default();
    if let Some(id) = p.id {
        sqlx::query("UPDATE category SET name = ?, owner = ?, type = ?, image = ?, popular = ?, prefix = ?, data_form = ?, note = ?, target_note = ?, apk = ? WHERE id = ?")
            .bind(&p.name).bind(&p.owner).bind(&p.category_type).bind(&p.image).bind(&p.popular)
            .bind(&p.prefix).bind(&df).bind(&p.note).bind(&p.target_note).bind(&p.apk).bind(id)
            .execute(&state.db).await?;
        Ok(Json(json!({ "result": true, "message": "Kategori diperbarui" })))
    } else {
        sqlx::query("INSERT INTO category (name, owner, type, image, popular, prefix, data_form, note, target_note, apk) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&p.name).bind(&p.owner).bind(&p.category_type).bind(&p.image).bind(&p.popular)
            .bind(&p.prefix).bind(&df).bind(&p.note).bind(&p.target_note).bind(&p.apk)
            .execute(&state.db).await?;
        Ok(Json(json!({ "result": true, "message": "Kategori ditambahkan" })))
    }
}

async fn handle_admin_category_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<AdminIdRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM category WHERE id = ?")
        .bind(p.id).fetch_optional(&state.db).await.unwrap_or(None);
    sqlx::query("DELETE FROM category WHERE id = ?").bind(p.id).execute(&state.db).await?;
    if let Some(n) = name {
        let _ = sqlx::query("DELETE FROM service WHERE game = ?").bind(&n).execute(&state.db).await;
    }
    Ok(Json(json!({ "result": true, "message": "Kategori dihapus beserta layanannya" })))
}

async fn handle_admin_tabs(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    use sqlx::Row;
    let rows = sqlx::query("SELECT id, name, icon, type, `order` FROM tabs ORDER BY `order` ASC")
        .fetch_all(&state.db).await.unwrap_or_default();
    let data: Vec<serde_json::Value> = rows.into_iter().map(|r| json!({
        "id": r.try_get::<i64, _>("id").unwrap_or_default(),
        "name": r.try_get::<String, _>("name").unwrap_or_default(),
        "icon": r.try_get::<String, _>("icon").unwrap_or_default(),
        "type": r.try_get::<String, _>("type").unwrap_or_default(),
        "order": r.try_get::<i64, _>("order").unwrap_or_default(),
    })).collect();
    Ok(Json(json!({ "result": true, "data": data })))
}

#[derive(Deserialize)]
pub struct AdminTabSaveRequest {
    #[serde(default)]
    pub id: Option<i64>,
    pub name: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub tab_type: String,
    #[serde(default)]
    pub order: i64,
}

async fn handle_admin_tab_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<AdminTabSaveRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    if let Some(id) = p.id {
        sqlx::query("UPDATE tabs SET name = ?, icon = ?, type = ?, `order` = ? WHERE id = ?")
            .bind(&p.name).bind(&p.icon).bind(&p.tab_type).bind(p.order).bind(id)
            .execute(&state.db).await?;
        Ok(Json(json!({ "result": true, "message": "Tab diperbarui" })))
    } else {
        sqlx::query("INSERT INTO tabs (name, icon, type, `order`) VALUES (?, ?, ?, ?)")
            .bind(&p.name).bind(&p.icon).bind(&p.tab_type).bind(p.order)
            .execute(&state.db).await?;
        Ok(Json(json!({ "result": true, "message": "Tab ditambahkan" })))
    }
}

async fn handle_admin_tab_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<AdminIdRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    sqlx::query("DELETE FROM tabs WHERE id = ?").bind(p.id).execute(&state.db).await?;
    Ok(Json(json!({ "result": true, "message": "Tab dihapus" })))
}

// ---------- Conf-based config (popup / social / colors / pages) ----------

async fn conf_upsert(db: &sqlx::MySqlPool, code: &str, values: [&str; 5]) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO conf (code, c1, c2, c3, c4, c5) VALUES (?, ?, ?, ?, ?, ?) ON DUPLICATE KEY UPDATE c1 = VALUES(c1), c2 = VALUES(c2), c3 = VALUES(c3), c4 = VALUES(c4), c5 = VALUES(c5)",
    )
    .bind(code).bind(values[0]).bind(values[1]).bind(values[2]).bind(values[3]).bind(values[4])
    .execute(db).await?;
    Ok(())
}

async fn handle_admin_conf_get(state: AppState, headers: HeaderMap, code: &'static str) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    let rows = sqlx::query(r#"SELECT code, c1, c2, c3, c4, c5 FROM conf"#).fetch_all(&state.db).await.unwrap_or_default();
    Ok(Json(json!({
        "c1": conf_cell(&rows, code, "c1"),
        "c2": conf_cell(&rows, code, "c2"),
        "c3": conf_cell(&rows, code, "c3"),
        "c4": conf_cell(&rows, code, "c4"),
        "c5": conf_cell(&rows, code, "c5"),
    })))
}

async fn handle_admin_conf_popup_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    let _ = require_admin(&state, &headers).await;
    let g = |k: &str| p.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    conf_upsert(&state.db, "popup", [&g("image"), &g("subtitle"), &g("content"), &g("show"), ""]).await?;
    Ok(Json(json!({ "result": true, "message": "Popup disimpan" })))
}

async fn handle_admin_conf_social_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    let _ = require_admin(&state, &headers).await;
    let g = |k: &str| p.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    conf_upsert(&state.db, "social", [&g("c1"), &g("c2"), &g("c3"), &g("c4"), &g("c5")]).await?;
    Ok(Json(json!({ "result": true, "message": "Social links disimpan" })))
}

async fn handle_admin_conf_colors_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    let _ = require_admin(&state, &headers).await;
    let g = |k: &str| p.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    conf_upsert(&state.db, "color", [&g("c1"), &g("c2"), &g("c3"), &g("c4"), ""]).await?;
    Ok(Json(json!({ "result": true, "message": "Warna tema disimpan" })))
}

async fn handle_admin_conf_pages_get(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    let rows = sqlx::query(r#"SELECT code, c1, c2 FROM conf"#).fetch_all(&state.db).await.unwrap_or_default();
    Ok(Json(json!({
        "terms": try_decode_base64(&conf_cell(&rows, "pages", "c1")),
        "privacy": try_decode_base64(&conf_cell(&rows, "pages", "c2")),
    })))
}

async fn handle_admin_conf_pages_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    let _ = require_admin(&state, &headers).await;
    use base64::Engine;
    let terms_b64 = base64::engine::general_purpose::STANDARD.encode(p.get("terms").and_then(|v| v.as_str()).unwrap_or("").as_bytes());
    let privacy_b64 = base64::engine::general_purpose::STANDARD.encode(p.get("privacy").and_then(|v| v.as_str()).unwrap_or("").as_bytes());
    conf_upsert(&state.db, "pages", [&terms_b64, &privacy_b64, "", "", ""]).await?;
    Ok(Json(json!({ "result": true, "message": "Halaman legal disimpan" })))
}

async fn handle_admin_conf_social_get(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    handle_admin_conf_get(state, headers, "social").await
}
async fn handle_admin_conf_colors_get(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    handle_admin_conf_get(state, headers, "color").await
}
async fn handle_admin_conf_popup_get_only(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    handle_admin_conf_get(state, headers, "popup").await
}

// ---------- Reports ----------

#[derive(Deserialize)]
pub struct ReportRangeQuery {
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub to: String,
}

fn report_range(q: &ReportRangeQuery) -> (String, String) {
    let to = if q.to.is_empty() { chrono::Local::now().format("%Y-%m-%d").to_string() } else { q.to.clone() };
    let from = if q.from.is_empty() { chrono::Local::now().checked_sub_signed(chrono::Duration::days(30)).unwrap_or_else(chrono::Local::now).format("%Y-%m-%d").to_string() } else { q.from.clone() };
    (from, to)
}

async fn handle_admin_report_sales(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ReportRangeQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    use sqlx::Row;
    let (from, to) = report_range(&q);
    let rows = sqlx::query(
        "SELECT DATE(created_at) AS d, COUNT(*) AS trx, COALESCE(SUM(price),0) AS income, COALESCE(SUM(profit),0) AS profit FROM transaction WHERE status = 'success' AND DATE(created_at) BETWEEN ? AND ? GROUP BY DATE(created_at) ORDER BY d ASC",
    ).bind(&from).bind(&to).fetch_all(&state.db).await.unwrap_or_default();
    let series: Vec<serde_json::Value> = rows.into_iter().map(|r| json!({
        "date": r.try_get::<chrono::NaiveDate, _>("d").map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default(),
        "trx": r.try_get::<i64, _>("trx").unwrap_or_default(),
        "income": r.try_get::<f64, _>("income").unwrap_or_default(),
        "profit": r.try_get::<f64, _>("profit").unwrap_or_default(),
    })).collect();
    Ok(Json(json!({ "result": true, "from": from, "to": to, "data": series })))
}

async fn handle_admin_report_profit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ReportRangeQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    use sqlx::Row;
    let (from, to) = report_range(&q);
    let rows = sqlx::query(
        "SELECT provider, COUNT(*) AS trx, COALESCE(SUM(price),0) AS gross, COALESCE(SUM(profit),0) AS profit FROM transaction WHERE status = 'success' AND DATE(created_at) BETWEEN ? AND ? GROUP BY provider ORDER BY profit DESC",
    ).bind(&from).bind(&to).fetch_all(&state.db).await.unwrap_or_default();
    let data: Vec<serde_json::Value> = rows.into_iter().map(|r| json!({
        "provider": r.try_get::<String, _>("provider").unwrap_or_default(),
        "trx": r.try_get::<i64, _>("trx").unwrap_or_default(),
        "gross": r.try_get::<f64, _>("gross").unwrap_or_default(),
        "profit": r.try_get::<f64, _>("profit").unwrap_or_default(),
    })).collect();
    Ok(Json(json!({ "result": true, "from": from, "to": to, "data": data })))
}

async fn handle_admin_report_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ReportRangeQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    use sqlx::Row;
    let (from, to) = report_range(&q);
    let by_status = sqlx::query(
        "SELECT status, COUNT(*) AS trx, COALESCE(SUM(price),0) AS total FROM transaction WHERE DATE(created_at) BETWEEN ? AND ? GROUP BY status",
    ).bind(&from).bind(&to).fetch_all(&state.db).await.unwrap_or_default();
    let by_user = sqlx::query(
        "SELECT user, COUNT(*) AS trx, COALESCE(SUM(price),0) AS total FROM transaction WHERE status = 'success' AND DATE(created_at) BETWEEN ? AND ? GROUP BY user ORDER BY total DESC LIMIT 10",
    ).bind(&from).bind(&to).fetch_all(&state.db).await.unwrap_or_default();
    let map_rows = |rows: Vec<sqlx::mysql::MySqlRow>, label: &str| -> Vec<serde_json::Value> {
        rows.into_iter().map(|r| json!({
            label: r.try_get::<String, _>(label).unwrap_or_default(),
            "trx": r.try_get::<i64, _>("trx").unwrap_or_default(),
            "total": r.try_get::<f64, _>("total").unwrap_or_default(),
        })).collect()
    };
    Ok(Json(json!({
        "result": true, "from": from, "to": to,
        "by_status": map_rows(by_status, "status"),
        "by_user": map_rows(by_user, "user"),
    })))
}

// ---------- Server tools ----------

async fn handle_admin_error_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    use sqlx::Row;
    let rows = sqlx::query("SELECT id, type, text, ip, date FROM logs WHERE user = 'system' ORDER BY id DESC LIMIT 200")
        .fetch_all(&state.db).await.unwrap_or_default();
    let data: Vec<serde_json::Value> = rows.into_iter().map(|r| json!({
        "id": r.try_get::<i64, _>("id").unwrap_or_default(),
        "type": r.try_get::<String, _>("type").unwrap_or_default(),
        "text": r.try_get::<String, _>("text").unwrap_or_default(),
        "ip": r.try_get::<String, _>("ip").unwrap_or_default(),
        "date": r.try_get::<String, _>("date").unwrap_or_default(),
    })).collect();
    Ok(Json(json!({ "result": true, "data": data })))
}

async fn handle_admin_dashboard_stats(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    async fn scalar(db: &sqlx::MySqlPool, q: &str) -> i64 {
        sqlx::query_scalar::<MySql, i64>(q).fetch_one(db).await.unwrap_or(0)
    }
    async fn fscalar(db: &sqlx::MySqlPool, q: &str) -> f64 {
        sqlx::query_scalar::<MySql, f64>(q).fetch_one(db).await.unwrap_or(0.0)
    }

    let total_users = scalar(&state.db, "SELECT COUNT(*) FROM users").await;
    let total_balance = fscalar(&state.db, "SELECT COALESCE(SUM(balance),0) FROM users").await;
    let total_deposit_amount = fscalar(&state.db, "SELECT COALESCE(SUM(amount),0) FROM deposit WHERE status = 'paid'").await;
    let total_trx = scalar(&state.db, "SELECT COUNT(*) FROM transaction").await;
    let success_trx = scalar(&state.db, "SELECT COUNT(*) FROM transaction WHERE status = 'success'").await;
    let pending_trx = scalar(&state.db, "SELECT COUNT(*) FROM transaction WHERE status IN ('pending','process')").await;
    let error_trx = scalar(&state.db, "SELECT COUNT(*) FROM transaction WHERE status IN ('error','system')").await;
    let revenue_total = fscalar(&state.db, "SELECT COALESCE(SUM(price),0) FROM transaction WHERE status = 'success'").await;
    let profit_total = fscalar(&state.db, "SELECT COALESCE(SUM(profit),0) FROM transaction WHERE status = 'success'").await;
    let revenue_today = fscalar(&state.db, "SELECT COALESCE(SUM(price),0) FROM transaction WHERE status = 'success' AND DATE(created_at) = CURDATE()").await;
    let revenue_month = fscalar(&state.db, "SELECT COALESCE(SUM(price),0) FROM transaction WHERE status = 'success' AND MONTH(created_at) = MONTH(CURDATE()) AND YEAR(created_at) = YEAR(CURDATE())").await;

    // 7-day chart series
    async fn daily_series(db: &sqlx::MySqlPool, sql: &str) -> Vec<serde_json::Value> {
        use sqlx::Row;
        let rows = sqlx::query(sql).fetch_all(db).await.unwrap_or_default();
        rows.into_iter().map(|r| json!({
            "date": r.try_get::<chrono::NaiveDate, _>("d").map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default(),
            "value": r.try_get::<i64, _>("c").unwrap_or_default(),
        })).collect()
    }
    let orders_series = daily_series(&state.db, "SELECT DATE(created_at) AS d, COUNT(*) AS c FROM transaction WHERE created_at >= DATE_SUB(CURDATE(), INTERVAL 7 DAY) GROUP BY d ORDER BY d").await;
    let deposits_series = daily_series(&state.db, "SELECT DATE(date) AS d, COUNT(*) AS c FROM deposit WHERE date >= DATE_SUB(CURDATE(), INTERVAL 7 DAY) GROUP BY d ORDER BY d").await;
    let users_series = daily_series(&state.db, "SELECT DATE(date_cr) AS d, COUNT(*) AS c FROM users WHERE date_cr >= DATE_SUB(CURDATE(), INTERVAL 7 DAY) GROUP BY d ORDER BY d").await;

    Ok(Json(json!({
        "result": true,
        "data": {
            "total_users": total_users,
            "total_balance": total_balance,
            "total_deposit_amount": total_deposit_amount,
            "total_trx": total_trx,
            "success_trx": success_trx,
            "pending_trx": pending_trx,
            "error_trx": error_trx,
            "revenue_total": revenue_total,
            "profit_total": profit_total,
            "revenue_today": revenue_today,
            "revenue_month": revenue_month,
            "orders_series": orders_series,
            "deposits_series": deposits_series,
            "users_series": users_series,
        }
    })))
}

// ---------- Manual orders (provider X) ----------

async fn handle_admin_manual_orders(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    use sqlx::Row;
    let rows = sqlx::query("SELECT order_id, user, service_name, target, price, status, payment_status, note, created_at FROM transaction WHERE provider = 'X' ORDER BY id DESC LIMIT 100")
        .fetch_all(&state.db).await.unwrap_or_default();
    let data: Vec<serde_json::Value> = rows.into_iter().map(|r| {
        let created: Option<chrono::NaiveDateTime> = r.try_get("created_at").unwrap_or(None);
        json!({
            "order_id": r.try_get::<String, _>("order_id").unwrap_or_default(),
            "user": r.try_get::<String, _>("user").unwrap_or_default(),
            "service_name": r.try_get::<String, _>("service_name").unwrap_or_default(),
            "target": r.try_get::<String, _>("target").unwrap_or_default(),
            "price": r.try_get::<f64, _>("price").unwrap_or_default(),
            "status": r.try_get::<String, _>("status").unwrap_or_default(),
            "payment_status": r.try_get::<String, _>("payment_status").unwrap_or_default(),
            "note": r.try_get::<String, _>("note").unwrap_or_default(),
            "created_at": created.map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_default(),
        })
    }).collect();
    Ok(Json(json!({ "result": true, "data": data })))
}

#[derive(Deserialize)]
pub struct AdminManualResendRequest { pub order_id: String }

async fn handle_admin_manual_resend(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(p): Json<AdminManualResendRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    use sqlx::Row;
    let row = sqlx::query("SELECT user, service_name, price, status FROM transaction WHERE order_id = ? LIMIT 1")
        .bind(&p.order_id).fetch_optional(&state.db).await?
        .ok_or(AppError::ServiceNotFound)?;
    let user: String = row.try_get("user").unwrap_or_default();
    let svc: String = row.try_get("service_name").unwrap_or_default();
    let price: f64 = row.try_get("price").unwrap_or_default();
    let status: String = row.try_get("status").unwrap_or_default();

    let phone: Option<String> = sqlx::query_scalar("SELECT phone FROM users WHERE username = ? LIMIT 1")
        .bind(&user).fetch_optional(&state.db).await.unwrap_or(None);

    let mut sent = false;
    if let Some(phone) = phone {
        if !phone.is_empty() {
            let api_key = std::env::var("MPWA_API_KEY").unwrap_or_default();
            let sender = std::env::var("MPWA_SENDER_PHONE").unwrap_or_default();
            if !api_key.is_empty() {
                let msg = format!("Update pesanan {}\nProduk: {}\nHarga: Rp{}\nStatus: {}", p.order_id, svc, price, status);
                sent = rust_backend::domain::auth::service::send_whatsapp_message(&api_key, &sender, &phone, &msg).await.unwrap_or(false);
            }
        }
    }
    if sent {
        Ok(Json(json!({ "result": true, "message": "Notifikasi WhatsApp terkirim" })))
    } else {
        Ok(Json(json!({ "result": false, "message": "Gateway WhatsApp tidak tersedia / nomor tidak ditemukan" })))
    }
}

// ---------- Transaction detail ----------

#[derive(Deserialize)]
pub struct TransactionDetailQuery { pub order_id: String }

async fn handle_admin_transaction_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<TransactionDetailQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &headers).await?;
    use sqlx::Row;
    let r = sqlx::query("SELECT * FROM transaction WHERE order_id = ? LIMIT 1")
        .bind(&q.order_id).fetch_optional(&state.db).await?
        .ok_or(AppError::ServiceNotFound)?;

    let opt_s = |col: &str| -> String { r.try_get::<String, _>(col).unwrap_or_default() };
    let opt_f = |col: &str| -> f64 { r.try_get::<f64, _>(col).unwrap_or(0.0) };
    let dt = |col: &str| -> String {
        r.try_get::<Option<chrono::NaiveDateTime>, _>(col).unwrap_or(None)
            .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_default()
    };

    Ok(Json(json!({
        "result": true,
        "data": {
            "order_id": opt_s("order_id"),
            "order_tid": opt_s("order_tid"),
            "user": opt_s("user"),
            "code": opt_s("code"),
            "service_name": opt_s("service_name"),
            "game": opt_s("game"),
            "target": opt_s("target"),
            "price": opt_f("price"),
            "profit": opt_f("profit"),
            "status": opt_s("status"),
            "payment_status": opt_s("payment_status"),
            "provider": opt_s("provider"),
            "note": opt_s("note"),
            "voucher": r.try_get::<String, _>("voucher").unwrap_or_default(),
            "flashsale": r.try_get::<String, _>("flashsale").unwrap_or_default(),
            "nickname": r.try_get::<String, _>("nickname").unwrap_or_default(),
            "phone": r.try_get::<String, _>("phone").unwrap_or_default(),
            "created_at": dt("created_at"),
            "updated_at": dt("updated_at"),
            "expired_at": dt("expired_at"),
        }
    })))
}

// ============================================================
// PUBLIC: VOUCHER CHECK (parity with library/ajax/check-voucher.php)
// ============================================================

#[derive(Deserialize)]
pub struct VoucherCheckRequest {
    pub code: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub price: f64,
    #[serde(default)]
    pub service_code: String,
}

async fn handle_voucher_check(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<VoucherCheckRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    use sqlx::Row;
    let ip = client_ip_from_headers(&headers);

    // ANTI-BRUTEFORCE: batasi frekuensi cek voucher per-IP (10x/menit)
    if !rate_limit(&format!("vchk:{}", ip), 10, 60) {
        return Ok(Json(json!({ "result": false, "message": "Terlalu banyak percobaan. Coba lagi nanti." })));
    }
    // ANTI-PROBING: kode salah >5x dalam 10 menit → blokir sementara IP ini
    if !record_failure(&format!("vfail:{}", ip), 5, 600) {
        return Ok(Json(json!({ "result": false, "message": "Akses voucher dibatasi sementara karena terlalu banyak kode salah." })));
    }

    let code = req.code.trim().to_string();
    if code.is_empty() {
        return Ok(Json(json!({ "result": false, "message": "Kode voucher tidak boleh kosong." })));
    }

    let row = match sqlx::query("SELECT * FROM voucher WHERE voucher = ? LIMIT 1")
        .bind(&code).fetch_optional(&state.db).await
    {
        Ok(r) => r,
        Err(_) => return Ok(Json(json!({ "result": false, "message": "Fitur voucher tidak tersedia." }))),
    };
    let v = match row {
        Some(v) => v,
        None => return Ok(Json(json!({ "result": false, "message": "Kode voucher tidak valid." }))),
    };

    let stock: i64 = v.try_get("stock").unwrap_or(0);
    if stock <= 0 {
        return Ok(Json(json!({ "result": false, "message": "Stok voucher sudah habis." })));
    }

    let v_category: String = v.try_get("code").unwrap_or_default();
    if !req.category.is_empty() && !v_category.is_empty() && v_category != req.category {
        return Ok(Json(json!({ "result": false, "message": "Voucher tidak berlaku untuk produk ini." })));
    }

    // Exclusive with flashsale (parity with PHP)
    if !req.service_code.is_empty() {
        let on_flash: Option<(i64,)> = sqlx::query_as("SELECT id FROM flashsale WHERE code = ? AND status = '1' LIMIT 1")
            .bind(&req.service_code).fetch_optional(&state.db).await.unwrap_or(None);
        if on_flash.is_some() {
            return Ok(Json(json!({ "result": false, "message": "Voucher tidak bisa digunakan untuk produk flashsale." })));
        }
    }

    let minimum: f64 = v.try_get("minimum").unwrap_or(0.0);
    if minimum > 0.0 && req.price > 0.0 && req.price < minimum {
        return Ok(Json(json!({ "result": false, "message": format!("Minimum belanja Rp{} untuk memakai voucher ini.", minimum) })));
    }

    let discount: f64 = v.try_get("discount").unwrap_or(0.0);
    // Voucher valid → hapus penalti kegagalan IP ini
    reset_failures(&format!("vfail:{}", ip));
    Ok(Json(json!({
        "result": true,
        "message": "Voucher valid!",
        "data": { "code": code, "discount": discount, "minimum": minimum }
    })))
}

// ============================================================
// PUBLIC: DEPOSIT DETAIL (parity with app/guest/deposit/faktur.php)
// ============================================================

async fn handle_deposit_detail(
    State(state): State<AppState>,
    axum::extract::Path(did): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    use sqlx::Row;
    let row = sqlx::query("SELECT * FROM deposit WHERE deposit_id = ? LIMIT 1")
        .bind(&did).fetch_optional(&state.db).await?
        .ok_or(AppError::ServiceNotFound)?;

    let action: String = row.try_get("action").unwrap_or_default();
    let mtype: String = row.try_get("type").unwrap_or_default();
    let provider: String = row.try_get("provider").unwrap_or_default();
    let amount: f64 = row.try_get("amount").unwrap_or(0.0);
    let fee: f64 = row.try_get("fee").unwrap_or(0.0);
    let uniq: f64 = row.try_get("balance").unwrap_or(0.0);

    let is_url = action.starts_with("http://") || action.starts_with("https://");
    let payment_kind = if is_url {
        "url"
    } else if mtype == "virtual" {
        "va"
    } else if provider == "X" || mtype == "convenience" {
        "guide"
    } else {
        "qr"
    };

    Ok(Json(json!({
        "result": true,
        "data": {
            "deposit_id": row.try_get::<String, _>("deposit_id").unwrap_or_else(|_| did.clone()),
            "username": row.try_get::<String, _>("username").unwrap_or_default(),
            "method": row.try_get::<String, _>("name").unwrap_or_default(),
            "method_code": row.try_get::<String, _>("method").unwrap_or_default(),
            "type": mtype,
            "provider": provider,
            "amount": amount,
            "fee": fee,
            "uniq": uniq,
            "total": amount + fee + uniq,
            "status": row.try_get::<String, _>("status").unwrap_or_default(),
            "payment_action": action,
            "payment_kind": payment_kind,
            "guide": row.try_get::<String, _>("note").unwrap_or_default(),
            "date": row.try_get::<String, _>("date").unwrap_or_default(),
            "expired": row.try_get::<String, _>("expired").unwrap_or_default(),
        }
    })))
}

// ============================================================
// EXTERNAL API EXTENSIONS (profile + order, parity api/profile.php & api/order.php)
// ============================================================

fn url_decode_component(s: &str) -> String {
    let plus_fixed = s.replace('+', " ");
    let bytes = plus_fixed.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&plus_fixed[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn parse_form_body(body: &str) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for pair in body.split('&') {
        if pair.is_empty() { continue; }
        let mut it = pair.splitn(2, '=');
        let k = url_decode_component(it.next().unwrap_or(""));
        let v = url_decode_component(it.next().unwrap_or(""));
        map.insert(k, json!(v));
    }
    serde_json::Value::Object(map)
}

#[derive(Deserialize)]
pub struct ExternalProfileRequest { pub key: String, pub sign: String }

async fn handle_external_profile(
    State(state): State<AppState>,
    Json(req): Json<ExternalProfileRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    use sqlx::Row;
    let api = verify_api_key(&state, &req.key, &req.sign).await?;
    let username: String = api.try_get("user").unwrap_or_default();

    let user = get_user_profile(&state.db, &username).await?;
    Ok(Json(json!({
        "result": true,
        "data": {
            "username": user.username,
            "balance": user.balance,
            "level": user.level,
            "registered": user.date_cr.clone().unwrap_or_default(),
        },
        "message": "Akun ditemukan"
    })))
}

async fn handle_external_order(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Result<Json<serde_json::Value>, AppError> {
    // Accept both JSON and form-urlencoded bodies (PHP api accepted form-data)
    let value: serde_json::Value = if body.trim_start().starts_with('{') {
        serde_json::from_str(&body).unwrap_or(parse_form_body(&body))
    } else {
        parse_form_body(&body)
    };
    let payload: rust_backend::models::ApiOrderRequest = serde_json::from_value(value)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    let client_ip = headers.get("x-forwarded-for")
        .and_then(|h| h.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("127.0.0.1")
        .to_string();

    let res = rust_backend::domain::orders::engine::process_api_order(&state, &client_ip, payload).await?;
    Ok(Json(serde_json::to_value(res).unwrap_or(json!({ "result": false }))))
}

async fn start_web_callback_listener(db: sqlx::MySqlPool) {
    let bot_token = std::env::var("TELEGRAM_CALLBACK_BOT_TOKEN")
        .or_else(|_| std::env::var("TELEGRAM_BOT_TOKEN"))
        .or_else(|_| std::env::var("TELEGRAM_BOT_SENDER_TOKEN"))
        .or_else(|_| std::env::var("TELEGRAM_BOT_2_TOKEN"))
        .unwrap_or_default();
    let group_id = std::env::var("TELEGRAM_GROUP_2_ID").unwrap_or_default();

    if bot_token.is_empty() || group_id.is_empty() {
        tracing::warn!("[WEB CALLBACK LISTENER] Telegram bot credentials missing. Background status callback listener skipped.");
        return;
    }

    // ANTI 409: token listener web TIDAK BOLEH sama dengan token listener
    // fulfillment (TELEGRAM_BOT_2_TOKEN) — dua getUpdates identik saling
    // merebut update (HTTP 409) dan pesan bisa hilang.
    if let Ok(t2) = std::env::var("TELEGRAM_BOT_2_TOKEN") {
        if !t2.is_empty() && t2 == bot_token {
            tracing::error!(
                "[CONFIG FATAL] Token listener Web SAMA dengan TELEGRAM_BOT_2_TOKEN (listener fulfillment). \
                 Dua getUpdates dengan token yang sama akan bertabrakan (HTTP 409). WAJIB beda bot!"
            );
        }
    }

    tokio::spawn(async move {
        use teloxide::prelude::*;
        let bot = Bot::new(bot_token);
        tracing::info!("[WEB CALLBACK LISTENER] Starting Telegram background listener for status updates on Server Web...");

        teloxide::repl(bot, move |_bot: Bot, msg: Message| {
            let db = db.clone();
            let expected_group = group_id.clone();
            async move {
                if msg.chat.id.to_string() != expected_group {
                    return Ok(());
                }
                if let Some(text) = msg.text() {
                    let decrypted = if text.starts_with("ENC:") {
                        rust_backend::domain::telegram::sender::decrypt_telegram_payload(text).unwrap_or_default()
                    } else {
                        text.to_string()
                    };

                    for line in decrypted.lines() {
                        let parts: Vec<&str> = line.split_whitespace().collect();
                        if parts.is_empty() { continue; }

                        if parts[0] == "[STATUS_UPDATE]" && parts.len() >= 3 {
                            let order_id = parts[1];
                            let status = parts[2];
                            let note = if parts.len() >= 4 { parts[3].replace('_', " ") } else { String::new() };

                            // ACK rekonsiliasi: fulfillment jelas sudah menerima notif bayar
                            PENDING_PAID.remove(order_id);

                            let _ = sqlx::query("UPDATE transaction SET status = ?, note = ?, updated_at = NOW() WHERE order_id = ?")
                                .bind(status)
                                .bind(&note)
                                .bind(order_id)
                                .execute(&db)
                                .await;
                            tracing::info!("[WEB CALLBACK LISTENER] Replicated status for Order {}: {} (SN: {}) on DB Web", order_id, status, note);
                        } else if parts[0] == "[REFUND_USER]" && parts.len() >= 3 {
                            let username = parts[1];
                            let amount: f64 = parts[2].parse().unwrap_or(0.0);
                            let order_id = if parts.len() >= 4 { parts[3] } else { "-" };
                            
                            if amount > 0.0 {
                                if let Ok(mut tx) = db.begin().await {
                                    let _ = sqlx::query("UPDATE users SET balance = balance + ? WHERE username = ?")
                                        .bind(amount)
                                        .bind(username)
                                        .execute(&mut *tx)
                                        .await;

                                    let _ = sqlx::query("INSERT INTO mutation (username, amount, type, note, date_cr) VALUES (?, ?, '+', ?, NOW())")
                                        .bind(username)
                                        .bind(amount)
                                        .bind(format!("Refund Gagal Topup: {}", order_id))
                                        .execute(&mut *tx)
                                        .await;
                                    let _ = tx.commit().await;
                                    tracing::info!("[WEB CALLBACK LISTENER] Refunded Rp{} to user {} on DB Web", amount, username);
                                }
                            }
                        } else if parts[0] == "[SYNC_PRICE]" && parts.len() >= 8 {
                            // BOT 3: sinkronisasi harga layanan dari Server Topup (sumber kebenaran).
                            // Web hanya mencerminkan; perhitungan akhir tetap di fulfillment.
                            let code = parts[1];
                            let name = parts[2].replace('_', " ");
                            let price: f64 = parts[3].parse().unwrap_or(0.0);
                            let member: f64 = parts[4].parse().unwrap_or(0.0);
                            let reseller: f64 = parts[5].parse().unwrap_or(0.0);
                            let provider = parts[6];
                            let status = parts[7];

                            let upd = sqlx::query("UPDATE service SET name = ?, price = ?, member = ?, reseller = ?, provider = ?, status = ?, date_up = NOW() WHERE code = ?")
                                .bind(&name).bind(price).bind(member).bind(reseller).bind(provider).bind(status).bind(code)
                                .execute(&db)
                                .await;
                            match upd {
                                Ok(r) if r.rows_affected() == 0 => {
                                    let _ = sqlx::query("INSERT INTO service (code, name, game, type, price, member, reseller, provider, status, date_up) VALUES (?, ?, '', 'games', ?, ?, ?, ?, ?, NOW())")
                                        .bind(code).bind(&name).bind(price).bind(member).bind(reseller).bind(provider).bind(status)
                                        .execute(&db)
                                        .await;
                                    tracing::info!("[WEB SYNC] Service {} baru dibuat di DB Web (harga Rp{})", code, price);
                                }
                                Ok(_) => tracing::debug!("[WEB SYNC] Service {} tersinkron (Rp{})", code, price),
                                Err(e) => tracing::warn!("[WEB SYNC] Gagal sync service {}: {}", code, e),
                            }
                        } else if parts[0] == "[SYNC_FLASH]" && parts.len() >= 4 {
                            let code = parts[1];
                            let amount: f64 = parts[2].parse().unwrap_or(0.0);
                            let status = parts[3];

                            let res = sqlx::query("UPDATE flashsale SET amount = ?, status = ? WHERE code = ?")
                                .bind(amount).bind(status).bind(code)
                                .execute(&db)
                                .await;
                            if let Ok(r) = res {
                                if r.rows_affected() == 0 {
                                    let _ = sqlx::query("INSERT INTO flashsale (code, amount, status) VALUES (?, ?, ?)")
                                        .bind(code).bind(amount).bind(status)
                                        .execute(&db)
                                        .await;
                                }
                                tracing::debug!("[WEB SYNC] Flashsale {} tersinkron (status {})", code, status);
                            }
                        } else if parts[0] == "[SYNC_VOUCHER]" && parts.len() >= 6 {
                            let category_raw = parts[1];
                            let category = if category_raw == "-" { String::new() } else { category_raw.replace('_', " ") };
                            let voucher = parts[2];
                            let discount: f64 = parts[3].parse().unwrap_or(0.0);
                            let minimum: f64 = parts[4].parse().unwrap_or(0.0);
                            let stock: i64 = parts[5].parse().unwrap_or(0);

                            let res = sqlx::query("UPDATE voucher SET code = ?, discount = ?, minimum = ?, stock = ? WHERE voucher = ?")
                                .bind(&category).bind(discount).bind(minimum).bind(stock).bind(voucher)
                                .execute(&db)
                                .await;
                            if let Ok(r) = res {
                                if r.rows_affected() == 0 {
                                    let _ = sqlx::query("INSERT INTO voucher (code, voucher, discount, minimum, stock, date_cr) VALUES (?, ?, ?, ?, ?, NOW())")
                                        .bind(&category).bind(voucher).bind(discount).bind(minimum).bind(stock)
                                        .execute(&db)
                                        .await;
                                }
                                tracing::debug!("[WEB SYNC] Voucher {} tersinkron (stok {})", voucher, stock);
                            }
                        } else if parts[0] == "[SYNC_USER]" && parts.len() >= 3 {
                            // Level otoritatif dari Server Topup (web tidak menentukan level sendiri)
                            let username = parts[1];
                            let level = parts[2];
                            let _ = sqlx::query("UPDATE users SET level = ? WHERE username = ?")
                                .bind(level)
                                .bind(username)
                                .execute(&db)
                                .await;
                            tracing::debug!("[WEB SYNC] Level user {} → {}", username, level);
                        } else if parts[0] == "[ORDER_ACCEPTED]" && parts.len() >= 3 {
                            // Fulfillment MENERIMA order → kunci harga otoritatif.
                            // Invoice gateway baru dibuat setelah state ini (oleh faktur polling).
                            let order_id = parts[1];
                            let price: f64 = parts[2].parse().unwrap_or(0.0);
                            let _ = sqlx::query(
                                "UPDATE transaction SET price = ?, note = 'ACCEPTED', updated_at = NOW() WHERE order_id = ? AND payment_status = 'unpaid'",
                            )
                            .bind(price)
                            .bind(order_id)
                            .execute(&db)
                            .await;
                            tracing::info!("[WEB CHECKOUT] Order {} ACCEPTED oleh Topup Server (harga final Rp{})", order_id, price);
                        } else if parts[0] == "[ORDER_REJECTED]" && parts.len() >= 2 {
                            // Fulfillment MENOLAK order → batalkan sebelum invoice dibuat
                            let order_id = parts[1];
                            let reason = if parts.len() >= 3 { parts[2].replace('_', " ") } else { "Tidak valid".to_string() };
                            let _ = sqlx::query(
                                "UPDATE transaction SET status = 'error', payment_status = 'cancel', note = ?, updated_at = NOW() WHERE order_id = ? AND payment_status = 'unpaid'",
                            )
                            .bind(format!("DITOLAK: {}", reason))
                            .bind(order_id)
                            .execute(&db)
                            .await;
                            tracing::warn!("[WEB CHECKOUT] Order {} REJECTED oleh Topup Server: {}", order_id, reason);
                        }
                    }
                }
                Ok(())
            }
        }).await;
    });
}
