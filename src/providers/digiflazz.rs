use crate::error::AppError;
use md5::{Digest, Md5};
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub struct DigiFlazzClient {
    pub username: String,
    pub apikey: String,
    pub endpoint: String,
}

#[derive(Serialize)]
struct DigiTopupPayload<'a> {
    username: &'a str,
    buyer_sku_code: &'a str,
    customer_no: &'a str,
    ref_id: &'a str,
    sign: String,
}

#[derive(Deserialize, Debug)]
pub struct DigiResponseData {
    pub ref_id: Option<String>,
    pub buyer_sku_code: Option<String>,
    pub customer_no: Option<String>,
    pub message: Option<String>,
    pub status: Option<String>,
    pub rc: Option<String>,
    pub sn: Option<String>,
    pub buyer_last_sal: Option<f64>,
    pub price: Option<f64>,
}

#[derive(Deserialize, Debug)]
pub struct DigiResponse {
    pub data: Option<DigiResponseData>,
}

pub struct DigiResult {
    pub success: bool,
    pub is_pending: bool,
    pub is_failed: bool,
    pub trxid: String,
    pub sn: String,
    pub message: String,
}

impl DigiFlazzClient {
    pub fn new(username: String, apikey: String) -> Self {
        Self {
            username,
            apikey,
            endpoint: "https://api.digiflazz.com/v1/transaction".to_string(),
        }
    }

    pub fn generate_signature(&self, ref_id: &str) -> String {
        let raw = format!("{}{}{}", self.username, self.apikey, ref_id);
        let mut hasher = Md5::new();
        hasher.update(raw.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    pub async fn topup(
        &self,
        client: &Client,
        sku_code: &str,
        customer_no: &str,
        ref_id: &str,
    ) -> Result<DigiResult, AppError> {
        let sign = self.generate_signature(ref_id);
        let payload = DigiTopupPayload {
            username: &self.username,
            buyer_sku_code: sku_code,
            customer_no,
            ref_id,
            sign,
        };

        let response = client
            .post(&self.endpoint)
            .json(&payload)
            .send()
            .await?;

        let res_text = response.text().await?;
        let digi_res: DigiResponse = serde_json::from_str(&res_text)
            .map_err(|e| AppError::ProviderError(format!("DigiFlazz JSON Parse Error: {}, Raw: {}", e, res_text)))?;

        if let Some(data) = digi_res.data {
            let status_raw = data.status.as_deref().unwrap_or("Pending").to_lowercase();
            let msg = data.message.unwrap_or_else(|| "Success".to_string());
            let sn = data.sn.unwrap_or_default();
            let trxid = data.ref_id.unwrap_or_else(|| ref_id.to_string());

            let filtered_msg = if msg.to_lowercase().contains("saldo") || msg.to_lowercase().contains("balance") {
                "Server API bermasalah".to_string()
            } else {
                msg
            };

            if status_raw == "gagal" || status_raw == "failed" {
                Ok(DigiResult {
                    success: false,
                    is_pending: false,
                    is_failed: true,
                    trxid: String::new(),
                    sn: String::new(),
                    message: filtered_msg,
                })
            } else if status_raw == "sukses" || status_raw == "success" {
                Ok(DigiResult {
                    success: true,
                    is_pending: false,
                    is_failed: false,
                    trxid,
                    sn,
                    message: filtered_msg,
                })
            } else {
                // Pending / Processing
                Ok(DigiResult {
                    success: true, // Berhasil di-submit ke supplier
                    is_pending: true,
                    is_failed: false,
                    trxid,
                    sn,
                    message: filtered_msg,
                })
            }
        } else {
            Ok(DigiResult {
                success: false,
                is_pending: false,
                is_failed: true,
                trxid: String::new(),
                sn: String::new(),
                message: "DigiFlazz response data empty".to_string(),
            })
        }
    }

    pub fn generate_status_signature(&self) -> String {
        let raw = format!("{}{}{}", self.username, self.apikey, "status");
        let mut hasher = Md5::new();
        hasher.update(raw.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    pub async fn check_status(
        &self,
        client: &Client,
        sku_code: &str,
        customer_no: &str,
        ref_id: &str,
    ) -> Result<DigiResult, AppError> {
        let sign = self.generate_status_signature();
        let payload = serde_json::json!({
            "username": self.username,
            "buyer_sku_code": sku_code,
            "customer_no": customer_no,
            "ref_id": ref_id,
            "commands": "status",
            "sign": sign
        });

        let response = client
            .post(&self.endpoint)
            .json(&payload)
            .send()
            .await?;

        let res_text = response.text().await?;
        let digi_res: DigiResponse = serde_json::from_str(&res_text)
            .map_err(|e| AppError::ProviderError(format!("DigiFlazz CheckStatus Parse Error: {}, Raw: {}", e, res_text)))?;

        if let Some(data) = digi_res.data {
            let status_raw = data.status.as_deref().unwrap_or("Pending").to_lowercase();
            let msg = data.message.unwrap_or_else(|| "Status Checked".to_string());
            let sn = data.sn.unwrap_or_default();
            let trxid = data.ref_id.unwrap_or_else(|| ref_id.to_string());

            let filtered_msg = if msg.to_lowercase().contains("saldo") || msg.to_lowercase().contains("balance") {
                "Server API bermasalah".to_string()
            } else {
                msg
            };

            if status_raw == "sukses" || status_raw == "success" {
                Ok(DigiResult {
                    success: true,
                    is_pending: false,
                    is_failed: false,
                    trxid,
                    sn,
                    message: filtered_msg,
                })
            } else if status_raw == "gagal" || status_raw == "failed" {
                Ok(DigiResult {
                    success: false,
                    is_pending: false,
                    is_failed: true,
                    trxid: String::new(),
                    sn: String::new(),
                    message: filtered_msg,
                })
            } else {
                // Pending / Process
                Ok(DigiResult {
                    success: false,
                    is_pending: true,
                    is_failed: false,
                    trxid,
                    sn,
                    message: filtered_msg,
                })
            }
        } else {
            Ok(DigiResult {
                success: false,
                is_pending: true,
                is_failed: false,
                trxid: String::new(),
                sn: String::new(),
                message: "Status check pending response".to_string(),
            })
        }
    }
}
