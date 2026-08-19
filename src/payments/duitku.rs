use crate::error::AppError;
use md5::{Digest, Md5};
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct DuitkuItemDetail {
    pub name: String,
    pub price: u64,
    pub quantity: u32,
}

#[allow(non_snake_case)]
#[derive(Debug, Serialize)]
pub struct DuitkuCreateRequest {
    pub merchantCode: String,
    pub paymentAmount: u64,
    pub merchantOrderId: String,
    pub productDetails: String,
    pub email: String,
    pub paymentMethod: String,
    pub customerVaName: String,
    pub phoneNumber: String,
    pub itemDetails: Vec<DuitkuItemDetail>,
    pub returnUrl: String,
    pub callbackUrl: String,
    pub signature: String,
}

#[allow(non_snake_case)]
#[derive(Debug, Deserialize)]
pub struct DuitkuCreateResponse {
    pub merchantCode: Option<String>,
    pub reference: Option<String>,
    pub paymentUrl: Option<String>,
    pub qrString: Option<String>,
    pub statusCode: String, // 00 for success
    pub statusMessage: Option<String>,
}

#[allow(non_snake_case)]
#[derive(Debug, Deserialize)]
pub struct DuitkuWebhookPayload {
    pub merchantCode: String,
    pub amount: String,
    pub merchantOrderId: String,
    pub resultCode: String, // 00 for success
    pub signature: String,
}

pub struct DuitkuGateway {
    pub merchant_code: String,
    pub api_key: String,
    pub base_url: String,
}

impl DuitkuGateway {
    pub fn new(merchant_code: String, api_key: String, is_production: bool) -> Self {
        let base_url = if is_production {
            "https://passport.duitku.com/webapi/api/merchant/v2/inquiry".to_string()
        } else {
            "https://sandbox.duitku.com/webapi/api/merchant/v2/inquiry".to_string()
        };
        Self {
            merchant_code,
            api_key,
            base_url,
        }
    }

    pub fn generate_request_signature(&self, merchant_order_id: &str, amount: u64) -> String {
        let raw = format!("{}{}{}{}", self.merchant_code, merchant_order_id, amount, self.api_key);
        let mut hasher = Md5::new();
        hasher.update(raw.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    pub fn verify_webhook_signature(&self, amount: &str, merchant_order_id: &str, incoming_signature: &str) -> bool {
        let raw = format!("{}{}{}{}", self.merchant_code, amount, merchant_order_id, self.api_key);
        let mut hasher = Md5::new();
        hasher.update(raw.as_bytes());
        let expected = format!("{:x}", hasher.finalize());
        expected.eq_ignore_ascii_case(incoming_signature)
    }

    pub async fn create_transaction(
        &self,
        client: &Client,
        method: &str,
        merchant_order_id: &str,
        amount: u64,
        service_name: &str,
        target_id: &str,
        customer_name: &str,
        customer_email: &str,
        customer_phone: &str,
        return_url: &str,
        callback_url: &str,
    ) -> Result<DuitkuCreateResponse, AppError> {
        let signature = self.generate_request_signature(merchant_order_id, amount);
        let item_title = format!("{} (Target: {})", service_name, target_id);

        let payload = DuitkuCreateRequest {
            merchantCode: self.merchant_code.clone(),
            paymentAmount: amount,
            merchantOrderId: merchant_order_id.to_string(),
            productDetails: item_title.clone(),
            email: customer_email.to_string(),
            paymentMethod: method.to_string(),
            customerVaName: customer_name.to_string(),
            phoneNumber: customer_phone.to_string(),
            itemDetails: vec![DuitkuItemDetail {
                name: item_title,
                price: amount,
                quantity: 1,
            }],
            returnUrl: return_url.to_string(),
            callbackUrl: callback_url.to_string(),
            signature,
        };

        let response = client
            .post(&self.base_url)
            .json(&payload)
            .send()
            .await?;

        let res: DuitkuCreateResponse = response.json().await?;
        if res.statusCode == "00" {
            Ok(res)
        } else {
            Err(AppError::ProviderError(format!(
                "Duitku Error: {}",
                res.statusMessage.unwrap_or_else(|| "Unknown error".to_string())
            )))
        }
    }
}
