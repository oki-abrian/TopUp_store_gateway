use crate::error::AppError;
use reqwest::Client;
use serde::Serialize;

#[derive(Serialize)]
struct MpwaPayload<'a> {
    api_key: &'a str,
    sender: &'a str,
    number: &'a str,
    message: &'a str,
}

pub struct MpwaClient {
    pub api_key: String,
    pub sender: String,
    pub endpoint: String,
}

impl MpwaClient {
    pub fn new(api_key: String, sender: String) -> Self {
        Self {
            api_key,
            sender,
            endpoint: "https://mpwa.byllann.com/send-message".to_string(),
        }
    }

    pub fn format_order_success_message(
        order_id: &str,
        service_name: &str,
        target_id: &str,
        price: f64,
        sn_token: &str,
    ) -> String {
        format!(
            "✅ *TRANSAKSI BERHASIL*\n\n\
             📦 *No. Order*: {}\n\
             🎮 *Produk*: {}\n\
             🎯 *Target ID*: {}\n\
             💰 *Harga*: Rp {}\n\
             🔑 *SN/Token*: {}\n\n\
             Terima kasih telah bertransaksi di Aruteru Shoppu!",
            order_id,
            service_name,
            target_id,
            price as u64,
            if sn_token.is_empty() { "-" } else { sn_token }
        )
    }

    pub async fn send_whatsapp_notification(
        &self,
        client: &Client,
        phone_number: &str,
        message: &str,
    ) -> Result<bool, AppError> {
        if self.api_key.is_empty() {
            return Ok(false);
        }

        let payload = MpwaPayload {
            api_key: &self.api_key,
            sender: &self.sender,
            number: phone_number,
            message,
        };

        let res = client
            .post(&self.endpoint)
            .json(&payload)
            .send()
            .await?;

        Ok(res.status().is_success())
    }
}
