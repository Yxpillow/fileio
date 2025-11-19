use std::{env, fs, path::{Path, PathBuf}};

use axum::{
    body::Body,
    extract::{Path as AxPath, State, Multipart},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::{get, post, delete},
    Router,
};
use serde::{Deserialize, Serialize};
use redis::AsyncCommands;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use std::time::Duration;
use tokio::fs as tokio_fs;
use tokio_util::io::ReaderStream;
use tracing::{info, error};

#[derive(Clone)]
struct AppState {
    root_dir: PathBuf,
    api_key: Option<String>,
    redis_url: Option<String>,
    public_host: String,
}

#[derive(Serialize)]
struct BucketInfo {
    name: String,
    size: u64,
    created: String,
    modified: String,
    #[serde(rename = "fileCount")]
    file_count: usize,
}

#[derive(Serialize)]
struct BucketsResponse {
    buckets: Vec<BucketInfo>,
}

#[derive(Deserialize)]
struct CreateBucketReq { name: String }

#[derive(Serialize)]
struct UploadFileResp {
    success: bool,
    file: FileInfo,
}

#[derive(Serialize)]
struct FileInfo {
    name: String,
    #[serde(rename = "originalName")]
    original_name: String,
    size: u64,
    path: String,
    bucket: String,
}

#[derive(Serialize)]
struct FilesListResp {
    files: Vec<FileInfoShort>,
    bucket: String,
}

#[derive(Serialize)]
struct FileInfoShort {
    name: String,
    size: u64,
    created: String,
    modified: String,
    bucket: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    dotenvy::dotenv().ok();
    let root_dir = env::var("ROOT_DIR").unwrap_or_else(|_| "./storage".to_string());
    let api_key = env::var("API_KEY").ok().filter(|v| !v.is_empty());
    let port: u16 = env::var("PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(3001);
    let redis_url = build_redis_url();
    let public_host = env::var("PUBLIC_HOST").unwrap_or_else(|_| "localhost".to_string());

    ensure_dir(Path::new(&root_dir))?;

    let state = AppState { root_dir: PathBuf::from(root_dir), api_key, redis_url, public_host };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let authed = Router::new()
        .route("/api/buckets", get(list_buckets).post(create_bucket))
        .route("/api/buckets/:bucket", delete(delete_bucket))
        .route("/api/buckets/:bucket/files", get(list_files))
        .route("/api/buckets/:bucket/upload", post(upload_file))
        .route("/api/buckets/:bucket/files/:filename", get(download_file).delete(delete_file))
        .route("/api/buckets/:bucket/files/:filename/info", get(file_info))
        .route("/api/nodes/register", post(register_node))
        .route("/api/nodes", get(list_nodes))
        .route_layer(axum::middleware::from_fn_with_state(state.clone(), auth_middleware))
        .with_state(state.clone());

    let app = Router::new()
        .route("/health", get(health))
        .route("/health/status", get(health_status))
        .merge(authed)
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    info!(%addr, "starting fileio-b on");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = heartbeat_task().await;
    });
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown_rx))
        .await?;
    Ok(())
}

