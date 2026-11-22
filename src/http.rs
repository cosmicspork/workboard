// Handlers return axum's `Response` as their error so `?` can answer early; boxing it
// would only add noise.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, RawQuery, Request, State, rejection::BytesRejection},
    http::{HeaderValue, Method, StatusCode, Uri, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{MethodFilter, MethodRouter, get, post},
};
use rust_embed::Embed;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use url::Url;

use crate::{
    jira, ops,
    store::{Store, StoreError},
};

#[derive(Clone)]
pub struct App {
    pub store: Arc<Store>,
    pub site: JiraSite,
    /// Where a refresh asks jira-review's `search`.
    pub jira: jira::Source,
    pub refresh_in_progress: Arc<AtomicBool>,
}

/// The Jira site a refresh checks jira-review's answer against.
#[derive(Clone, Debug)]
pub enum JiraSite {
    Configured(Url),
    /// Not configured; the board works and a refresh answers `hint`.
    Missing {
        hint: String,
    },
}

/// What the standalone server checks requests against.
#[derive(Clone)]
pub struct Local {
    /// `http://127.0.0.1:<port>`.
    pub origin: String,
}

#[derive(Embed)]
#[folder = "spa/dist"]
struct Assets;

type ApiResult<T> = Result<Json<T>, Response>;

pub(crate) fn error(status: StatusCode, code: &str, message: &str) -> Response {
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
        StoreError::NotFound => error(StatusCode::NOT_FOUND, "not_found", "Not found."),
        StoreError::Conflict => error(
            StatusCode::CONFLICT,
            "task_conflict",
            "This changed elsewhere. Reload it before saving.",
        ),
        StoreError::IdentityChanged => error(
            StatusCode::BAD_GATEWAY,
            "jira_identity_changed",
            "jira-review's Jira account or site changed from the one this board is bound to.",
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

/// Serve operation `op`: its arguments are the path parameters and the JSON
/// body, or for a `GET` the query string, run through the same
/// `ops::run` the tools use.
async fn serve_op(
    op: &'static ops::Op,
    app: App,
    params: Option<Path<HashMap<String, String>>>,
    query: Option<String>,
    bytes: Result<Bytes, BytesRejection>,
) -> Response {
    let args = match op_arguments(op, params, query, bytes) {
        Ok(args) => args,
        Err(response) => return response,
    };
    let store = app.store.clone();
    let refresh = app.refresh_in_progress.load(Ordering::Acquire);
    let ran = blocking(move || {
        let ctx = ops::Ctx {
            store: &store,
            refresh_in_progress: refresh,
        };
        ops::run(&ctx, op.name, &args)
    })
    .await;
    match ran {
        Err(response) => response,
        Ok(Err(err)) => {
            let (status, code, message) = err.parts();
            let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            error(status, code, &message)
        }
        Ok(Ok(value)) => {
            let status = StatusCode::from_u16(op.status).unwrap_or(StatusCode::OK);
            if status == StatusCode::NO_CONTENT {
                status.into_response()
            } else {
                (status, Json(value)).into_response()
            }
        }
    }
}

fn op_arguments(
    op: &'static ops::Op,
    params: Option<Path<HashMap<String, String>>>,
    query: Option<String>,
    bytes: Result<Bytes, BytesRejection>,
) -> Result<Value, Response> {
    let bad = |message: &str| error(StatusCode::BAD_REQUEST, "invalid_request", message);
    let bytes = body(bytes)?;
    let mut args = Map::new();
    if op.method == "GET" {
        for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
            if args.insert(key.to_string(), json!(value)).is_some() {
                return Err(bad(&format!("{key} is given more than once.")));
            }
        }
    } else {
        match parse::<Value>(&bytes)? {
            Value::Object(body) => args = body,
            _ => return Err(bad("Invalid JSON request.")),
        }
    }
    for (key, value) in params.map(|Path(p)| p).unwrap_or_default() {
        if args.get(&key).is_some_and(|given| given != &json!(value)) {
            return Err(bad(&format!("{key} is given by the path.")));
        }
        args.insert(key, json!(value));
    }
    Ok(Value::Object(args))
}

fn method_filter(method: &str) -> MethodFilter {
    match method {
        "GET" => MethodFilter::GET,
        "POST" => MethodFilter::POST,
        "PUT" => MethodFilter::PUT,
        "DELETE" => MethodFilter::DELETE,
        other => panic!("operation table names method {other}"),
    }
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
    let site = match &app.site {
        JiraSite::Configured(site) => site.clone(),
        JiraSite::Missing { hint } => {
            return Err(error(StatusCode::CONFLICT, "jira_site_missing", hint));
        }
    };
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
    // Plugin mode: jira-review's `search` through the node's `tools/call`, as
    // this plugin. Standalone: a `jira-review mcp` child (see `jira`).
    let outcome = jira::fetch(&app.jira, &site, bound).await;
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
            // The board keeps only the code; the plugin log keeps why.
            eprintln!("workboard: Jira refresh failed ({code}): {}", err.message);
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

pub(crate) async fn missing_api() -> Response {
    error(StatusCode::NOT_FOUND, "not_found", "API route not found.")
}
async fn fallback(uri: Uri) -> Response {
    if uri.path() == "/api" || uri.path().starts_with("/api/") {
        missing_api().await
    } else {
        asset(uri).await
    }
}

async fn guard(State(local): State<Arc<Local>>, request: Request, next: Next) -> Response {
    let is_api = request.uri().path().starts_with("/api");
    let ipv4_host = local.origin.strip_prefix("http://").unwrap_or("");
    let port = ipv4_host.strip_prefix("127.0.0.1:").unwrap_or("");
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok());
    let ipv4 = host == Some(ipv4_host);
    let localhost = host.and_then(|value| value.strip_prefix("localhost:")) == Some(port);
    let allowed_host = ipv4 || localhost;
    let mutation = !matches!(*request.method(), Method::GET | Method::HEAD);
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    let allowed_origin = (ipv4 && origin == Some(local.origin.as_str()))
        || (localhost
            && origin.and_then(|value| value.strip_prefix("http://localhost:")) == Some(port));
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

/// The REST API both modes serve, without any request checks: one route
/// per entry of the operation table, and the routes it lists as HTTP only.
pub(crate) fn api() -> Router<App> {
    let mut router = Router::new();
    let mut paths: Vec<&str> = Vec::new();
    for op in ops::OPS {
        if !paths.contains(&op.path) {
            paths.push(op.path);
        }
    }
    for path in paths {
        let mut methods = MethodRouter::new();
        for op in ops::OPS.iter().filter(|op| op.path == path) {
            methods = methods.on(
                method_filter(op.method),
                move |State(app): State<App>,
                      params: Option<Path<HashMap<String, String>>>,
                      RawQuery(query): RawQuery,
                      bytes: Result<Bytes, BytesRejection>| {
                    serve_op(op, app, params, query, bytes)
                },
            );
        }
        router = router.route(path, methods);
    }
    // Each in `ops::HTTP_ONLY`, with why it is not a tool.
    router.route("/api/jira/refresh", post(refresh))
}

/// The standalone server: the API and the embedded SPA, behind the loopback
/// Host and Origin checks.
pub fn router(app: App, local: Local) -> Router {
    api()
        .fallback(fallback)
        .route("/", get(asset))
        .route("/assets/{*path}", get(asset))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(middleware::from_fn_with_state(Arc::new(local), guard))
        .with_state(app)
}

/// The largest request body the API reads.
pub const BODY_LIMIT: usize = 64 * 1024;
