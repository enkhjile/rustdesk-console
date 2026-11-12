use super::{
    protocol::{Desired, Report, Settings},
    Error,
};
use ::entity::{companion_agent as agent, peer};
use axum::http::StatusCode;
use rand::RngCore;
use sea_orm::sea_query::Expr;
use sea_orm::*;
use sha2::{Digest, Sha256};

type Result<T> = std::result::Result<T, Error>;
fn hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}
fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
fn conflict() -> Error {
    Error(StatusCode::CONFLICT, "agent_state_conflict")
}
fn enrollment_error(error: DbErr) -> Error {
    if matches!(error.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) {
        conflict()
    } else {
        error.into()
    }
}
fn unauthorized() -> Error {
    Error(StatusCode::UNAUTHORIZED, "invalid_agent_credential")
}
fn counter(s: &str) -> Result<u64> {
    s.parse()
        .map_err(|_| Error(StatusCode::SERVICE_UNAVAILABLE, "invalid_stored_counter"))
}
fn new_credential() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    format!(
        "rda_{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    )
}

pub async fn create(db: &DatabaseConnection, peer_row_id: i32) -> Result<(i32, String)> {
    if peer::Entity::find_by_id(peer_row_id)
        .one(db)
        .await?
        .is_none()
    {
        return Err(Error(StatusCode::NOT_FOUND, "device_not_found"));
    }
    if agent::Entity::find()
        .filter(agent::Column::PeerRowId.eq(peer_row_id))
        .one(db)
        .await?
        .is_some()
    {
        return Err(conflict());
    }
    let credential = new_credential();
    let row = agent::ActiveModel {
        peer_row_id: Set(peer_row_id),
        agent_uuid: Set(None),
        token_hash: Set(hash(&credential)),
        revoked: Set(false),
        revision: Set("0".into()),
        desired_json: Set("null".into()),
        sequence: Set("0".into()),
        report_json: Set(None),
        last_seen: Set(0),
        ..Default::default()
    }
    .insert(db)
    .await
    .map_err(enrollment_error)?;
    Ok((row.id, credential))
}

fn canonical_uuid(uuid: &str) -> Result<String> {
    let parsed = uuid::Uuid::parse_str(uuid)
        .map_err(|_| Error(StatusCode::BAD_REQUEST, "invalid_agent_uuid"))?;
    if parsed.get_version_num() != 4 || parsed.get_variant() != uuid::Variant::RFC4122 {
        return Err(Error(StatusCode::BAD_REQUEST, "invalid_agent_uuid"));
    }
    Ok(parsed.hyphenated().to_string())
}

pub async fn bind(db: &DatabaseConnection, id: i32, uuid: &str) -> Result<()> {
    let uuid = canonical_uuid(uuid)?;
    let tx = db.begin().await?;
    let row = lock_agent(&tx, id).await?;
    if row.revoked || row.agent_uuid.as_ref().is_some_and(|v| v != &uuid) {
        return Err(conflict());
    }
    if agent::Entity::find()
        .filter(agent::Column::AgentUuid.eq(&uuid))
        .filter(agent::Column::Id.ne(id))
        .one(&tx)
        .await?
        .is_some()
    {
        return Err(conflict());
    }
    let mut row: agent::ActiveModel = row.into();
    row.agent_uuid = Set(Some(uuid));
    row.update(&tx).await.map_err(enrollment_error)?;
    tx.commit().await?;
    Ok(())
}

