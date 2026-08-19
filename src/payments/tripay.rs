use crate::error::AppError;
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

#[derive(Debug, Serialize)]
pub struct TripayOrderItem {
    pub sku: String,
    pub name: String,
    pub price: f64,
    pub quantity: u32,
    pub product_url: String,
    pub image_url: String,
}

#[derive(Debug, Serialize)]
pub struct TripayCreateRequest {
    pub method: String,
    pub merchant_ref: String,
    pub amount: f64,
    pub customer_name: String,
    pub customer_email: String,
    pub customer_phone: String,
    pub order_items: Vec<TripayOrderItem>,
    pub return_url: String,
    pub expired_time: i64,
    pub signature: String,
}

#[derive(Debug, Deserialize)]
pub struct TripayCreateResponseData {
    pub reference: String,
    pub merchant_ref: String,
    pub payment_method: String,
    pub payment_name: String,
    pub amount: f64,
    pub checkout_url: Option<String>,
    pub qr_string: Option<String>,
    pub pay_code: Option<String>,
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct TripayCreateResponse {
    pub success: bool,
    pub message: String,
    pub data: Option<TripayCreateResponseData>,
}

#[derive(Debug, Deserialize)]
pub struct TripayWebhookPayload {
    pub reference: String,
    pub merchant_ref: String,
    pub payment_method: String,
    pub amount_received: f64,
    pub status: String, // PAID, EXPIRED, FAILED
}

pub struct TripayGateway {
    pub merchant_code: String,
    pub api_key: String,
    pub private_key: String,
    pub base_url: String,
}

impl TripayGateway {
    pub fn new(merchant_code: String, api_key: String, private_key: String, is_production: bool) -> Self {
        let base_url = if is_production {
            "https://tripay.co.id/api".to_string()
        } else {
            "https://tripay.co.id/api-sandbox".to_string()
        };
        Self {
            merchant_code,
            api_key,
            private_key,
            base_url,
        }
    }

    pub fn generate_request_signature(&self, merchant_ref: &str, amount: f64) -> String {
        type HmacSha256 = Hmac<Sha256>;
        let raw = format!("{}{}{}", self.merchant_code, merchant_ref, amount as u64);
        let mut mac = HmacSha256::new_from_slice(self.private_key.as_bytes()).expect("HMAC keys can be any length");
        mac.update(raw.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }

    pub fn verify_webhook_signature(&self, raw_payload: &str, signature_header: &str) -> bool {
        type HmacSha256 = Hmac<Sha256>;
        if let Ok(mut mac) = HmacSha256::new_from_slice(self.private_key.as_bytes()) {
            let _: &mut HmacSha256 = &mut mac;
            mac.update(raw_payload.as_bytes());
            let expected = hex::encode(mac.finalize().into_bytes());
            expected.eq_ignore_ascii_case(signature_header)
        } else {
            false
        }
    }


    pub async fn create_transaction(
        &self,
        client: &Client,
        method: &str,
        merchant_ref: &str,
        amount: f64,
        service_name: &str,
        target_id: &str,
        customer_name: &str,
        customer_email: &str,
        customer_phone: &str,
        return_url: &str,
    ) -> Result<TripayCreateResponseData, AppError> {
        let signature = self.generate_request_signature(merchant_ref, amount);
        let expired_time = chrono::Utc::now().timestamp() + (24 * 3600);

        let item_detail_name = format!("{} (Target: {})", service_name, target_id);

        let payload = TripayCreateRequest {
            method: method.to_string(),
            merchant_ref: merchant_ref.to_string(),
            amount,
            customer_name: customer_name.to_string(),
            customer_email: customer_email.to_string(),
            customer_phone: customer_phone.to_string(),
            order_items: vec![TripayOrderItem {
                sku: merchant_ref.to_string(),
                name: item_detail_name,
                price: amount,
                quantity: 1,
                product_url: format!("{}/order/{}", return_url, merchant_ref),
                image_url: "".to_string(),
            }],
            return_url: return_url.to_string(),
            expired_time,
            signature,
        };

        let endpoint = format!("{}/transaction/create", self.base_url);
        let response = client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&payload)
            .send()
            .await?;

        let res: TripayCreateResponse = response.json().await?;
        if res.success {
            res.data.ok_or_else(|| AppError::InternalError("Tripay data empty".to_string()))
        } else {
            Err(AppError::ProviderError(format!("Tripay Error: {}", res.message)))
        }
    }
}
