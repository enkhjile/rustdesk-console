//! Dedicated encrypted password operations, never normal policies/reports.
use super::{store, Error};
use ::entity::{agent_password as operation, peer};
use axum::http::StatusCode;
use base64::{engine::general_purpose::STANDARD, Engine};
use ring::{
    aead,
    rand::{SecureRandom, SystemRandom},
};
use sea_orm::{sea_query::Expr, *};
use serde::Deserialize;
use serde_json::{json, Value};
use zeroize::Zeroizing;

type Result<T> = std::result::Result<T, Error>;
fn unavailable() -> Error {
    Error(StatusCode::SERVICE_UNAVAILABLE, "password_key_unavailable")
}
fn conflict() -> Error {
    Error(StatusCode::CONFLICT, "password_operation_pending")
}

pub struct Key(aead::LessSafeKey);
impl Key {
    pub fn load() -> Result<Self> {
        let text = Zeroizing::new(
            std::env::var("RUSTDESK_AGENT_PASSWORD_KEY").map_err(|_| unavailable())?,
        );
        Self::parse(&text)
    }
    fn parse(text: &str) -> Result<Self> {
        if text.len() != 64 || !text.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(unavailable());
        }
        let mut bytes = Zeroizing::new([0u8; 32]);
        for (i, value) in bytes.iter_mut().enumerate() {
            *value = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).map_err(|_| unavailable())?;
        }
        Ok(Self(aead::LessSafeKey::new(
            aead::UnboundKey::new(&aead::AES_256_GCM, &*bytes).map_err(|_| unavailable())?,
        )))
    }
    fn seal(&self, secret: &str, aad: &str) -> Result<String> {
        let mut nonce = [0u8; 12];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| unavailable())?;
        let mut body = Zeroizing::new(secret.as_bytes().to_vec());
        self.0
            .seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(aad.as_bytes()),
                &mut *body,
            )
            .map_err(|_| unavailable())?;
        let mut sealed = nonce.to_vec();
        sealed.extend_from_slice(&body);
        Ok(STANDARD.encode(sealed))
    }
    fn open(&self, sealed: &str, aad: &str) -> Result<Zeroizing<String>> {
        let mut bytes = Zeroizing::new(STANDARD.decode(sealed).map_err(|_| unavailable())?);
        if bytes.len() < 28 {
            return Err(unavailable());
        }
        let (nonce, body) = bytes.split_at_mut(12);
        let nonce: [u8; 12] = nonce.try_into().map_err(|_| unavailable())?;
        let plaintext = self
            .0
            .open_in_place(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(aad.as_bytes()),
                body,
            )
            .map_err(|_| unavailable())?;
        Ok(Zeroizing::new(
            std::str::from_utf8(plaintext)
                .map_err(|_| unavailable())?
                .to_owned(),
        ))
    }
}
fn aad(uuid: &str, id: &str, sequence: &str) -> String {
    format!("agent-password-v1:{uuid}:{id}:{sequence}")
}

pub async fn create(
    db: &DatabaseConnection,
    key: &Key,
    id: i32,
    actor: i32,
    password: &str,
) -> Result<Value> {
    if !(8..=128).contains(&password.len()) || !password.bytes().all(|c| (0x20..=0x7e).contains(&c))
    {
        return Err(Error(StatusCode::BAD_REQUEST, "invalid_rustdesk_password"));
    }
    let tx = db.begin().await?;
    let agent = store::lock_agent(&tx, id).await?;
    let uuid = agent.agent_uuid.as_deref().ok_or_else(conflict)?;
    if agent.revoked
        || peer::Entity::find_by_id(agent.peer_row_id)
            .one(&tx)
            .await?
            .is_none_or(|p| p.status != 1)
    {
        return Err(conflict());
    }
    let previous = operation::Entity::find_by_id(id).one(&tx).await?;
    let now = chrono::Utc::now().timestamp();
    if previous
        .as_ref()
        .is_some_and(|p| p.status == "claimed" || (p.status == "queued" && p.expires_at > now))
    {
        return Err(conflict());
    }
    let sequence = match &previous {
        Some(p) => p
            .sequence
            .parse::<u64>()
            .ok()
            .and_then(|n| n.checked_add(1))
            .ok_or_else(conflict)?,
        None => 1,
    }
    .to_string();
    let operation_id = uuid::Uuid::new_v4().to_string();
    let sealed = key.seal(password, &aad(uuid, &operation_id, &sequence))?;
    let value = operation::ActiveModel {
        agent_id: Set(id),
        operation_id: Set(operation_id.clone()),
        sequence: Set(sequence.clone()),
        agent_uuid: Set(uuid.into()),
        ciphertext: Set(Some(sealed)),
        expires_at: Set(now + 900),
        status: Set("queued".into()),
        actor_id: Set(actor),
    };
    if previous.is_some() {
        value.update(&tx).await?;
    } else {
        value.insert(&tx).await?;
    }
    tx.commit().await?;
    Ok(json!({"operation_id":operation_id,"sequence":sequence,"status":"queued"}))
}

