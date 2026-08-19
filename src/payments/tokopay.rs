use crate::error::AppError;
use md5::{Digest, Md5};
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct TokopayItemDetail {
    pub product_code: String,
    pub name: String,
    pub price: u64,
    pub product_url: String,
    pub image_url: String,
}

#[derive(Debug, Serialize)]
pub struct TokopayCreateRequest {
    pub merchant_id: String,
    pub kode_channel: String,
    pub reff_id: String,
    pub amount: u64,
    pub customer_name: String,
    pub customer_email: String,
    pub customer_phone: String,
    pub redirect_url: String,
    pub expired_ts: u64,
    pub signature: String,
    pub items: Vec<TokopayItemDetail>,
}

#[derive(Debug, Deserialize)]
pub struct TokopayCreateData {
    pub trx_id: Option<String>,
    pub pay_url: Option<String>,
    pub qr_link: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TokopayCreateResponse {
    pub status: bool,
    pub data: Option<TokopayCreateData>,
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TokopayWebhookPayload {
    pub merchant_id: String,
    pub reff_id: String,
    pub amount: u64,
    pub status: String, // Success
    pub signature: String,
}

pub struct TokopayGateway {
    pub merchant_id: String,
    pub secret_key: String,
    pub endpoint: String,
}

impl TokopayGateway {
    pub fn new(merchant_id: String, secret_key: String) -> Self {
        Self {
            merchant_id,
            secret_key,
            endpoint: "https://api.tokopay.id/v1/order".to_string(),
        }
    }

    pub fn generate_signature(&self, reff_id: &str) -> String {
        let raw = format!("{}:{}:{}", self.merchant_id, self.secret_key, reff_id);
        let mut hasher = Md5::new();
        hasher.update(raw.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    pub async fn create_transaction(
        &self,
        client: &Client,
        channel_code: &str,
        reff_id: &str,
        amount: u64,
        category_code: &str,
        service_name: &str,
        target_id: &str,
        customer_name: &str,
        customer_email: &str,
        customer_phone: &str,
        redirect_url: &str,
    ) -> Result<TokopayCreateData, AppError> {
        let signature = self.generate_signature(reff_id);
        let item_title = format!("{} [ID: {}]", service_name, target_id);

        let payload = TokopayCreateRequest {
            merchant_id: self.merchant_id.clone(),
            kode_channel: channel_code.to_string(),
            reff_id: reff_id.to_string(),
            amount,
            customer_name: customer_name.to_string(),
            customer_email: customer_email.to_string(),
            customer_phone: customer_phone.to_string(),
            redirect_url: redirect_url.to_string(),
            expired_ts: 0,
            signature,
            items: vec![TokopayItemDetail {
                product_code: category_code.to_string(),
                name: item_title,
                price: amount,
                product_url: "".to_string(),
                image_url: "".to_string(),
            }],
        };

        let response = client
            .post(&self.endpoint)
            .json(&payload)
            .send()
            .await?;

        let res: TokopayCreateResponse = response.json().await?;
        if res.status {
            res.data.ok_or_else(|| AppError::InternalError("Tokopay data empty".to_string()))
        } else {
            Err(AppError::ProviderError(format!(
                "Tokopay Error: {}",
                res.message.unwrap_or_else(|| "Failed".to_string())
            )))
        }
    }
}
