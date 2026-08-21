use sqlx::{MySqlPool, Row};

#[derive(Debug, Clone)]
pub struct ResolvedPrice {
    pub base: f64,
    pub margin: f64,
    pub flash_discount: f64,
    pub voucher_discount: f64,
    pub final_price: f64,
    pub service_name: String,
    pub provider: String,
    pub voucher_code: String,
}

/// Otoritatif harga di sisi Server Topup (fulfillment).
/// Web TIDAK dipercaya untuk harga: nilai ini dihitung ulang dari DB Topup
/// (service + level user + flashsale + voucher) setiap kali order masuk.
pub async fn resolve_price(
    db: &MySqlPool,
    service_code: &str,
    username: &str,
    voucher_code: &str,
) -> Result<ResolvedPrice, String> {
    // 1. Service wajib ada di DB Topup
    let svc = sqlx::query("SELECT price, member, reseller, name, provider, game FROM service WHERE code = ? LIMIT 1")
        .bind(service_code)
        .fetch_optional(db)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Service {} not found in Topup DB", service_code))?;

    let base: f64 = svc.try_get("price").unwrap_or(0.0);
    let member_margin: f64 = svc.try_get("member").unwrap_or(0.0);
    let reseller_margin: f64 = svc.try_get("reseller").unwrap_or(0.0);
    let service_name: String = svc.try_get("name").unwrap_or_default();
    let provider: String = svc.try_get("provider").unwrap_or_else(|_| "DIGI".to_string());
    let game: String = svc.try_get("game").unwrap_or_default();

    // 2. Margin berdasarkan level user (guest tidak dapat margin)
    let is_known_user = !username.is_empty() && username != "-" && username != "GUEST";
    let mut margin = 0.0;
    if is_known_user {
        let level: String = sqlx::query_scalar("SELECT COALESCE(level, 'Member') FROM users WHERE username = ? LIMIT 1")
            .bind(username)
            .fetch_optional(db)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "Member".to_string());
        margin = if level == "Reseller" || level == "Admin" { reseller_margin } else { member_margin };
    }

    let subtotal = base + margin;

    // 3. Flashsale aktif
    let mut flash_discount = 0.0f64;
    if let Ok(Some(row)) = sqlx::query("SELECT amount FROM flashsale WHERE code = ? AND status = '1' LIMIT 1")
        .bind(service_code)
        .fetch_optional(db)
        .await
    {
        flash_discount = row.try_get("amount").unwrap_or(0.0);
    }

    // 4. Voucher (eksklusif dengan flashsale; tabel opsional di DB Topup)
    let mut voucher_discount = 0.0f64;
    let mut applied_voucher = String::new();
    let vc = voucher_code.trim();
    if is_known_user && !vc.is_empty() && vc != "-" && flash_discount <= 0.0 {
        if let Ok(Some(v)) = sqlx::query("SELECT code, discount, minimum, stock FROM voucher WHERE voucher = ? LIMIT 1")
            .bind(vc)
            .fetch_optional(db)
            .await
        {
            let stock: i64 = v.try_get("stock").unwrap_or(0);
            let v_category: String = v.try_get("code").unwrap_or_default();
            let minimum: f64 = v.try_get("minimum").unwrap_or(0.0);
            let discount: f64 = v.try_get("discount").unwrap_or(0.0);

            let after_flash = (subtotal - flash_discount).max(0.0);
            if stock > 0
                && (v_category.is_empty() || v_category == game)
                && (minimum <= 0.0 || after_flash >= minimum)
            {
                voucher_discount = discount;
                applied_voucher = vc.to_string();
            } else {
                tracing::warn!(
                    "[PRICING] Voucher {} rejected for order (stock={}, category_match={}, minimum_ok={})",
                    vc, stock, v_category.is_empty() || v_category == game, minimum <= 0.0 || after_flash >= minimum
                );
            }
        }
    }

    let final_price = (subtotal - flash_discount - voucher_discount).max(0.0);

    Ok(ResolvedPrice {
        base,
        margin,
        flash_discount,
        voucher_discount,
        final_price,
        service_name,
        provider,
        voucher_code: applied_voucher,
    })
}

/// Klaim slot voucher pada SATU order (idempoten anti duplikasi pesan).
/// Untuk order saldo, fulfillment menerima [NEW_ORDER] DAN [REQ_OTP] yang sama-sama
/// membawa kode voucher. Fungsi ini memastikan hanya handler PERTAMA yang berhak
/// memotong stok: UPDATE ... WHERE voucher masih kosong = atomic claim per order.
/// Return true bila handler ini yang memenangkan klaim (wajib memotong stok).
pub async fn claim_order_voucher(db: &MySqlPool, order_id: &str, voucher_code: &str) -> bool {
    match sqlx::query(
        "UPDATE transaction SET voucher = ? WHERE order_id = ? AND (voucher IS NULL OR voucher = '')",
    )
    .bind(voucher_code)
    .bind(order_id)
    .execute(db)
    .await
    {
        Ok(res) => res.rows_affected() > 0,
        Err(e) => {
            tracing::warn!("[PRICING] Failed to claim voucher {} for order {}: {}", voucher_code, order_id, e);
            false
        }
    }
}

/// Lepaskan klaim voucher pada order (dipakai saat stok ternyata habis).
pub async fn release_order_voucher(db: &MySqlPool, order_id: &str) {
    let _ = sqlx::query("UPDATE transaction SET voucher = '' WHERE order_id = ?")
        .bind(order_id)
        .execute(db)
        .await;
}

/// Potong stok voucher SECARA ATOMIK (anti double-spend/race condition).
/// Dipanggil saat ORDER DIBUAT di Server Topup — bukan saat lunas — sehingga
/// N permintaan bersamaan dengan stok 1 hanya akan menghasilkan 1 pemenang
/// (InnoDB row-lock menjamin serialisasi UPDATE ini).
/// Return true bila voucher berhasil dikonsumsi.
pub async fn try_consume_voucher(db: &MySqlPool, voucher_code: &str) -> bool {
    if voucher_code.is_empty() || voucher_code == "-" {
        return false;
    }
    match sqlx::query("UPDATE voucher SET stock = stock - 1 WHERE voucher = ? AND stock > 0")
        .bind(voucher_code)
        .execute(db)
        .await
    {
        Ok(res) => {
            if res.rows_affected() > 0 {
                tracing::info!("[PRICING] Voucher {} consumed atomically (stock decremented)", voucher_code);
                true
            } else {
                tracing::warn!("[PRICING] Voucher {} rejected: stock empty (possible brute-force/race)", voucher_code);
                false
            }
        }
        Err(e) => {
            tracing::warn!("[PRICING] Failed to consume voucher {}: {}", voucher_code, e);
            false
        }
    }
}

/// Kembalikan stok voucher (order expire / topup gagal → pembeli tidak jadi memakai promo).
pub async fn restock_voucher(db: &MySqlPool, voucher_code: &str) {
    if voucher_code.is_empty() || voucher_code == "-" {
        return;
    }
    let _ = sqlx::query("UPDATE voucher SET stock = stock + 1 WHERE voucher = ?")
        .bind(voucher_code)
        .execute(db)
        .await;
    tracing::info!("[PRICING] Voucher {} restocked", voucher_code);
}
