pub mod password;
pub mod protocol;
mod store;

use crate::{
    http::{middleware::AdminUser, response},
    state::AppState,
};
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use tower_http::set_header::SetResponseHeaderLayer;

#[derive(Debug)]
pub struct Error(StatusCode, &'static str);
impl From<sea_orm::DbErr> for Error {
    fn from(_: sea_orm::DbErr) -> Self {
        Self(StatusCode::SERVICE_UNAVAILABLE, "agent_storage_unavailable")
    }
}
impl From<serde_json::Error> for Error {
    fn from(_: serde_json::Error) -> Self {
        Self(StatusCode::SERVICE_UNAVAILABLE, "agent_state_invalid")
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/agent/desired", get(desired))
        .route("/v1/agent/status", post(status))
        .route("/v1/agent/password/claim", post(password_claim))
        .route("/v1/agent/password/result", post(password_result))
        .route(
            "/api/admin/agent/:id/password",
            post(password_create).get(password_status),
        )
        .route("/api/admin/agent/create", post(create))
        .route("/api/admin/agent/:id/bind", post(bind))
        .route("/api/admin/agent/:id/revoke", post(revoke))
        .route("/api/admin/agent/:id/rotate", post(rotate))
        .route("/api/admin/agent/peer/:id", get(peer_status))
        .layer(DefaultBodyLimit::max(65536))
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("no-store"),
        ))
}
fn token(headers: &HeaderMap) -> &str {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
}
fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, Error> {
    serde_json::from_slice(body)
        .map_err(|_| Error(StatusCode::BAD_REQUEST, "invalid_agent_payload"))
}
async fn desired(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Error> {
    Ok(match store::poll(&state.db, token(&headers)).await? {
        Some(desired) => Json(desired).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    })
}
async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, Error> {
    store::report(&state.db, token(&headers), parse(&body)?).await?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    peer_row_id: i32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bind {
    agent_uuid: String,
}
async fn create(
    State(state): State<AppState>,
    admin: AdminUser,
    body: Bytes,
) -> Result<Response, Error> {
    let input: Create = parse(&body)?;
    let (id, credential) = store::create(&state.db, input.peer_row_id).await?;
    tracing::info!(
        admin_id = admin.user.id,
        agent_id = id,
        "agent credential issued"
    );
    Ok(response::success(json!({"id":id,"credential":credential})))
}
async fn bind(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i32>,
    body: Bytes,
) -> Result<Response, Error> {
    let input: Bind = parse(&body)?;
    store::bind(&state.db, id, &input.agent_uuid).await?;
    tracing::info!(admin_id = admin.user.id, agent_id = id, "agent UUID bound");
    Ok(response::success(json!(null)))
}
async fn revoke(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i32>,
) -> Result<Response, Error> {
    store::revoke(&state.db, id).await?;
    tracing::info!(
        admin_id = admin.user.id,
        agent_id = id,
        "agent credential revoked"
    );
    Ok(response::success(json!(null)))
}
async fn peer_status(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<i32>,
) -> Result<Response, Error> {
    Ok(response::success(store::status(&state.db, id).await?))
}

async fn rotate(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i32>,
) -> Result<Response, Error> {
    let credential = store::rotate(&state.db, id).await?;
    tracing::info!(
        admin_id = admin.user.id,
        agent_id = id,
        "agent credential rotated"
    );
    Ok(response::success(json!({"id":id,"credential":credential})))
}

#[cfg(test)]
mod tests;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PasswordInput {
    password: zeroize::Zeroizing<String>,
}
async fn password_create(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i32>,
    body: Bytes,
) -> Result<Response, Error> {
    let input: PasswordInput = parse(&body)?;
    let key = password::Key::load()?;
    let result = password::create(&state.db, &key, id, admin.user.id, &input.password).await?;
    tracing::info!(
        admin_id = admin.user.id,
        agent_id = id,
        operation_id = result["operation_id"].as_str(),
        "RustDesk password operation queued"
    );
    Ok(response::success(result))
}
async fn password_status(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<i32>,
) -> Result<Response, Error> {
    Ok(response::success(
        json!({"available":password::Key::load().is_ok(),"operation":password::status(&state.db,id).await?}),
    ))
}
async fn password_claim(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, Error> {
    let key = password::Key::load().ok();
    Ok(
        match password::claim(&state.db, key.as_ref(), token(&headers)).await? {
            Some(value) => Json(value).into_response(),
            None => StatusCode::NO_CONTENT.into_response(),
        },
    )
}
async fn password_result(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, Error> {
    password::finish(&state.db, token(&headers), parse(&body)?).await?;
    Ok(StatusCode::NO_CONTENT)
}
