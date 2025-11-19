use std::{env, fs, io, path::{Path, PathBuf}};

use axum::{
    body::Body,
    extract::{Path as AxPath, State, Multipart},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::{get, post, delete},
    Router,
};
use serde::{Deserialize, Serialize};
use tower_http::cors::{Any, CorsLayer};
use tokio::fs as tokio_fs;
use tokio_util::io::ReaderStream;
use tracing::{info, error};

#[derive(Clone)]
struct AppState {
    root_dir: PathBuf,
    api_key: Option<String>,
}

#[derive(Serialize)]
struct BucketInfo {
    name: String,
    size: u64,
    created: String,
    modified: String,
    fileCount: usize,
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
    originalName: String,
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

    ensure_dir(Path::new(&root_dir))?;

    let state = AppState { root_dir: PathBuf::from(root_dir), api_key };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/api/buckets", get(list_buckets).post(create_bucket))
        .route("/api/buckets/:bucket", delete(delete_bucket))
        .route("/api/buckets/:bucket/files", get(list_files))
        .route("/api/buckets/:bucket/upload", post(upload_file))
        .route("/api/buckets/:bucket/files/:filename", get(download_file).delete(delete_file))
        .route("/api/buckets/:bucket/files/:filename/info", get(file_info))
        .route_layer(axum::middleware::from_fn_with_state(state.clone(), auth_middleware))
        .layer(cors)
        .with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    info!(%addr, "starting fileio-b on");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn auth_middleware(
    State(state): State<AppState>,
    req: axum::http::Request<Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    next.run(req).await
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
                        fileCount: file_count,
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
        let resp = UploadFileResp { success: true, file: FileInfo { name: unique.clone(), originalName: original_name, size, path: save_path.to_string_lossy().to_string(), bucket: bucket.clone() } };
        return axum::Json(resp).into_response();
    }
    (StatusCode::BAD_REQUEST, axum::Json(serde_json::json!({"error":"没有文件被上传"}))).into_response()
}

async fn download_file(State(state): State<AppState>, AxPath((bucket, filename)): AxPath<(String, String)>) -> impl IntoResponse {
    let file_path = state.root_dir.join(&bucket).join(&filename);
    if !file_path.exists() { 
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
        Ok(_) => axum::Json(serde_json::json!({"message":"文件删除成功"})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(serde_json::json!({"error": format!("文件删除失败: {}", e)}))).into_response(),
    }
}

async fn file_info(State(state): State<AppState>, AxPath((bucket, filename)): AxPath<(String, String)>) -> impl IntoResponse {
    let file_path = state.root_dir.join(&bucket).join(&filename);
    match fs::metadata(&file_path) {
        Ok(m) => {
            axum::Json(serde_json::json!({
                "filename": filename,
                "size": m.len(),
                "createdAt": format_time(m.created().ok()),
                "modifiedAt": format_time(m.modified().ok()),
                "bucket": bucket,
            })).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({"error":"文件不存在"}))).into_response(),
    }
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