// Copyright (c) 2023 Dark Energy Processor
//
// This software is provided 'as-is', without any express or implied
// warranty. In no event will the authors be held liable for any damages
// arising from the use of this software.
//
// Permission is granted to anyone to use this software for any purpose,
// including commercial applications, and to alter it and redistribute it
// freely, subject to the following restrictions:
//
// 1. The origin of this software must not be misrepresented; you must not
//    claim that you wrote the original software. If you use this software
//    in a product, an acknowledgment in the product documentation would be
//    appreciated but is not required.
// 2. Altered source versions must be plainly marked as such, and must not be
//    misrepresented as being the original software.
// 3. This notice may not be removed or altered from any source distribution.

use std::{collections::HashMap, sync::Arc};

use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use tower_http::{services::ServeDir, trace::TraceLayer};
use tracing::info;

use crate::config::Config;
use crate::file_handler::{sanitize_path, FileState};
use crate::models::*;

// ── Program version constants ──────────────────────────────────────────────────

const DLAPI_MAJOR_VERSION: u32 = 1;
const DLAPI_MINOR_VERSION: u32 = 1;
const NPPS4_DLAPI_PROGRAM_VERSION: (u32, u32, u32) = (2023, 5, 14);

// ── Application state ──────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    files: Arc<FileState>,
    git_commit: Arc<String>,
}

// ── Access control middleware ──────────────────────────────────────────────────

async fn verify_api_access(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();
    let shared_key = request
        .headers()
        .get("dlapi-shared-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    if !state.config.is_accessible(&path, shared_key.as_deref()) {
        return (
            StatusCode::NOT_FOUND,
            Json(ErrorResponseModel { detail: "Not found.".into() }),
        )
            .into_response();
    }

    next.run(request).await
}

/// Maximum number of files accepted in a single /api/v1/getfile request.
const GETFILE_MAX_FILES: usize = 1024;

// ── URL builder ────────────────────────────────────────────────────────────────

/// Build a full URL for a path under /archive-root/.
/// If `base_url` is configured in config, that takes precedence over the
/// incoming Host header (prevents Host header injection / cache poisoning).
fn archive_url(headers: &HeaderMap, base_url: Option<&str>, path: &str) -> String {
    let clean = path.trim_start_matches('/');
    if let Some(base) = base_url {
        return format!("{base}/archive-root/{clean}");
    }
    // Fallback: derive from request headers (only safe when not behind a cache)
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost:8000");
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    format!("{scheme}://{host}/archive-root/{clean}")
}

// ── Helper response builders ───────────────────────────────────────────────────

fn bad_request(msg: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponseModel { detail: msg.into() }),
    )
        .into_response()
}

fn internal_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponseModel { detail: "Internal server error".into() }),
    )
        .into_response()
}

/// Map platform integer (1=iOS, 2=Android) to zero-based index.
fn validate_platform(platform: u8) -> Option<usize> {
    match platform {
        1 => Some(0), // iOS
        2 => Some(1), // Android
        _ => None,
    }
}

// ── Route handlers ─────────────────────────────────────────────────────────────

/// GET /api/publicinfo
async fn publicinfo_handler(
    State(state): State<AppState>,
    _headers: HeaderMap,
) -> impl IntoResponse {
    let (major, minor) = match state.files.get_latest_version() {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("get_latest_version: {e}");
            return internal_error();
        }
    };

    let mut application = HashMap::new();
    application.insert("NPPS4DLAPICommit".into(), state.git_commit.as_ref().clone());
    application.insert(
        "NPPS4DLAPIVersion".into(),
        format!(
            "{}.{:02}.{:02}",
            NPPS4_DLAPI_PROGRAM_VERSION.0,
            NPPS4_DLAPI_PROGRAM_VERSION.1,
            NPPS4_DLAPI_PROGRAM_VERSION.2
        ),
    );

    let info = PublicInfoModel {
        public_api: state.config.is_public_accessible(),
        dlapi_version: VersionModel {
            major: DLAPI_MAJOR_VERSION,
            minor: DLAPI_MINOR_VERSION,
        },
        serve_time_limit: 0,
        game_version: format!("{major}.{minor}"),
        application,
    };

    (StatusCode::OK, Json(info)).into_response()
}

/// POST /api/v1/update
async fn update_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(param): Json<UpdateRequest>,
) -> impl IntoResponse {
    let platform = match validate_platform(param.platform) {
        Some(p) => p,
        None => return bad_request("Invalid platform"),
    };

    match state.files.get_update_file(&param.version, platform) {
        Ok(mut downloads) => {
            for d in &mut downloads {
                d.url = archive_url(&headers, state.config.base_url.as_deref(), &d.url);
            }
            (StatusCode::OK, Json(downloads)).into_response()
        }
        Err(e) => {
            tracing::error!("get_update_file: {e}");
            internal_error()
        }
    }
}

/// POST /api/v1/batch
async fn batch_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(param): Json<BatchDownloadRequest>,
) -> impl IntoResponse {
    let platform = match validate_platform(param.platform) {
        Some(p) => p,
        None => return bad_request("Invalid platform"),
    };

    match state.files.get_batch_list(param.package_type, platform, &param.exclude) {
        Ok(Some(mut downloads)) => {
            for d in &mut downloads {
                d.url = archive_url(&headers, state.config.base_url.as_deref(), &d.url);
            }
            (StatusCode::OK, Json(downloads)).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponseModel { detail: "Package type not found".into() }),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("get_batch_list: {e}");
            internal_error()
        }
    }
}

