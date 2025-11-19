use std::fs;
use std::path::PathBuf;
use axum::{body::Body, extract::{Path as AxPath, State, Multipart}, http::{HeaderMap, StatusCode, header}, response::IntoResponse};
use serde::{Deserialize, Serialize};

use crate::state::{AppState, port_from_env};
use crate::util::{ensure_dir, format_time, rand_u32};
use crate::redis::{set_key, get_key, del_key, register_node, list_nodes};

#[derive(Serialize)]
pub struct BucketInfo { pub name: String, pub size: u64, pub created: String, pub modified: String, pub fileCount: usize }

#[derive(Serialize)]
pub struct BucketsResponse { pub buckets: Vec<BucketInfo> }

#[derive(Deserialize)]
pub struct CreateBucketReq { pub name: String }

#[derive(Serialize)]
pub struct UploadFileResp { pub success: bool, pub file: FileInfo }

#[derive(Serialize)]
pub struct FileInfo { pub name: String, pub originalName: String, pub size: u64, pub path: String, pub bucket: String }

#[derive(Serialize)]
pub struct FilesListResp { pub files: Vec<FileInfoShort>, pub bucket: String }

#[derive(Serialize)]
pub struct FileInfoShort { pub name: String, pub size: u64, pub created: String, pub modified: String, pub bucket: String }

pub async fn list_buckets(State(state): State<AppState>) -> impl IntoResponse {
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
                            if let Ok(m) = fs::metadata(f.path()) { if m.is_file() { size += m.len(); file_count += 1; } }
                        }
                    }
                    buckets.push(BucketInfo { name: bucket_name, size, created: format_time(meta.created().ok()), modified: format_time(meta.modified().ok()), fileCount: file_count });
                }
            }
            axum::Json(BucketsResponse { buckets }).into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(serde_json::json!({"error":"无法读取储存桶目录"}))).into_response(),
    }
}

pub async fn create_bucket(State(state): State<AppState>, axum::Json(payload): axum::Json<CreateBucketReq>) -> impl IntoResponse {
    let name = payload.name;
    if name.is_empty() { return (StatusCode::BAD_REQUEST, axum::Json(serde_json::json!({"error":"储存桶名称不能为空"}))).into_response(); }
    let valid = name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && !name.starts_with('-') && !name.ends_with('-');
    if !valid { return (StatusCode::BAD_REQUEST, axum::Json(serde_json::json!({"error":"储存桶名称只能包含小写字母、数字和连字符，且不能以连字符开头或结尾"}))).into_response(); }
    let bucket_dir = state.root_dir.join(&name);
    if bucket_dir.exists() { return (StatusCode::CONFLICT, axum::Json(serde_json::json!({"error":"储存桶已存在"}))).into_response(); }
    if let Err(e) = fs::create_dir_all(&bucket_dir) { return (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(serde_json::json!({"error":"创建储存桶失败","details":e.to_string()}))).into_response(); }
    axum::Json(serde_json::json!({"success":true, "bucket": {"name": name}})).into_response()
}

pub async fn delete_bucket(State(state): State<AppState>, AxPath(bucket): AxPath<String>) -> impl IntoResponse {
    let bucket_dir = state.root_dir.join(&bucket);
    if !bucket_dir.exists() { return (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({"error":"储存桶不存在"}))).into_response(); }
    match fs::remove_dir_all(&bucket_dir) {
        Ok(_) => axum::Json(serde_json::json!({"success": true, "message": "储存桶已成功删除"})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(serde_json::json!({"error":"删除储存桶失败","details":e.to_string()}))).into_response(),
    }
}

pub async fn list_files(State(state): State<AppState>, AxPath(bucket): AxPath<String>) -> impl IntoResponse {
    let bucket_dir = state.root_dir.join(&bucket);
    if !bucket_dir.exists() { return (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({"error":"储存桶不存在"}))).into_response(); }
    let mut files = Vec::new();
    match fs::read_dir(&bucket_dir) {
        Ok(iter) => {
            for entry in iter.filter_map(Result::ok) {
                let p = entry.path();
                if let Ok(m) = fs::metadata(&p) { if m.is_file() {
                    files.push(FileInfoShort { name: entry.file_name().to_string_lossy().to_string(), size: m.len(), created: format_time(m.created().ok()), modified: format_time(m.modified().ok()), bucket: bucket.clone() });
                }}
            }
            axum::Json(FilesListResp { files, bucket }).into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(serde_json::json!({"error":"无法读取文件目录"}))).into_response(),
    }
}