async fn auth_middleware(
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

#[derive(serde::Deserialize)]
struct NodeRegisterReq { id: Option<String>, host: Option<String>, port: Option<u16> }

async fn health() -> impl IntoResponse { axum::Json(serde_json::json!({"status":"ok"})) }

async fn health_status(State(state): State<AppState>) -> impl IntoResponse {
    let redis = match &state.redis_url {
        Some(url) => match redis_ping(url).await { Ok(true) => serde_json::json!({"connected":true}), Ok(false) => serde_json::json!({"connected":false}), Err(e) => serde_json::json!({"error": e.to_string()}) },
        None => serde_json::json!({"disabled": true}),
    };
    axum::Json(serde_json::json!({"status":"ok","redis":redis})).into_response()
}

async fn register_node(State(state): State<AppState>, payload: Option<axum::Json<NodeRegisterReq>>) -> impl IntoResponse {
    let id = payload.as_ref().and_then(|p| p.id.clone()).unwrap_or_else(|| format!("server-{}", std::process::id()));
    let host = payload.as_ref().and_then(|p| p.host.clone()).unwrap_or_else(|| state.public_host.clone());
    let port = payload.as_ref().and_then(|p| p.port).unwrap_or_else(|| port_from_env());
    if let Some(url) = &state.redis_url {
        let node = serde_json::json!({"id": id, "host": host, "port": port}).to_string();
        let _ = register_node_with_url(url, &node).await;
    }
    axum::Json(serde_json::json!({"success": true})).into_response()
}

async fn list_nodes(State(state): State<AppState>) -> impl IntoResponse {
    if let Some(url) = &state.redis_url {
        if let Ok(members) = list_nodes_with_url(url).await {
            let nodes: Vec<serde_json::Value> = members.into_iter().filter_map(|s| serde_json::from_str(&s).ok()).collect();
            return axum::Json(serde_json::json!({"nodes": nodes})).into_response();
        }
    }
    axum::Json(serde_json::json!({"nodes": []})).into_response()
}

async fn register_node_with_url(url: &str, node_json: &str) -> anyhow::Result<()> {
    let client = redis::Client::open(url.to_string())?;
    let mut conn = client.get_multiplexed_async_connection().await?;
    let _: () = redis::AsyncCommands::sadd(&mut conn, "nodes", node_json).await?;
    Ok(())
}

async fn list_nodes_with_url(url: &str) -> anyhow::Result<Vec<String>> {
    let client = redis::Client::open(url.to_string())?;
    let mut conn = client.get_multiplexed_async_connection().await?;
    let members: Vec<String> = redis::AsyncCommands::smembers(&mut conn, "nodes").await?;
    Ok(members)
}

fn ensure_dir(path: &Path) -> anyhow::Result<()> {
    if !path.exists() {
        fs::create_dir_all(path)?;
    }
    Ok(())
}

async fn list_buckets(State(state): State<AppState>) -> impl IntoResponse {
    let mut buckets = Vec::new();
    match fs::read_dir(&state.root_dir) {
        Ok(rd) => {
            for entry in rd.filter_map(Result::ok) {
                let bucket_name = entry.file_name().to_string_lossy().to_string();
                let bucket_path = entry.path();
                if bucket_path.is_dir() {
                    let meta = match fs::metadata(&bucket_path) { Ok(m) => m, Err(_) => continue };
                    let mut size: u64 = 0;
                    let mut file_count: usize = 0;
                    if let Ok(files_iter) = fs::read_dir(&bucket_path) {
                        for f in files_iter.filter_map(Result::ok) {
                            if let Ok(m) = fs::metadata(f.path()) {
                                if m.is_file() { size += m.len(); file_count += 1; }
                            }
                        }
                    }
                    buckets.push(BucketInfo {
                        name: bucket_name,
                        size,
                        created: format_time(meta.created().ok()),
                        modified: format_time(meta.modified().ok()),
                        file_count: file_count,
                    });
                }
            }
            axum::Json(BucketsResponse { buckets }).into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(serde_json::json!({"error":"无法读取储存桶目录"}))).into_response(),
    }
}

async fn create_bucket(State(state): State<AppState>, axum::Json(payload): axum::Json<CreateBucketReq>) -> impl IntoResponse {
    let name = payload.name;
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, axum::Json(serde_json::json!({"error":"储存桶名称不能为空"}))).into_response();
    }
    let valid = name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-') && !name.ends_with('-');
    if !valid { 
        return (StatusCode::BAD_REQUEST, axum::Json(serde_json::json!({"error":"储存桶名称只能包含小写字母、数字和连字符，且不能以连字符开头或结尾"}))).into_response();
    }
    let bucket_dir = state.root_dir.join(&name);
    if bucket_dir.exists() {
        return (StatusCode::CONFLICT, axum::Json(serde_json::json!({"error":"储存桶已存在"}))).into_response();
    }
    if let Err(e) = fs::create_dir_all(&bucket_dir) { 
        return (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(serde_json::json!({"error":"创建储存桶失败","details":e.to_string()}))).into_response(); 
    }
    axum::Json(serde_json::json!({"success":true, "bucket": {"name": name}})).into_response()
}

