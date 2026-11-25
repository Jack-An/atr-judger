use crate::judge::{BatchOutcome, BatchRequest, Engine, Outcome, Registry, Request};
use crate::sandbox::Sandbox;
use axum::extract::{DefaultBodyLimit, FromRequest, Request as HttpRequest, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Serialize, de::DeserializeOwned};
use std::env;
use std::io::{self, Error};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use subtle::ConstantTimeEq;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

struct App {
    token: String,
    slots: Arc<Semaphore>,
    engine: Engine,
}

pub fn serve() -> io::Result<()> {
    let token =
        env::var("RUST_JUDGER_TOKEN").map_err(|_| Error::other("RUST_JUDGER_TOKEN is required"))?;
    if !(32..=256).contains(&token.len()) || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(Error::other(
            "RUST_JUDGER_TOKEN must be 32 to 256 visible ASCII bytes",
        ));
    }
    let concurrency = env::var("RUST_JUDGER_CONCURRENCY")
        .unwrap_or_else(|_| "4".to_string())
        .parse::<usize>()
        .map_err(Error::other)?;
    if !(1..=64).contains(&concurrency) {
        return Err(Error::other(
            "RUST_JUDGER_CONCURRENCY must be between 1 and 64",
        ));
    }
    let cgroup_path = env::var_os("RUST_JUDGER_CGROUP_ROOT").ok_or_else(|| {
        Error::other("RUST_JUDGER_CGROUP_ROOT must specify a dedicated cgroup v2 domain")
    })?;
    let runtime =
        PathBuf::from(env::var_os("RUST_JUDGER_RUNTIME_ROOT").unwrap_or_else(|| "/".into()));
    let runtime = runtime.canonicalize()?;
    let extensions = env::var_os("RUST_JUDGER_LANGUAGES").map(PathBuf::from);
    let registry = Registry::load(&runtime, extensions.as_deref())?;
    let engine = Engine::new(
        registry,
        Sandbox::new(runtime, &PathBuf::from(cgroup_path))?,
    );
    engine.probe()?;
    let address = env::var("RUST_JUDGER_BIND").unwrap_or_else(|_| "127.0.0.1:5060".into());
    let app = Router::new()
        .route("/readyz", get(ready))
        .route("/v1/languages", get(languages))
        .route("/v1/judge", post(judge))
        .route("/v1/judge/batch", post(judge_batch))
        .layer(DefaultBodyLimit::max(8 * 1024 * 1024))
        .with_state(Arc::new(App {
            token,
            slots: Arc::new(Semaphore::new(concurrency)),
            engine,
        }));

    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?
        .block_on(async move {
            let listener = tokio::net::TcpListener::bind(address).await?;
            eprintln!("atr-judger listening on {}", listener.local_addr()?);
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown())
                .await
        })
}

#[derive(Serialize)]
struct Health {
    version: &'static str,
    sandbox: &'static str,
}

async fn ready(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
) -> Result<Json<Health>, StatusCode> {
    if !authorized(&app, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(Json(Health {
        version: env!("CARGO_PKG_VERSION"),
        sandbox: "linux-cgroup-v2",
    }))
}

async fn languages(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
) -> Result<axum::response::Response, StatusCode> {
    use axum::response::IntoResponse;
    if !authorized(&app, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(Json(app.engine.languages.list()).into_response())
}

async fn judge(
    State(app): State<Arc<App>>,
    request: HttpRequest,
) -> Result<Json<Outcome>, StatusCode> {
    let (slot, request) = extract::<Request>(&app, request).await?;
    if !request.valid(&app.engine.languages) {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    run_job(slot, move || app.engine.judge(request)).await
}

async fn judge_batch(
    State(app): State<Arc<App>>,
    request: HttpRequest,
) -> Result<Json<BatchOutcome>, StatusCode> {
    let (slot, request) = extract::<BatchRequest>(&app, request).await?;
    if !request.valid(&app.engine.languages) {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    run_job(slot, move || app.engine.judge_batch(request)).await
}

async fn extract<T: DeserializeOwned>(
    app: &App,
    request: HttpRequest,
) -> Result<(OwnedSemaphorePermit, T), StatusCode> {
    if !authorized(app, request.headers()) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let slot = app
        .slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    let Json(request) = tokio::time::timeout(
        Duration::from_secs(10),
        Json::<T>::from_request(request, app),
    )
    .await
    .map_err(|_| StatusCode::REQUEST_TIMEOUT)?
    .map_err(|error| error.status())?;
    Ok((slot, request))
}

async fn run_job<T: Send + 'static>(
    slot: OwnedSemaphorePermit,
    job: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> Result<Json<T>, StatusCode> {
    // The job owns the permit and completes cleanup if the HTTP client disconnects.
    let result = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        job()
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    result.map(Json).map_err(|error| {
        eprintln!("Judger infrastructure failure ({:?})", error.kind());
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

fn authorized(app: &App, headers: &HeaderMap) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.as_bytes().strip_prefix(b"Bearer "))
        .is_some_and(|token| bool::from(app.token.as_bytes().ct_eq(token)))
}

async fn shutdown() {
    let Ok(mut signal) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    else {
        let _ = tokio::signal::ctrl_c().await;
        return;
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = signal.recv() => {},
    }
}
