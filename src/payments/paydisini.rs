use crate::error::AppError;
use md5::{Digest, Md5};
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct PaydisiniCreateRequest {
    pub key: String,
    pub request: String,
    pub merchant_id: String,
    pub unique_code: String,
    pub service: String,
    pub amount: u64,
    pub note: String,
    pub valid_time: u32,
    pub customer_email: String,
    pub ewallet_phone: String,
    pub type_fee: u8,
    pub payment_guide: bool,
    pub return_url: String,
    pub signature: String,
}

#[derive(Debug, Deserialize)]
pub struct PaydisiniCreateData {
    pub checkout_url: Option<String>,
    pub qr_content: Option<String>,
    pub pay_code: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PaydisiniCreateResponse {
    pub success: bool,
    pub data: Option<PaydisiniCreateData>,
    pub msg: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PaydisiniWebhookPayload {
    pub key: String,
    pub unique_code: String,
    pub status: String, // Success
    pub amount: u64,
    pub signature: String,
}

pub struct PaydisiniGateway {
    pub api_key: String,
    pub merchant_id: String,
    pub endpoint: String,
}

impl PaydisiniGateway {
    pub fn new(api_key: String, merchant_id: String) -> Self {
        Self {
            api_key,
            merchant_id,
            endpoint: "https://paydisini.co.id/api/".to_string(),
        }
    }

    pub fn generate_request_signature(&self, unique_code: &str, service_id: &str, amount: u64, valid_time: u32) -> String {
        let raw = format!("{}{}{}{}{}NewTransaction", self.api_key, unique_code, service_id, amount, valid_time);
        let mut hasher = Md5::new();
        hasher.update(raw.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    pub async fn create_transaction(
        &self,
        client: &Client,
        service_id: &str,
        unique_code: &str,
        amount: u64,
        service_name: &str,
        target_id: &str,
        customer_email: &str,
        customer_phone: &str,
        return_url: &str,
    ) -> Result<PaydisiniCreateData, AppError> {
        let valid_time = 1800; // 30 mins
        let signature = self.generate_request_signature(unique_code, service_id, amount, valid_time);
        let note = format!("Pembelian {} - ID Game: {} (Order #{})", service_name, target_id, unique_code);

        let payload = PaydisiniCreateRequest {
            key: self.api_key.clone(),
            request: "new".to_string(),
            merchant_id: self.merchant_id.clone(),
            unique_code: unique_code.to_string(),
            service: service_id.to_string(),
            amount,
            note,
            valid_time,
            customer_email: customer_email.to_string(),
            ewallet_phone: customer_phone.to_string(),
            type_fee: 1,
            payment_guide: true,
            return_url: return_url.to_string(),
            signature,
        };

        let response = client
            .post(&self.endpoint)
            .form(&payload)
            .send()
            .await?;

        let res: PaydisiniCreateResponse = response.json().await?;
        if res.success {
            res.data.ok_or_else(|| AppError::InternalError("Paydisini data empty".to_string()))
        } else {
            Err(AppError::ProviderError(format!(
                "Paydisini Error: {}",
                res.msg.unwrap_or_else(|| "Failed".to_string())
            )))
        }
    }
}