async fn delete_bucket(State(state): State<AppState>, AxPath(bucket): AxPath<String>) -> impl IntoResponse {
    let bucket_dir = state.root_dir.join(&bucket);
    if !bucket_dir.exists() { 
        return (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({"error":"储存桶不存在"}))).into_response(); 
    }
    match fs::remove_dir_all(&bucket_dir) {
        Ok(_) => axum::Json(serde_json::json!({"success": true, "message": "储存桶已成功删除"})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(serde_json::json!({"error":"删除储存桶失败","details":e.to_string()}))).into_response(),
    }
}

async fn list_files(State(state): State<AppState>, AxPath(bucket): AxPath<String>) -> impl IntoResponse {
    let bucket_dir = state.root_dir.join(&bucket);
    if !bucket_dir.exists() { 
        return (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({"error":"储存桶不存在"}))).into_response(); 
    }
    let mut files = Vec::new();
    match fs::read_dir(&bucket_dir) {
        Ok(iter) => {
            for entry in iter.filter_map(Result::ok) {
                let p = entry.path();
                if let Ok(m) = fs::metadata(&p) { if m.is_file() {
                    files.push(FileInfoShort {
                        name: entry.file_name().to_string_lossy().to_string(),
                        size: m.len(),
                        created: format_time(m.created().ok()),
                        modified: format_time(m.modified().ok()),
                        bucket: bucket.clone(),
                    });
                }}
            }
            axum::Json(FilesListResp { files, bucket }).into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(serde_json::json!({"error":"无法读取文件目录"}))).into_response(),
    }
}

async fn upload_file(State(state): State<AppState>, AxPath(bucket): AxPath<String>, mut multipart: Multipart) -> impl IntoResponse {
    let bucket_dir = state.root_dir.join(&bucket);
    if let Err(e) = fs::create_dir_all(&bucket_dir) { 
        return (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(serde_json::json!({"error":"创建储存桶失败","details":e.to_string()}))).into_response(); 
    }

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().map(|s| s.to_string()).unwrap_or_else(|| "file".to_string());
        if name != "file" { continue; }
        let original_name = field.file_name().map(|s| s.to_string()).unwrap_or_else(|| "upload.bin".to_string());
        let unique = format!("{}-{}-{}", chrono::Utc::now().timestamp_millis(), rand_u32(), original_name);
        let save_path = bucket_dir.join(&unique);
        let bytes = match field.bytes().await { Ok(b) => b, Err(e) => {
            return (StatusCode::BAD_REQUEST, axum::Json(serde_json::json!({"error":"文件读取失败","details":e.to_string()}))).into_response();
        }};
        if let Err(e) = tokio_fs::write(&save_path, &bytes).await { 
            return (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(serde_json::json!({"error":"文件保存失败","details":e.to_string()}))).into_response(); 
        }
        let size = bytes.len() as u64;
        let resp = UploadFileResp { success: true, file: FileInfo { name: unique.clone(), original_name: original_name, size, path: save_path.to_string_lossy().to_string(), bucket: bucket.clone() } };

        if let Some(url) = &state.redis_url {
            let key = format!("{}:{}", bucket, unique);
            let value = serde_json::json!({
                "id": format!("server-{}", std::process::id()),
                "host": state.public_host,
                "port": port_from_env(),
            }).to_string();
            let _ = set_redis_key(url, &key, &value).await;
        }
        return axum::Json(resp).into_response();
    }
    (StatusCode::BAD_REQUEST, axum::Json(serde_json::json!({"error":"没有文件被上传"}))).into_response()
}

async fn download_file(State(state): State<AppState>, AxPath((bucket, filename)): AxPath<(String, String)>) -> impl IntoResponse {
    let file_path = state.root_dir.join(&bucket).join(&filename);
    if !file_path.exists() { 
        if let Some(url) = &state.redis_url {
            let key = format!("{}:{}", bucket, filename);
            if let Ok(Some(loc)) = get_redis_key(url, &key).await {
                if let Ok(obj) = serde_json::from_str::<serde_json::Value>(&loc) {
                    if let (Some(host), Some(port)) = (obj.get("host").and_then(|v| v.as_str()), obj.get("port").and_then(|v| v.as_u64())) {
                        let target = format!("http://{}:{}/api/buckets/{}/files/{}", host, port, bucket, filename);
                        return axum::response::Redirect::to(&target).into_response();
                    }
                }
            }
        }
        return (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({"error":"文件不存在"}))).into_response(); 
    }
    match tokio_fs::File::open(&file_path).await {
        Ok(file) => {
            let stream = ReaderStream::new(file);
            let body = Body::from_stream(stream);
            let mut headers = HeaderMap::new();
            headers.insert(header::CONTENT_DISPOSITION, format!("attachment; filename=\"{}\"", filename).parse().unwrap());
            (StatusCode::OK, headers, body).into_response()
        }
        Err(e) => {
            error!(error=?e, "open file failed");
            (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(serde_json::json!({"error":"服务器内部错误"}))).into_response()
        }
    }
}