pub async fn revoke(db: &DatabaseConnection, id: i32) -> Result<()> {
    let tx = db.begin().await?;
    lock_agent(&tx, id).await?;
    agent::Entity::update_many()
        .col_expr(agent::Column::Revoked, Expr::value(true))
        .filter(agent::Column::Id.eq(id))
        .exec(&tx)
        .await?;
    use ::entity::agent_password as password;
    password::Entity::update_many()
        .col_expr(
            password::Column::Ciphertext,
            Expr::value(Option::<String>::None),
        )
        .filter(password::Column::AgentId.eq(id))
        .exec(&tx)
        .await?;
    password::Entity::update_many()
        .col_expr(password::Column::Status, Expr::value("expired"))
        .filter(password::Column::AgentId.eq(id))
        .filter(password::Column::Status.eq("queued"))
        .exec(&tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn rotate(db: &DatabaseConnection, id: i32) -> Result<String> {
    let tx = db.begin().await?;
    let row = lock_agent(&tx, id).await?;
    let credential = new_credential();
    let mut row: agent::ActiveModel = row.into();
    row.token_hash = Set(hash(&credential));
    row.revoked = Set(false);
    row.update(&tx).await?;
    tx.commit().await?;
    Ok(credential)
}

// A no-op UPDATE obtains the database's write/row lock (including SQLite).
// All protocol reads and mutations follow it in the same transaction, so two
// requests cannot reuse a revision/sequence with different content.
pub(super) async fn lock_agent(tx: &DatabaseTransaction, id: i32) -> Result<agent::Model> {
    agent::Entity::update_many()
        .col_expr(agent::Column::Id, Expr::col(agent::Column::Id).into())
        .filter(agent::Column::Id.eq(id))
        .exec(tx)
        .await?;
    agent::Entity::find_by_id(id)
        .one(tx)
        .await?
        .ok_or(Error(StatusCode::NOT_FOUND, "agent_not_found"))
}
pub(super) async fn authenticate(
    db: &DatabaseConnection,
    token: &str,
) -> Result<(DatabaseTransaction, agent::Model)> {
    let (tx, row) = authenticate_credential(db, token).await?;
    if row.agent_uuid.is_none() {
        return Err(unauthorized());
    }
    Ok((tx, row))
}

async fn authenticate_credential(
    db: &DatabaseConnection,
    token: &str,
) -> Result<(DatabaseTransaction, agent::Model)> {
    if token.len() != 68
        || !token.starts_with("rda_")
        || !token[4..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(unauthorized());
    }
    let digest = hash(token);
    let tx = db.begin().await?;
    agent::Entity::update_many()
        .col_expr(agent::Column::Id, Expr::col(agent::Column::Id).into())
        .filter(agent::Column::TokenHash.eq(&digest))
        .exec(&tx)
        .await?;
    let row = agent::Entity::find()
        .filter(agent::Column::TokenHash.eq(digest))
        .one(&tx)
        .await?
        .ok_or_else(unauthorized)?;
    if row.revoked {
        return Err(unauthorized());
    }
    Ok((tx, row))
}

pub async fn enroll(db: &DatabaseConnection, token: &str, uuid: &str) -> Result<()> {
    let (tx, row) = authenticate_credential(db, token).await?;
    enabled_peer(&tx, row.peer_row_id).await?;
    let uuid = canonical_uuid(uuid)?;
    if row.agent_uuid.as_ref().is_some_and(|value| value != &uuid) {
        return Err(conflict());
    }
    if row.agent_uuid.is_none() {
        let mut row: agent::ActiveModel = row.into();
        row.agent_uuid = Set(Some(uuid));
        row.update(&tx).await.map_err(enrollment_error)?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn enrollment_ready(db: &DatabaseConnection, token: &str) -> Result<bool> {
    let (tx, row) = authenticate(db, token).await?;
    enabled_peer(&tx, row.peer_row_id).await?;
    Ok(row.report_json.is_some())
}

async fn enabled_peer(tx: &DatabaseTransaction, peer_row_id: i32) -> Result<peer::Model> {
    let peer = peer::Entity::find_by_id(peer_row_id)
        .one(tx)
        .await?
        .ok_or_else(unauthorized)?;
    if peer.status != 1 {
        return Err(unauthorized());
    }
    Ok(peer)
}

async fn desired(tx: &DatabaseTransaction, row: &mut agent::Model) -> Result<Option<Desired>> {
    let peer = enabled_peer(tx, row.peer_row_id).await?;
    let settings = match crate::services::strategy::effective_for_peer(tx, &peer).await? {
        Some(strategy) => Settings::from_options(&strategy.config_options)
            .map_err(|_| Error(StatusCode::SERVICE_UNAVAILABLE, "invalid_agent_strategy"))?,
        None => None,
    };
    let serialized = serde_json::to_string(&settings)?;
    let mut revision = counter(&row.revision)?;
    if serialized != row.desired_json {
        if settings.is_some() {
            revision = revision
                .checked_add(1)
                .ok_or(Error(StatusCode::SERVICE_UNAVAILABLE, "revision_exhausted"))?;
        }
        row.revision = revision.to_string();
        row.desired_json = serialized;
    }
    row.last_seen = now();
    Ok(settings.map(|settings| Desired {
        schema_version: 1,
        revision,
        settings,
    }))
}
async fn save(tx: DatabaseTransaction, row: agent::Model) -> Result<()> {
    let updated: agent::ActiveModel = row.into();
    updated.reset_all().update(&tx).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn poll(db: &DatabaseConnection, token: &str) -> Result<Option<Desired>> {
    let (tx, mut row) = authenticate(db, token).await?;
    let desired = desired(&tx, &mut row).await?;
    save(tx, row).await?;
    Ok(desired)
}

pub async fn report(db: &DatabaseConnection, token: &str, report: Report) -> Result<()> {
    let (tx, mut row) = authenticate(db, token).await?;
    if row.agent_uuid.as_deref() != Some(&report.agent_uuid) {
        return Err(Error(StatusCode::FORBIDDEN, "agent_uuid_mismatch"));
    }
    let wanted = desired(&tx, &mut row).await?;
    if report.desired_revision != wanted.as_ref().map(|state| state.revision) {
        return Err(conflict());
    }
    if !report.valid_for(wanted.as_ref().map(|state| &state.settings)) {
        return Err(Error(StatusCode::BAD_REQUEST, "invalid_agent_report"));
    }
    let body = serde_json::to_string(&report)?;
    let previous = counter(&row.sequence)?;
    if report.observation_sequence < previous
        || (report.observation_sequence == previous && row.report_json.as_deref() != Some(&body))
    {
        return Err(conflict());
    }
    row.sequence = report.observation_sequence.to_string();
    row.report_json = Some(body);
    save(tx, row).await
}

pub async fn status(db: &DatabaseConnection, peer_id: i32) -> Result<serde_json::Value> {
    let Some(row) = agent::Entity::find()
        .filter(agent::Column::PeerRowId.eq(peer_id))
        .one(db)
        .await?
    else {
        return Ok(serde_json::Value::Null);
    };
    let report = row
        .report_json
        .as_deref()
        .map(serde_json::from_str::<serde_json::Value>)
        .transpose()?;
    let reported_revision = report
        .as_ref()
        .and_then(|r| r["desired_revision"].as_u64())
        .map(|r| r.to_string());
    let policy = match peer::Entity::find_by_id(peer_id).one(db).await? {
        Some(peer) => crate::services::strategy::effective_for_peer(db, &peer).await?,
        None => None,
    };
    let parsed = policy
        .as_ref()
        .map(|p| Settings::from_options(&p.config_options))
        .transpose();
    // A bad policy must not hide enrollment status or administrator recovery controls.
    let policy_invalid = parsed.is_err();
    let settings = parsed.ok().flatten().flatten();
    let settings_json = serde_json::to_string(&settings)?;
    Ok(serde_json::json!({
            "id": row.id, "peer_row_id": row.peer_row_id, "agent_uuid": row.agent_uuid,
            "revoked": row.revoked, "desired_revision": row.revision,
            "desired_settings": settings, "policy_pending": policy_invalid || settings_json != row.desired_json,
            "policy_invalid": policy_invalid,
            "strategy_id": policy.map(|p| p.id),
            "observation_sequence": row.sequence, "last_seen": row.last_seen,
            "reported_revision": reported_revision, "report": report
    }))
}