pub async fn claim(
    db: &DatabaseConnection,
    key: Option<&Key>,
    token: &str,
) -> Result<Option<Value>> {
    let (tx, agent) = store::authenticate(db, token).await?;
    if peer::Entity::find_by_id(agent.peer_row_id)
        .one(&tx)
        .await?
        .is_none_or(|p| p.status != 1)
    {
        return Err(Error(StatusCode::UNAUTHORIZED, "invalid_agent_credential"));
    }
    let Some(row) = operation::Entity::find_by_id(agent.id).one(&tx).await? else {
        tx.commit().await?;
        return Ok(None);
    };
    if !matches!(row.status.as_str(), "queued" | "claimed") {
        tx.commit().await?;
        return Ok(None);
    }
    if agent.agent_uuid.as_deref() != Some(&row.agent_uuid) {
        return Err(conflict());
    }
    let sequence = row.sequence.parse::<u64>().map_err(|_| conflict())?;
    let expired = row.expires_at <= chrono::Utc::now().timestamp();
    let secret = if expired {
        None
    } else {
        row.ciphertext
            .as_ref()
            .map(|v| {
                key.ok_or_else(unavailable)?
                    .open(v, &aad(&row.agent_uuid, &row.operation_id, &row.sequence))
            })
            .transpose()?
    };
    // Change only status: concurrent expiry cleanup must not restore ciphertext.
    operation::Entity::update_many()
        .col_expr(operation::Column::Status, Expr::value("claimed"))
        .filter(operation::Column::AgentId.eq(agent.id))
        .exec(&tx)
        .await?;
    tx.commit().await?;
    Ok(Some(
        json!({"schema_version":1,"operation_id":row.operation_id,"sequence":sequence,
        "expires_at":row.expires_at,"password":secret.as_ref().map(|v|v.as_str())}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub schema_version: u32,
    pub operation_id: String,
    pub sequence: u64,
    pub outcome: String,
}
pub async fn finish(db: &DatabaseConnection, token: &str, result: Receipt) -> Result<()> {
    if result.schema_version != 1
        || result.sequence == 0
        || !matches!(
            result.outcome.as_str(),
            "command_accepted" | "indeterminate" | "expired" | "unsupported"
        )
    {
        return Err(Error(StatusCode::BAD_REQUEST, "invalid_password_result"));
    }
    let (tx, agent) = store::authenticate(db, token).await?;
    let row = operation::Entity::find_by_id(agent.id)
        .one(&tx)
        .await?
        .ok_or_else(conflict)?;
    // A terminal result may have been committed before its HTTP acknowledgement
    // was lost. A newer operation must not strand that agent's durable outbox.
    // Acknowledge obsolete results without modifying the current operation.
    if result.sequence < row.sequence.parse::<u64>().map_err(|_| conflict())? {
        tx.commit().await?;
        return Ok(());
    }
    if row.operation_id != result.operation_id
        || row.sequence != result.sequence.to_string()
        || (row.status != "claimed" && row.status != result.outcome)
    {
        return Err(conflict());
    }
    let mut row: operation::ActiveModel = row.into();
    row.status = Set(result.outcome);
    row.ciphertext = Set(None);
    row.update(&tx).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn status(db: &DatabaseConnection, id: i32) -> Result<Value> {
    Ok(operation::Entity::find_by_id(id).one(db).await?.map(|r|json!({"operation_id":r.operation_id,"sequence":r.sequence,"status":r.status,"expires_at":r.expires_at,"actor_id":r.actor_id})).unwrap_or(Value::Null))
}
pub async fn purge(db: &DatabaseConnection) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    operation::Entity::update_many()
        .col_expr(
            operation::Column::Ciphertext,
            Expr::value(Option::<String>::None),
        )
        .filter(operation::Column::ExpiresAt.lte(now))
        .exec(db)
        .await?;
    operation::Entity::update_many()
        .col_expr(operation::Column::Status, Expr::value("expired"))
        .filter(operation::Column::Status.eq("queued"))
        .filter(operation::Column::ExpiresAt.lte(now))
        .exec(db)
        .await?;
    Ok(())
}
pub fn start_cleanup(db: DatabaseConnection) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            if purge(&db).await.is_err() {
                tracing::warn!("password operation cleanup unavailable");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_encryption_binds_device_and_operation_and_rejects_tampering() {
        let key = Key::parse(&"01".repeat(32)).unwrap();
        let sealed = key
            .seal("test-only-password", "device:operation:1")
            .unwrap();
        assert!(!sealed.contains("test-only-password"));
        assert_eq!(
            &**key.open(&sealed, "device:operation:1").unwrap(),
            "test-only-password"
        );
        assert!(key.open(&sealed, "different-device:operation:1").is_err());
        assert!(Key::parse(&"02".repeat(32))
            .unwrap()
            .open(&sealed, "device:operation:1")
            .is_err());
        let mut bytes = STANDARD.decode(&sealed).unwrap();
        bytes[15] ^= 1;
        assert!(key
            .open(&STANDARD.encode(bytes), "device:operation:1")
            .is_err());
        assert!(Key::parse("bad-key").is_err());
    }

    #[tokio::test]
    async fn password_expiry_erases_secret_without_allowing_overtaking_or_replay() {
        let (_, db) = super::super::tests::setup("sqlite::memory:").await;
        let key = Key::parse(&"01".repeat(32)).unwrap();
        let (id, token) = store::create(&db, 1).await.unwrap();
        store::bind(&db, id, &uuid::Uuid::new_v4().to_string())
            .await
            .unwrap();
        assert!(claim(&db, None, &token).await.unwrap().is_none());
        let created = create(&db, &key, id, 1, "test-only-password")
            .await
            .unwrap();
        assert!(claim(&db, None, &token).await.is_err());
        let saved = operation::Entity::find_by_id(id)
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        assert!(saved.ciphertext.is_some());
        assert!(!saved
            .ciphertext
            .as_ref()
            .unwrap()
            .contains("test-only-password"));
        assert!(!status(&db, id)
            .await
            .unwrap()
            .to_string()
            .contains("test-only-password"));
        let delivery = claim(&db, Some(&key), &token).await.unwrap().unwrap();
        assert_eq!(delivery["password"], "test-only-password");
        assert_eq!(delivery["operation_id"], created["operation_id"]);
        assert!(create(&db, &key, id, 1, "new-test-password").await.is_err());
        operation::Entity::update_many()
            .col_expr(operation::Column::ExpiresAt, Expr::value(1i64))
            .exec(&db)
            .await
            .unwrap();
        purge(&db).await.unwrap();
        let expired = operation::Entity::find_by_id(id)
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(expired.ciphertext, None);
        assert_eq!(expired.status, "claimed");
        assert!(create(&db, &key, id, 1, "new-test-password").await.is_err());
        assert!(claim(&db, None, &token).await.unwrap().unwrap()["password"].is_null());
        let receipt = || Receipt {
            schema_version: 1,
            operation_id: created["operation_id"].as_str().unwrap().into(),
            sequence: 1,
            outcome: "indeterminate".into(),
        };
        finish(&db, &token, receipt()).await.unwrap();
        finish(&db, &token, receipt()).await.unwrap();
        assert_eq!(
            create(&db, &key, id, 1, "new-test-password").await.unwrap()["sequence"],
            "2"
        );
        finish(&db, &token, receipt()).await.unwrap();
        let current = status(&db, id).await.unwrap();
        assert_eq!(current["sequence"], "2");
        assert_eq!(current["status"], "queued");
    }

    #[tokio::test]
    async fn password_operations_require_bound_active_identity_and_revoke_erases_secret() {
        let (_, db) = super::super::tests::setup("sqlite::memory:").await;
        let key = Key::parse(&"01".repeat(32)).unwrap();
        let (id, token) = store::create(&db, 1).await.unwrap();
        assert!(create(&db, &key, id, 1, "test-only-password")
            .await
            .is_err());
        store::bind(&db, id, &uuid::Uuid::new_v4().to_string())
            .await
            .unwrap();
        assert!(create(&db, &key, id, 1, "short").await.is_err());
        create(&db, &key, id, 1, "test-only-password")
            .await
            .unwrap();
        assert!(claim(&db, Some(&key), "invalid-token").await.is_err());
        let (other, other_token) = store::create(&db, 2).await.unwrap();
        store::bind(&db, other, &uuid::Uuid::new_v4().to_string())
            .await
            .unwrap();
        assert!(claim(&db, Some(&key), &other_token)
            .await
            .unwrap()
            .is_none());
        store::revoke(&db, id).await.unwrap();
        assert!(claim(&db, Some(&key), &token).await.is_err());
        assert!(operation::Entity::find_by_id(id)
            .one(&db)
            .await
            .unwrap()
            .unwrap()
            .ciphertext
            .is_none());
        assert!(create(&db, &key, id, 1, "new-test-password").await.is_err());
    }
}
