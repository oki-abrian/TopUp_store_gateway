use crate::error::AppError;
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct GameCheckResult {
    pub success: bool,
    pub username: Option<String>,
    pub message: Option<String>,
}

pub struct DuniaGamesChecker;

impl DuniaGamesChecker {
    pub async fn check(
        client: &Client,
        user_id: &str,
        zone_id: &str,
        game_code: &str,
    ) -> Result<GameCheckResult, AppError> {
        let endpoint = format!(
            "https://cek.rizkydev.web.id/api/game/{}",
            game_code
        );

        let response = client
            .get(&endpoint)
            .query(&[("id", user_id), ("zone", zone_id)])
            .send()
            .await?;

        let body: serde_json::Value = response.json().await?;
        if body["result"].as_bool() == Some(true) || body["status"].as_bool() == Some(true) {
            let name = body["data"]["username"]
                .as_str()
                .or(body["data"].as_str())
                .unwrap_or("");
            Ok(GameCheckResult {
                success: true,
                username: Some(name.to_string()),
                message: None,
            })
        } else {
            Ok(GameCheckResult {
                success: false,
                username: None,
                message: Some(body["message"].as_str().unwrap_or("Account invalid").to_string()),
            })
        }
    }
}
