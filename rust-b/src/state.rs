use std::{env, path::PathBuf};

#[derive(Clone)]
pub struct AppState {
    pub root_dir: PathBuf,
    pub api_key: Option<String>,
    pub redis_url: Option<String>,
    pub public_host: String,
}

pub fn build_state() -> AppState {
    let root_dir = env::var("ROOT_DIR").unwrap_or_else(|_| "./storage".to_string());
    let api_key = env::var("API_KEY").ok().filter(|v| !v.is_empty());
    let redis_url = build_redis_url();
    let public_host = env::var("PUBLIC_HOST").unwrap_or_else(|_| "localhost".to_string());
    AppState {
        root_dir: PathBuf::from(root_dir),
        api_key,
        redis_url,
        public_host,
    }
}

pub fn build_redis_url() -> Option<String> {
    let host = env::var("REDIS_HOST").ok();
    let port = env::var("REDIS_PORT").ok();
    let password = env::var("REDIS_PASSWORD").ok();
    let h = host.unwrap_or_else(|| "localhost".to_string());
    let p = port.unwrap_or_else(|| "6379".to_string());
    if let Some(pass) = password.filter(|v| !v.is_empty()) {
        Some(format!("redis://:{}@{}:{}/", pass, h, p))
    } else {
        Some(format!("redis://{}:{}/", h, p))
    }
}

pub fn port_from_env() -> u16 {
    env::var("PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(3001)
}