pub async fn upload_file(State(state): State<AppState>, AxPath(bucket): AxPath<String>, mut multipart: Multipart) -> impl IntoResponse {
    let bucket_dir = state.root_dir.join(&bucket);
    if let Err(e) = fs::create_dir_all(&bucket_dir) { return (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(serde_json::json!({"error":"创建储存桶失败","details":e.to_string()}))).into_response(); }
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().map(|s| s.to_string()).unwrap_or_else(|| "file".to_string());
        if name != "file" { continue; }
        let original_name = field.file_name().map(|s| s.to_string()).unwrap_or_else(|| "upload.bin".to_string());
        let unique = format!("{}-{}-{}", chrono::Utc::now().timestamp_millis(), rand_u32(), original_name);
        let save_path = bucket_dir.join(&unique);
        let bytes = match field.bytes().await { Ok(b) => b, Err(e) => { return (StatusCode::BAD_REQUEST, axum::Json(serde_json::json!({"error":"文件读取失败","details":e.to_string()}))).into_response(); }};
        if let Err(e) = tokio::fs::write(&save_path, &bytes).await { return (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(serde_json::json!({"error":"文件保存失败","details":e.to_string()}))).into_response(); }
        let size = bytes.len() as u64;
        let resp = UploadFileResp { success: true, file: FileInfo { name: unique.clone(), originalName: original_name, size, path: save_path.to_string_lossy().to_string(), bucket: bucket.clone() } };
        if let Some(url) = &state.redis_url { let key = format!("{}:{}", bucket, unique); let value = serde_json::json!({"id": format!("server-{}", std::process::id()), "host": state.public_host, "port": port_from_env()}).to_string(); let _ = set_key(url, &key, &value).await; }
        return axum::Json(resp).into_response();
    }
    (StatusCode::BAD_REQUEST, axum::Json(serde_json::json!({"error":"没有文件被上传"}))).into_response()
}

pub async fn download_file(State(state): State<AppState>, AxPath((bucket, filename)): AxPath<(String, String)>) -> impl IntoResponse {
    let file_path = state.root_dir.join(&bucket).join(&filename);
    if !file_path.exists() {
        if let Some(url) = &state.redis_url { let key = format!("{}:{}", bucket, filename); if let Ok(Some(loc)) = get_key(url, &key).await { if let Ok(obj) = serde_json::from_str::<serde_json::Value>(&loc) { if let (Some(host), Some(port)) = (obj.get("host").and_then(|v| v.as_str()), obj.get("port").and_then(|v| v.as_u64())) { let target = format!("http://{}:{}/api/buckets/{}/files/{}", host, port, bucket, filename); return axum::response::Redirect::to(&target).into_response(); } } } }
        return (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({"error":"文件不存在"}))).into_response();
    }
    match tokio::fs::File::open(&file_path).await {
        Ok(file) => { let stream = tokio_util::io::ReaderStream::new(file); let body = Body::from_stream(stream); let mut headers = HeaderMap::new(); headers.insert(header::CONTENT_DISPOSITION, format!("attachment; filename=\"{}\"", filename).parse().unwrap()); (StatusCode::OK, headers, body).into_response() }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(serde_json::json!({"error":"服务器内部错误"}))).into_response(),
    }
}

pub async fn delete_file(State(state): State<AppState>, AxPath((bucket, filename)): AxPath<(String, String)>) -> impl IntoResponse {
    let file_path = state.root_dir.join(&bucket).join(&filename);
    if !file_path.exists() { return (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({"error":"文件不存在"}))).into_response(); }
    match fs::remove_file(&file_path) {
        Ok(_) => { if let Some(url) = &state.redis_url { let key = format!("{}:{}", bucket, filename); let _ = del_key(url, &key).await; } axum::Json(serde_json::json!({"message":"文件删除成功"})).into_response() }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(serde_json::json!({"error": format!("文件删除失败: {}", e)}))).into_response(),
    }
}

pub async fn file_info(State(state): State<AppState>, AxPath((bucket, filename)): AxPath<(String, String)>) -> impl IntoResponse {
    let file_path = state.root_dir.join(&bucket).join(&filename);
    match fs::metadata(&file_path) {
        Ok(m) => {
            let mut obj = serde_json::json!({"filename": filename, "size": m.len(), "createdAt": format_time(m.created().ok()), "modifiedAt": format_time(m.modified().ok()), "bucket": bucket});
            if let Some(url) = &state.redis_url { let key = format!("{}:{}", bucket, filename); if let Ok(Some(loc)) = get_key(url, &key).await { obj["location"] = serde_json::from_str::<serde_json::Value>(&loc).unwrap_or(serde_json::Value::Null); } }
            axum::Json(obj).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({"error":"文件不存在"}))).into_response(),
    }
}

#[derive(Deserialize)]
pub struct NodeRegisterReq { pub id: Option<String>, pub host: Option<String>, pub port: Option<u16> }

pub async fn health() -> impl IntoResponse { axum::Json(serde_json::json!({"status":"ok"})) }

pub async fn register_node_endpoint(State(state): State<AppState>, payload: Option<axum::Json<NodeRegisterReq>>) -> impl IntoResponse {
    let id = payload.as_ref().and_then(|p| p.id.clone()).unwrap_or_else(|| format!("server-{}", std::process::id()));
    let host = payload.as_ref().and_then(|p| p.host.clone()).unwrap_or_else(|| state.public_host.clone());
    let port = payload.as_ref().and_then(|p| p.port).unwrap_or_else(|| port_from_env());
    if let Some(url) = &state.redis_url { let node = serde_json::json!({"id": id, "host": host, "port": port}).to_string(); let _ = register_node(url, &node).await; }
    axum::Json(serde_json::json!({"success": true})).into_response()
}

pub async fn list_nodes_endpoint(State(state): State<AppState>) -> impl IntoResponse {
    if let Some(url) = &state.redis_url { if let Ok(members) = list_nodes(url).await { let nodes: Vec<serde_json::Value> = members.into_iter().filter_map(|s| serde_json::from_str(&s).ok()).collect(); return axum::Json(serde_json::json!({"nodes": nodes})).into_response(); } }
    axum::Json(serde_json::json!({"nodes": []})).into_response()
}