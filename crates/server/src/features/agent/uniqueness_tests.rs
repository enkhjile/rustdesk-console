use super::{store, tests::setup};
use ::entity::companion_agent as agent;
use axum::http::StatusCode;
use sea_orm::*;

#[tokio::test]
async fn concurrent_binding_cannot_share_an_agent_uuid() {
    let (_, db) = setup("sqlite::memory:").await;
    let (first, _) = store::create(&db, 1).await.unwrap();
    let (second, _) = store::create(&db, 2).await.unwrap();
    let uuid = uuid::Uuid::new_v4().to_string();
    let (a, b) = tokio::join!(
        store::bind(&db, first, &uuid),
        store::bind(&db, second, &uuid)
    );
    let error = match (a, b) {
        (Ok(()), Err(error)) | (Err(error), Ok(())) => error,
        _ => panic!("exactly one binding must succeed"),
    };
    assert_eq!(error.0, StatusCode::CONFLICT);
    assert_eq!(
        agent::Entity::find()
            .filter(agent::Column::AgentUuid.eq(uuid))
            .count(&db)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn concurrent_enrollment_has_one_identity_and_revocation_blocks_its_credential() {
    let (_, db) = setup("sqlite::memory:").await;
    let (first, second) = tokio::join!(store::create(&db, 1), store::create(&db, 1));
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let (enrollment, rejected) = match (first, second) {
        (Ok(value), Err(error)) | (Err(error), Ok(value)) => (value, error),
        _ => panic!("expected exactly one enrollment"),
    };
    assert_eq!(rejected.0, StatusCode::CONFLICT);
    assert_eq!(agent::Entity::find().count(&db).await.unwrap(), 1);
    let (id, token) = enrollment;
    // NULL UUIDs are allowed for multiple devices awaiting binding.
    store::create(&db, 2).await.unwrap();
    store::bind(&db, id, &uuid::Uuid::new_v4().to_string())
        .await
        .unwrap();
    assert_eq!(store::status(&db, 1).await.unwrap()["id"], id);
    assert!(store::poll(&db, &token).await.is_ok());
    store::revoke(&db, id).await.unwrap();
    assert_eq!(
        store::poll(&db, &token).await.unwrap_err().0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn fresh_schema_rejects_duplicate_peer_uuid_and_token_hash() {
    for column in [
        agent::Column::PeerRowId,
        agent::Column::AgentUuid,
        agent::Column::TokenHash,
    ] {
        let (_, db) = setup("sqlite::memory:").await;
        let (id, _) = store::create(&db, 1).await.unwrap();
        let uuid = uuid::Uuid::new_v4().to_string();
        store::bind(&db, id, &uuid).await.unwrap();
        let row = agent::Entity::find_by_id(id)
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        let mut duplicate: agent::ActiveModel = row.clone().into();
        duplicate = duplicate.reset_all();
        duplicate.id = NotSet;
        duplicate.peer_row_id = Set(2);
        duplicate.agent_uuid = Set(Some(uuid::Uuid::new_v4().to_string()));
        duplicate.token_hash = Set("different-test-hash".into());
        match column {
            agent::Column::PeerRowId => duplicate.peer_row_id = Set(row.peer_row_id),
            agent::Column::AgentUuid => duplicate.agent_uuid = Set(row.agent_uuid),
            agent::Column::TokenHash => duplicate.token_hash = Set(row.token_hash),
            _ => unreachable!(),
        }
        assert!(
            duplicate.insert(&db).await.is_err(),
            "duplicate {column:?} accepted"
        );
    }
}
