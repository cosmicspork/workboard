use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Request, State, rejection::BytesRejection},
    http::{HeaderValue, Method, StatusCode, Uri, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use rust_embed::Embed;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use url::Url;

use crate::{
    jira,
    model::{CreateTask, TriageInput, UpdateTask},
    store::{Store, StoreError},
};

#[derive(Clone)]
pub struct App {
    pub store: Arc<Store>,
    pub site: Url,
    pub acli: String,
    pub origin: String,
    pub refresh_in_progress: Arc<AtomicBool>,
}

#[derive(Embed)]
#[folder = "spa/dist"]
struct Assets;

type ApiResult<T> = Result<Json<T>, Response>;

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({"error": code, "message": message}))).into_response()
}

fn internal() -> Response {
    error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal_error",
        "Local data is unavailable.",
    )
}

fn store_error(err: StoreError) -> Response {
    match err {
        StoreError::Invalid(_) => {
            error(StatusCode::BAD_REQUEST, "invalid_request", &err.to_string())
        }
        StoreError::NotFound => error(StatusCode::NOT_FOUND, "not_found", "Task not found."),
        StoreError::Conflict => error(
            StatusCode::CONFLICT,
            "task_conflict",
            "Task changed. Reload it before saving.",
        ),
        StoreError::IdentityChanged => error(
            StatusCode::BAD_GATEWAY,
            "jira_identity_changed",
            "The Jira CLI account or site changed. Select the intended account.",
        ),
        StoreError::Internal(_) => internal(),
    }
}

fn parse<T: DeserializeOwned>(bytes: &Bytes) -> Result<T, Response> {
    serde_json::from_slice(bytes).map_err(|_| {
        error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Invalid JSON request.",
        )
    })
}
fn body(bytes: Result<Bytes, BytesRejection>) -> Result<Bytes, Response> {
    bytes.map_err(|rejection| {
        let status = rejection.status();
        error(
            status,
            "invalid_request",
            "Request body could not be read or exceeds 64 KiB.",
        )
    })
}

async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Response> {
    tokio::task::spawn_blocking(job)
        .await
        .map_err(|_| internal())
}

async fn board(State(app): State<App>) -> ApiResult<crate::model::Board> {
    let active = app.refresh_in_progress.load(Ordering::Acquire);
    let store = app.store.clone();
    let local_date = chrono::Local::now().date_naive().to_string();
    blocking(move || store.board(local_date, active))
        .await?
        .map(Json)
        .map_err(|_| internal())
}

async fn create(
    State(app): State<App>,
    bytes: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<crate::model::Task>), Response> {
    let bytes = body(bytes)?;
    let input: CreateTask = parse(&bytes)?;
    let store = app.store.clone();
    let task = blocking(move || store.create(input))
        .await?
        .map_err(store_error)?;
    Ok((StatusCode::CREATED, Json(task)))
}

async fn update(
    State(app): State<App>,
    Path(id): Path<String>,
    bytes: Result<Bytes, BytesRejection>,
) -> ApiResult<crate::model::Task> {
    let bytes = body(bytes)?;
    let input: UpdateTask = parse(&bytes)?;
    let store = app.store.clone();
    blocking(move || store.update(&id, input))
        .await?
        .map(Json)
        .map_err(store_error)
}

async fn triage(
    State(app): State<App>,
    bytes: Result<Bytes, BytesRejection>,
) -> ApiResult<Vec<crate::model::Task>> {
    let bytes = body(bytes)?;
    let input: TriageInput = parse(&bytes)?;
    let store = app.store.clone();
    blocking(move || store.triage(input))
        .await?
        .map(Json)
        .map_err(store_error)
}

struct RefreshGuard(Arc<AtomicBool>);
impl Drop for RefreshGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

async fn refresh(
    State(app): State<App>,
    bytes: Result<Bytes, BytesRejection>,
) -> ApiResult<crate::model::Sync> {
    let bytes = body(bytes)?;
    let value: Value = parse(&bytes)?;
    if value != json!({}) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Expected an empty JSON object.",
        ));
    }
    if app
        .refresh_in_progress
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(error(
            StatusCode::CONFLICT,
            "refresh_in_progress",
            "Jira refresh is already running.",
        ));
    }
    let _guard = RefreshGuard(app.refresh_in_progress.clone());
    let store = app.store.clone();
    let sync = blocking(move || store.sync())
        .await?
        .map_err(|_| internal())?;
    let bound = sync.site.as_deref().zip(sync.account_email.as_deref());
    let outcome = jira::fetch(&app.acli, &app.site, bound).await;
    let now = chrono::Utc::now().to_rfc3339();
    match outcome {
        Ok((identity, issues)) => {
            let store = app.store.clone();
            let applied = blocking(move || store.apply_refresh(&identity, &issues, &now)).await?;
            match applied {
                Ok(sync) => Ok(Json(sync)),
                Err(StoreError::IdentityChanged) => {
                    let store = app.store.clone();
                    let failed_at = chrono::Utc::now().to_rfc3339();
                    blocking(move || store.record_failure(&failed_at, "jira_identity_changed"))
                        .await?
                        .map_err(|_| internal())?;
                    Err(store_error(StoreError::IdentityChanged))
                }
                Err(err) => Err(store_error(err)),
            }
        }
        Err(err) => {
            let code = err.code;
            let store = app.store.clone();
            blocking(move || store.record_failure(&now, code))
                .await?
                .map_err(|_| internal())?;
            let status = if code == "jira_timeout" {
                StatusCode::GATEWAY_TIMEOUT
            } else {
                StatusCode::BAD_GATEWAY
            };
            Err(error(status, code, &err.message))
        }
    }
}

async fn asset(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match Assets::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            let cache = if path.starts_with("assets/") {
                "public, max-age=31536000, immutable"
            } else {
                "no-cache"
            };
            (
                [
                    (header::CONTENT_TYPE, mime.to_string()),
                    (header::CACHE_CONTROL, cache.to_string()),
                ],
                file.data,
            )
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn missing_api() -> Response {
    error(StatusCode::NOT_FOUND, "not_found", "API route not found.")
}
async fn fallback(uri: Uri) -> Response {
    if uri.path() == "/api" || uri.path().starts_with("/api/") {
        missing_api().await
    } else {
        asset(uri).await
    }
}

async fn guard(State(app): State<App>, request: Request, next: Next) -> Response {
    let is_api = request.uri().path().starts_with("/api");
    let host = app.origin.strip_prefix("http://").unwrap_or("");
    let allowed_host = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        == Some(host);
    let mutation = !matches!(*request.method(), Method::GET | Method::HEAD);
    let allowed_origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        == Some(app.origin.as_str());
    let foreign_origin = request.headers().contains_key(header::ORIGIN) && !allowed_origin;
    let mut response = if !allowed_host || foreign_origin || (mutation && !allowed_origin) {
        error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Use the local workboard URL.",
        )
    } else if mutation
        && is_api
        && !request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                v.eq_ignore_ascii_case("application/json")
                    || v.to_ascii_lowercase().starts_with("application/json;")
            })
    {
        error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "JSON Content-Type is required.",
        )
    } else {
        next.run(request).await
    };
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    if is_api {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

pub fn router(app: App) -> Router {
    Router::new()
        .route("/api/board", get(board))
        .route("/api/tasks", post(create))
        .route("/api/tasks/triage", post(triage))
        .route("/api/tasks/{id}", put(update))
        .route("/api/jira/refresh", post(refresh))
        .fallback(fallback)
        .route("/", get(asset))
        .route("/assets/{*path}", get(asset))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn_with_state(app.clone(), guard))
        .with_state(app)
}
