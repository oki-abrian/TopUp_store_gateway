use crate::error::AppError;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize)]
pub struct MLCheckResult {
    pub success: bool,
    pub username: Option<String>,
    pub region: Option<String>,
    pub message: Option<String>,
}

pub struct SmileOneChecker;

impl SmileOneChecker {
    pub fn map_region_name(code: &str) -> String {
        let mut map = HashMap::new();
        map.insert("ID", "Indonesia");
        map.insert("MY", "Malaysia");
        map.insert("SG", "Singapura");
        map.insert("PH", "Filipina");
        map.insert("TH", "Thailand");
        map.insert("VN", "Vietnam");
        map.insert("KR", "Korea");
        map.insert("JP", "Jepang");
        map.insert("TW", "Taiwan");
        map.insert("NA", "Amerika Utara");
        map.insert("EU", "Eropa");
        map.insert("SA", "Amerika Selatan");
        map.insert("BR", "Brasil");
        map.insert("RU", "Rusia");

        map.get(code).copied().unwrap_or(code).to_string()
    }

    pub async fn check_role(
        client: &Client,
        user_id: &str,
        zone_id: &str,
    ) -> Result<MLCheckResult, AppError> {
        let url = "https://www.smile.one/merchant/mobilelegends/checkrole/";
        let params = [
            ("user_id", user_id),
            ("zone_id", zone_id),
            ("pid", "25"),
            ("checkrole", "1"),
            ("pay_methond", ""),
            ("channel_method", ""),
        ];

        let response = client
            .post(url)
            .form(&params)
            .send()
            .await?;

        let body: serde_json::Value = response.json().await?;
        if body["status"].as_i64() == Some(200) || body["success"].as_bool() == Some(true) {
            let nickname = body["username"].as_str().or(body["data"]["username"].as_str()).unwrap_or("");
            let region_code = body["region"].as_str().or(body["data"]["region"].as_str()).unwrap_or("");
            let region_name = Self::map_region_name(region_code);

            Ok(MLCheckResult {
                success: true,
                username: Some(nickname.to_string()),
                region: Some(region_name),
                message: None,
            })
        } else {
            Ok(MLCheckResult {
                success: false,
                username: None,
                region: None,
                message: Some(body["message"].as_str().unwrap_or("Role not found").to_string()),
            })
        }
    }
}
