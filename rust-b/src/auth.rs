use axum::{body::Body, http::StatusCode, response::IntoResponse};
use axum::extract::State;
use crate::state::AppState;

pub async fn auth_middleware(
    State(state): State<AppState>,
    req: axum::http::Request<Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if let Some(expected) = &state.api_key {
        if !expected.is_empty() {
            let headers = req.headers();
            match headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
                Some(got) if got == expected => {}
                _ => return (StatusCode::FORBIDDEN, axum::Json(serde_json::json!({"error":"无效的API密钥"}))).into_response(),
            }
        }
    }
    next.run(req).await
}