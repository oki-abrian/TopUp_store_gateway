use serde::{Deserialize, Serialize};
use sqlx::MySqlPool;

#[derive(Debug, Deserialize, Serialize)]
pub struct AdminConfigWebsiteRequest {
    pub title: String,
    pub keyword: String,
    pub description: String,
    pub banner: String,
    pub icon: String,
    pub navbar: String,
}

pub async fn get_website_config(db: &MySqlPool) -> Result<serde_json::Value, crate::error::AppError> {
    use sqlx::Row;
    let mut config_map = serde_json::Map::new();
    let rows = sqlx::query("SELECT parameter, content FROM config WHERE name = 'webcfg'")
        .fetch_all(db)
        .await?;

    for row in rows {
        let param: String = row.try_get("parameter").unwrap_or_default();
        let content: String = row.try_get("content").unwrap_or_default();
        match param.as_str() {
            "1" => config_map.insert("title".to_string(), serde_json::Value::String(content)),
            "2" => config_map.insert("navbar".to_string(), serde_json::Value::String(content)),
            "3" => config_map.insert("description".to_string(), serde_json::Value::String(content)),
            "4" => config_map.insert("keyword".to_string(), serde_json::Value::String(content)),
            "5" => config_map.insert("banner".to_string(), serde_json::Value::String(content)),
            "6" => config_map.insert("icon".to_string(), serde_json::Value::String(content)),
            _ => None,
        };
    }
    
    Ok(serde_json::Value::Object(config_map))
}

pub async fn update_website_config(db: &MySqlPool, req: AdminConfigWebsiteRequest) -> Result<(), crate::error::AppError> {
    sqlx::query("UPDATE config SET content = ? WHERE name = 'webcfg' AND parameter = '1'").bind(&req.title).execute(db).await?;
    sqlx::query("UPDATE config SET content = ? WHERE name = 'webcfg' AND parameter = '2'").bind(&req.navbar).execute(db).await?;
    sqlx::query("UPDATE config SET content = ? WHERE name = 'webcfg' AND parameter = '3'").bind(&req.description).execute(db).await?;
    sqlx::query("UPDATE config SET content = ? WHERE name = 'webcfg' AND parameter = '4'").bind(&req.keyword).execute(db).await?;
    sqlx::query("UPDATE config SET content = ? WHERE name = 'webcfg' AND parameter = '5'").bind(&req.banner).execute(db).await?;
    sqlx::query("UPDATE config SET content = ? WHERE name = 'webcfg' AND parameter = '6'").bind(&req.icon).execute(db).await?;
    Ok(())
}
