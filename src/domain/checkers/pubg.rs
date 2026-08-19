use crate::error::AppError;
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct PubgCheckResult {
    pub success: bool,
    pub username: Option<String>,
    pub message: Option<String>,
}

pub struct PubgChecker;

impl PubgChecker {
    pub async fn check(client: &Client, char_id: &str) -> Result<PubgCheckResult, AppError> {
        let endpoint = "https://cek-id-game.vercel.app/api/game/pubg-mobile-global-vc";

        let response = client
            .get(endpoint)
            .query(&[("id", char_id), ("zone", "")])
            .send()
            .await?;

        let body: serde_json::Value = response.json().await?;
        if body["status"].as_bool() == Some(true) {
            let nickname = body["data"]["username"].as_str().unwrap_or("");
            Ok(PubgCheckResult {
                success: true,
                username: Some(nickname.to_string()),
                message: None,
            })
        } else {
            Ok(PubgCheckResult {
                success: false,
                username: None,
                message: Some("PUBG Character ID invalid".to_string()),
            })
        }
    }
}
