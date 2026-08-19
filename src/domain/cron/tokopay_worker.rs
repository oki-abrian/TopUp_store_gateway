use sqlx::{MySqlPool, Row};
use std::time::Duration;
use reqwest::Client;
use serde_json::Value;

fn is_safe_string(s: &str) -> bool {
    s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub async fn start_tokopay_worker(db_pool: MySqlPool) {
    tokio::spawn(async move {
        tracing::info!("[TOKOPAY WORKER] Starting Tokopay polling worker...");
        let client = Client::new();

        loop {
            // Sleep for 60 seconds between cycles (Slow fallback/reconciliation mode)
            tokio::time::sleep(Duration::from_secs(60)).await;

            // Fetch unpaid Tokopay deposits
            let query = "SELECT deposit_id, username, amount FROM deposit WHERE status = 'unpaid' AND method = 'TOKOPAY'";
        
        match sqlx::query(query).fetch_all(&db_pool).await {
            Ok(rows) => {
                if rows.is_empty() {
                    continue; // Nothing to check
                }

                // Get Tokopay credentials from DB
                let provider_query = "SELECT merchant, apikey FROM provider WHERE code = 'TOKOPAY' LIMIT 1";
                if let Ok(Some(prov)) = sqlx::query(provider_query).fetch_optional(&db_pool).await {
                    let merchant: String = prov.try_get("merchant").unwrap_or_default();
                    let secret: String = prov.try_get("apikey").unwrap_or_default();

                    for row in rows {
                        let deposit_id: String = row.try_get("deposit_id").unwrap_or_default();
                        let username: String = row.try_get("username").unwrap_or_default();
                        let amount: f64 = row.try_get("amount").unwrap_or_default();

                        // Query Tokopay API
                        let mut params = std::collections::HashMap::new();
                        params.insert("merchant", merchant.clone());
                        params.insert("secret", secret.clone());
                        params.insert("ref_id", deposit_id.clone());
                        params.insert("nominal", amount.to_string());

                        tracing::info!("[TOKOPAY WORKER] Checking status for deposit {}", deposit_id);

                        if let Ok(resp) = client.get("https://api.tokopay.id/v1/order").query(&params).send().await {
                            if let Ok(json) = resp.json::<Value>().await {
                                // Assuming Tokopay returns something like {"data": {"status": "Success"}} or similar
                                // Let's check for "status" field in "data" or root
                                let mut is_paid = false;
                                
                                if let Some(data) = json.get("data") {
                                    if let Some(status) = data.get("status") {
                                        let status_str = status.as_str().unwrap_or_default().to_lowercase();
                                        if is_safe_string(&status_str) && (status_str == "success" || status_str == "paid") {
                                            is_paid = true;
                                        }
                                    }
                                } else if let Some(status) = json.get("status") {
                                    let status_str = status.as_str().unwrap_or_default().to_lowercase();
                                    if is_safe_string(&status_str) && (status_str == "success" || status_str == "paid") {
                                        is_paid = true;
                                    }
                                }

                                if is_paid {
                                    tracing::info!("[TOKOPAY WORKER] Deposit {} is PAID! Updating balance for {}", deposit_id, username);
                                    
                                    // Atomic update
                                    let mut tx = match db_pool.begin().await {
                                        Ok(t) => t,
                                        Err(_) => continue,
                                    };

                                    let mark_paid = sqlx::query("UPDATE deposit SET status = 'paid' WHERE deposit_id = ? AND status = 'unpaid'")
                                        .bind(&deposit_id)
                                        .execute(&mut *tx)
                                        .await;

                                    if let Ok(res) = mark_paid {
                                        if res.rows_affected() > 0 {
                                            let add_balance = sqlx::query("UPDATE users SET balance = balance + ? WHERE username = ?")
                                                .bind(amount)
                                                .bind(&username)
                                                .execute(&mut *tx)
                                                .await;

                                            if add_balance.is_ok() {
                                                let _ = tx.commit().await;
                                                tracing::info!("[TOKOPAY WORKER] Successfully added Rp{} to user {}", amount, username);
                                            } else {
                                                let _ = tx.rollback().await;
                                            }
                                        } else {
                                            let _ = tx.rollback().await;
                                        }
                                    } else {
                                        let _ = tx.rollback().await;
                                    }
                                } else if let Some(status) = json.get("status") {
                                     let status_str = status.as_str().unwrap_or_default().to_lowercase();
                                     if status_str == "failed" || status_str == "expired" || status_str == "cancel" {
                                          let _ = sqlx::query("UPDATE deposit SET status = 'canceled' WHERE deposit_id = ?")
                                                .bind(&deposit_id)
                                                .execute(&db_pool)
                                                .await;
                                     }
                                }
                            }
                        }
                    }
                } else {
                    tracing::warn!("[TOKOPAY WORKER] TOKOPAY provider config not found in DB.");
                }
            }
            Err(e) => {
                tracing::error!("[TOKOPAY WORKER] Failed to fetch deposits: {}", e);
            }
        }

        // Fetch unpaid Tokopay TRANSACTIONS (Orders)
        let trx_query = "SELECT order_id, user, price FROM transaction WHERE payment_status = 'unpaid' AND (code LIKE '%TOKOPAY%' OR service_name LIKE '%TOKOPAY%' OR provider = 'TOKOPAY')";
        
        match sqlx::query(trx_query).fetch_all(&db_pool).await {
            Ok(rows) => {
                if !rows.is_empty() {
                    let provider_query = "SELECT merchant, apikey FROM provider WHERE code = 'TOKOPAY' LIMIT 1";
                    if let Ok(Some(prov)) = sqlx::query(provider_query).fetch_optional(&db_pool).await {
                        let merchant: String = prov.try_get("merchant").unwrap_or_default();
                        let secret: String = prov.try_get("apikey").unwrap_or_default();

                        for row in rows {
                            let order_id: String = row.try_get("order_id").unwrap_or_default();
                            let price: f64 = row.try_get("price").unwrap_or_default();

                            let mut params = std::collections::HashMap::new();
                            params.insert("merchant", merchant.clone());
                            params.insert("secret", secret.clone());
                            params.insert("ref_id", order_id.clone());
                            params.insert("nominal", price.to_string());

                            tracing::info!("[TOKOPAY WORKER] Checking status for order {}", order_id);

                            if let Ok(resp) = client.get("https://api.tokopay.id/v1/order").query(&params).send().await {
                                if let Ok(json) = resp.json::<Value>().await {
                                    let mut is_paid = false;
                                    
                                    if let Some(data) = json.get("data") {
                                        if let Some(status) = data.get("status") {
                                            let status_str = status.as_str().unwrap_or_default().to_lowercase();
                                            if is_safe_string(&status_str) && (status_str == "success" || status_str == "paid") {
                                                is_paid = true;
                                            }
                                        }
                                    } else if let Some(status) = json.get("status") {
                                        let status_str = status.as_str().unwrap_or_default().to_lowercase();
                                        if is_safe_string(&status_str) && (status_str == "success" || status_str == "paid") {
                                            is_paid = true;
                                        }
                                    }

                                    if is_paid {
                                        tracing::info!("[TOKOPAY WORKER] Order {} is PAID! Processing to Digiflazz via atomic claim...", order_id);
                                        
                                        // Call Digiflazz through exclusive atomic claim in process_paid_transaction
                                        let digi_client = reqwest::Client::new();
                                        match crate::domain::telegram::process_paid_transaction(&db_pool, &digi_client, &order_id, price).await {
                                            Ok(_) => tracing::info!("[TOKOPAY WORKER] Order {} successfully topped up!", order_id),
                                            Err(e) => tracing::error!("[TOKOPAY WORKER] Failed to process order {}: {}", order_id, e),
                                        }
                                    } else if let Some(status) = json.get("status") {
                                        let status_str = status.as_str().unwrap_or_default().to_lowercase();
                                        if status_str == "failed" || status_str == "expired" || status_str == "cancel" {
                                             let _ = sqlx::query("UPDATE transaction SET status = 'error', payment_status = 'canceled', note = 'Expired/Canceled from Gateway' WHERE order_id = ?")
                                                   .bind(&order_id)
                                                   .execute(&db_pool)
                                                   .await;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                tracing::error!("[TOKOPAY WORKER] Failed to fetch transactions: {}", e);
            }
        }
    }
    });
}