async fn delete_file(State(state): State<AppState>, AxPath((bucket, filename)): AxPath<(String, String)>) -> impl IntoResponse {
    let file_path = state.root_dir.join(&bucket).join(&filename);
    if !file_path.exists() {
        return (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({"error":"文件不存在"}))).into_response(); 
    }
    match fs::remove_file(&file_path) {
        Ok(_) => {
            if let Some(url) = &state.redis_url {
                let key = format!("{}:{}", bucket, filename);
                let _ = del_redis_key(url, &key).await;
            }
            axum::Json(serde_json::json!({"message":"文件删除成功"})).into_response()
        },
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(serde_json::json!({"error": format!("文件删除失败: {}", e)}))).into_response(),
    }
}

async fn file_info(State(state): State<AppState>, AxPath((bucket, filename)): AxPath<(String, String)>) -> impl IntoResponse {
    let file_path = state.root_dir.join(&bucket).join(&filename);
    match fs::metadata(&file_path) {
        Ok(m) => {
            let mut obj = serde_json::json!({
                "filename": filename,
                "size": m.len(),
                "createdAt": format_time(m.created().ok()),
                "modifiedAt": format_time(m.modified().ok()),
                "bucket": bucket,
            });
            if let Some(url) = &state.redis_url {
                let key = format!("{}:{}", bucket, filename);
                if let Ok(Some(loc)) = get_redis_key(url, &key).await {
                    obj["location"] = serde_json::from_str::<serde_json::Value>(&loc).unwrap_or(serde_json::Value::Null);
                }
            }
            axum::Json(obj).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({"error":"文件不存在"}))).into_response(),
    }
}

async fn set_redis_key(url: &str, key: &str, value: &str) -> anyhow::Result<()> {
    let client = redis::Client::open(url)?;
    let mut conn = client.get_multiplexed_async_connection().await?;
    conn.set::<_, _, ()>(key, value).await?;
    Ok(())
}

async fn get_redis_key(url: &str, key: &str) -> anyhow::Result<Option<String>> {
    let client = redis::Client::open(url)?;
    let mut conn = client.get_multiplexed_async_connection().await?;
    let res: Option<String> = conn.get(key).await?;
    Ok(res)
}

async fn del_redis_key(url: &str, key: &str) -> anyhow::Result<()> {
    let client = redis::Client::open(url)?;
    let mut conn = client.get_multiplexed_async_connection().await?;
    let _: () = conn.del(key).await?;
    Ok(())
}

fn port_from_env() -> u16 {
    env::var("PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(3001)
}

fn format_time(t: Option<std::time::SystemTime>) -> String {
    match t.and_then(|x| x.duration_since(std::time::UNIX_EPOCH).ok()) {
        Some(d) => format!("{}", d.as_secs()),
        None => "0".into(),
    }
}

fn rand_u32() -> u32 { 
    use rand::RngCore; 
    let mut rng = rand::rngs::OsRng; 
    rng.next_u32() 
}
fn build_redis_url() -> Option<String> {
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

async fn redis_ping(url: &str) -> anyhow::Result<bool> {
    let client = redis::Client::open(url)?;
    let mut conn = client.get_multiplexed_async_connection().await?;
    let res: String = redis::cmd("PING").query_async(&mut conn).await?;
    Ok(res.to_uppercase() == "PONG")
}

async fn heartbeat_task() {
    loop {
        tokio::time::sleep(Duration::from_secs(10)).await;
        tracing::info!("heartbeat");
    }
}

async fn shutdown_signal(mut rx: tokio::sync::oneshot::Receiver<()>) {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::SignalKind;
        let mut sigterm = tokio::signal::unix::signal(SignalKind::terminate()).expect("sigterm");
        sigterm.recv().await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = terminate => {}, _ = &mut rx => {} }
}