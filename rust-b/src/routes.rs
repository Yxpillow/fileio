use axum::{routing::{get, post, delete}, Router};
use tower_http::cors::{Any, CorsLayer};

use crate::state::AppState;
use crate::auth::auth_middleware;
use crate::handlers::{list_buckets, create_bucket, delete_bucket, list_files, upload_file, download_file, delete_file, file_info, health, register_node_endpoint, list_nodes_endpoint};

pub fn build_router(state: AppState) -> Router {
    let cors = CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any);
    let authed = Router::new()
        .route("/api/buckets", get(list_buckets).post(create_bucket))
        .route("/api/buckets/:bucket", delete(delete_bucket))
        .route("/api/buckets/:bucket/files", get(list_files))
        .route("/api/buckets/:bucket/upload", post(upload_file))
        .route("/api/buckets/:bucket/files/:filename", get(download_file).delete(delete_file))
        .route("/api/buckets/:bucket/files/:filename/info", get(file_info))
        .route("/api/nodes/register", post(register_node_endpoint))
        .route("/api/nodes", get(list_nodes_endpoint))
        .route_layer(axum::middleware::from_fn_with_state(state.clone(), auth_middleware))
        .with_state(state.clone());
    Router::new()
        .route("/health", get(health))
        .merge(authed)
        .layer(cors)
        .with_state(state)
}