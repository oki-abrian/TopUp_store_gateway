use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub server_port: u16,
    pub server_host: String,
    pub database_url: String,
    pub redis_url: String,
    pub hold_member: f64,
    pub hold_reseller: f64,
    pub hold_admin: f64,
    pub trx_interval_seconds: u64,
    pub mpwa_api_key: String,
    pub mpwa_sender_phone: String,
}

impl Config {
    pub fn from_env() -> Self {
        dotenvy::dotenv().ok();

        Self {
            server_port: env::var("SERVER_PORT")
                .unwrap_or_else(|_| "8080".to_string())
                .parse()
                .expect("SERVER_PORT must be a number"),
            server_host: env::var("SERVER_HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            database_url: env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
            redis_url: env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string()),
            hold_member: env::var("HOLD_BALANCE_MEMBER")
                .unwrap_or_else(|_| "0".to_string())
                .parse()
                .unwrap_or(0.0),
            hold_reseller: env::var("HOLD_BALANCE_RESELLER")
                .unwrap_or_else(|_| "50000".to_string())
                .parse()
                .unwrap_or(50000.0),
            hold_admin: env::var("HOLD_BALANCE_ADMIN")
                .unwrap_or_else(|_| "100000".to_string())
                .parse()
                .unwrap_or(100000.0),
            trx_interval_seconds: env::var("TRX_INTERVAL_SECONDS")
                .unwrap_or_else(|_| "10".to_string())
                .parse()
                .unwrap_or(10),
            mpwa_api_key: env::var("MPWA_API_KEY").unwrap_or_default(),
            mpwa_sender_phone: env::var("MPWA_SENDER_PHONE").unwrap_or_default(),
        }
    }

    pub fn get_hold_balance(&self, level: &str) -> f64 {
        match level {
            "Reseller" => self.hold_reseller,
            "Admin" => self.hold_admin,
            _ => self.hold_member,
        }
    }
}