/// POST /api/v1/download
async fn download_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(param): Json<DownloadRequest>,
) -> impl IntoResponse {
    let platform = match validate_platform(param.platform) {
        Some(p) => p,
        None => return bad_request("Invalid platform"),
    };

    match state.files.get_single_package(param.package_type, param.package_id, platform) {
        Ok(Some(mut downloads)) => {
            for d in &mut downloads {
                d.url = archive_url(&headers, state.config.base_url.as_deref(), &d.url);
            }
            (StatusCode::OK, Json(downloads)).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponseModel { detail: "Package not found".into() }),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("get_single_package: {e}");
            internal_error()
        }
    }
}

/// GET /api/v1/getdb/:name
async fn getdb_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    match state.files.get_database_file(&name) {
        Ok(Some(data)) => {
            let db_name: String = name
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/vnd.sqlite3")
                .header(
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{db_name}.db_\""),
                )
                .body(Body::from(data))
                .unwrap()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponseModel { detail: "Database not found".into() }),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("get_database_file: {e}");
            internal_error()
        }
    }
}

/// POST /api/v1/getfile
async fn getfile_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(param): Json<MicroDownloadRequest>,
) -> impl IntoResponse {
    let platform = match validate_platform(param.platform) {
        Some(p) => p,
        None => return bad_request("Invalid platform"),
    };

    if param.files.len() > GETFILE_MAX_FILES {
        return bad_request(&format!("Too many files requested (max {GETFILE_MAX_FILES})"));
    }

    let mut results = Vec::with_capacity(param.files.len());
    for file_path in &param.files {
        match state.files.get_microdl_file(file_path, platform) {
            Ok(mut info) => {
                info.url = archive_url(&headers, state.config.base_url.as_deref(), &info.url);
                results.push(info);
            }
            Err(e) => {
                tracing::error!("get_microdl_file({file_path}): {e}");
                return internal_error();
            }
        }
    }

    (StatusCode::OK, Json(results)).into_response()
}

/// GET /api/v1/release_info
async fn release_info_handler(State(state): State<AppState>) -> impl IntoResponse {
    match state.files.get_release_info() {
        Ok(map) => (StatusCode::OK, Json(map)).into_response(),
        Err(e) => {
            tracing::error!("get_release_info: {e}");
            internal_error()
        }
    }
}

// ── Legacy /v7 micro-download route ───────────────────────────────────────────

/// GET /v7/micro_download/:platform/:version/*file_path
///
/// Reverse-maps the old v7 CDN path format to the current archive-root layout:
///   /v7/micro_download/{platform}/{version}/{file}
///   → {archive_root}/{Platform}/package/{version}/microdl/{file}
async fn v7_microdl_handler(
    State(state): State<AppState>,
    Path((platform, version, file_path)): Path<(String, String, String)>,
) -> Response {
    let platform_dir = match platform.to_lowercase().as_str() {
        "android" => "Android",
        "ios" => "iOS",
        _ => {
            return (StatusCode::NOT_FOUND, Json(ErrorResponseModel { detail: "Not found.".into() }))
                .into_response();
        }
    };

    // Sanitize version: only digits and dots
    let version_clean: String = version.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();

    let sanitized_file = sanitize_path(&file_path);
    if sanitized_file.is_empty() {
        return (StatusCode::NOT_FOUND, Json(ErrorResponseModel { detail: "Not found.".into() }))
            .into_response();
    }

    let fs_path = state.files.archive_root
        .join(platform_dir)
        .join("package")
        .join(&version_clean)
        .join("microdl")
        .join(&sanitized_file);

    match tokio::fs::read(&fs_path).await {
        Ok(data) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(Body::from(data))
            .unwrap(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (StatusCode::NOT_FOUND, Json(ErrorResponseModel { detail: "Not found.".into() }))
                .into_response()
        }
        Err(_) => internal_error(),
    }
}

// ── Entry point ────────────────────────────────────────────────────────────────

pub async fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "n4dlapi=info,tower_http=info".parse().unwrap()),
        )
        .init();

    let config = Config::load()?;
    let archive_root = config.archive_root.clone();

    info!("Archive root: {}", archive_root.display());

    let git_commit = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    info!("Git commit: {git_commit}");
    info!(
        "DLAPI version: {DLAPI_MAJOR_VERSION}.{DLAPI_MINOR_VERSION}, program version: {}.{:02}.{:02}",
        NPPS4_DLAPI_PROGRAM_VERSION.0,
        NPPS4_DLAPI_PROGRAM_VERSION.1,
        NPPS4_DLAPI_PROGRAM_VERSION.2,
    );

    let state = AppState {
        config: Arc::new(config),
        files: Arc::new(FileState::new(archive_root.clone())),
        git_commit: Arc::new(git_commit),
    };

    // API routes with access control middleware
    let api_routes = Router::new()
        .route("/api/publicinfo", get(publicinfo_handler))
        .route("/api/v1/update", post(update_handler))
        .route("/api/v1/batch", post(batch_handler))
        .route("/api/v1/download", post(download_handler))
        .route("/api/v1/getdb/:name", get(getdb_handler))
        .route("/api/v1/getfile", post(getfile_handler))
        .route("/api/v1/release_info", get(release_info_handler))
        .layer(middleware::from_fn_with_state(state.clone(), verify_api_access));

    // Static file serving for the CDN archive root
    let static_routes = Router::new()
        .nest_service("/archive-root", ServeDir::new(&archive_root))
        .route("/v7/micro_download/:platform/:version/*file_path", get(v7_microdl_handler));

    let app = Router::new()
        .merge(api_routes)
        .merge(static_routes)
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listen_addr = std::env::var("N4DLAPI_LISTEN")
        .unwrap_or_else(|_| "127.0.0.1:8000".to_string());
    let listener = tokio::net::TcpListener::bind(&listen_addr).await?;
    info!("Listening on http://{listen_addr}");

    axum::serve(listener, app).await?;
    Ok(())
}
