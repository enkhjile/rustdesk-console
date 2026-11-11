use super::*;
use entity::{agent_password, companion_agent as agent};

async fn legacy_db() -> DatabaseConnection {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    // The previously shipped schema: no uniqueness constraints.
    db.execute_unprepared(
        "CREATE TABLE companion_agents (
        id INTEGER PRIMARY KEY AUTOINCREMENT, peer_row_id INTEGER NOT NULL,
        agent_uuid TEXT, token_hash TEXT NOT NULL, revoked BOOLEAN NOT NULL,
        revision TEXT NOT NULL, desired_json TEXT NOT NULL, sequence TEXT NOT NULL,
        report_json TEXT, last_seen BIGINT NOT NULL)",
    )
    .await
    .unwrap();
    create_tables(&db).await.unwrap();
    record_schema_version(&db, 274).await.unwrap();
    db
}

fn enrollment(id: i32) -> agent::ActiveModel {
    agent::ActiveModel {
        id: Set(id),
        peer_row_id: Set(id),
        agent_uuid: Set(None),
        token_hash: Set(format!("dummy-hash-{id}")),
        revoked: Set(false),
        revision: Set("7".into()),
        desired_json: Set("null".into()),
        sequence: Set("12".into()),
        report_json: Set(Some("{}".into())),
        last_seen: Set(123),
    }
}

async fn migrate(db: &DatabaseConnection) -> anyhow::Result<()> {
    migrate_and_seed(db, &Config::default(), &I18n::load("en"), "en").await
}

#[tokio::test]
async fn agent_upgrade_preserves_state_and_resumes_partial_indexes() {
    for partial in [false, true] {
        assert_upgrade_preserves_state(partial).await;
    }
}

async fn assert_upgrade_preserves_state(partial: bool) {
    let db = legacy_db().await;
    let mut first = enrollment(1);
    first.agent_uuid = Set(Some("dummy-uuid".into()));
    first.insert(&db).await.unwrap();
    enrollment(2).insert(&db).await.unwrap();
    enrollment(3).insert(&db).await.unwrap();
    let password = agent_password::ActiveModel {
        agent_id: Set(1),
        operation_id: Set("dummy-operation".into()),
        sequence: Set("4".into()),
        agent_uuid: Set("dummy-uuid".into()),
        ciphertext: Set(Some("dummy-ciphertext".into())),
        expires_at: Set(456),
        status: Set("queued".into()),
        actor_id: Set(1),
    }
    .insert(&db)
    .await
    .unwrap();
    let before = agent::Entity::find()
        .order_by_asc(agent::Column::Id)
        .all(&db)
        .await
        .unwrap();
    // MySQL DDL can commit before the migration version is recorded.
    if partial {
        db.execute_unprepared(
        "CREATE UNIQUE INDEX companion_agents_peer_row_id_unique ON companion_agents(peer_row_id)",
    )
    .await
    .unwrap();
    }
    migrate(&db).await.unwrap();
    migrate(&db).await.unwrap();
    assert_eq!(
        agent::Entity::find()
            .order_by_asc(agent::Column::Id)
            .all(&db)
            .await
            .unwrap(),
        before
    );
    assert_eq!(
        agent_password::Entity::find_by_id(1)
            .one(&db)
            .await
            .unwrap(),
        Some(password)
    );
    assert_eq!(latest_schema_version(&db).await.unwrap(), Some(275));
    assert_eq!(
        version::Entity::find()
            .all(&db)
            .await
            .unwrap()
            .iter()
            .filter(|v| v.version == 275)
            .count(),
        1
    );
    for column in ["peer_row_id", "agent_uuid", "token_hash"] {
        let mut duplicate = enrollment(4);
        match column {
            "peer_row_id" => duplicate.peer_row_id = Set(1),
            "agent_uuid" => duplicate.agent_uuid = Set(Some("dummy-uuid".into())),
            "token_hash" => duplicate.token_hash = Set("dummy-hash-1".into()),
            _ => unreachable!(),
        }
        assert!(
            duplicate.insert(&db).await.is_err(),
            "duplicate {column} accepted after upgrade"
        );
    }
}

#[tokio::test]
async fn agent_upgrade_refuses_duplicates_without_deleting_or_versioning_them() {
    for column in ["peer_row_id", "agent_uuid", "token_hash"] {
        let db = legacy_db().await;
        let mut first = enrollment(1);
        first.agent_uuid = Set(Some("dummy-uuid".into()));
        first.insert(&db).await.unwrap();
        let mut duplicate = enrollment(2);
        match column {
            "peer_row_id" => duplicate.peer_row_id = Set(1),
            "agent_uuid" => duplicate.agent_uuid = Set(Some("dummy-uuid".into())),
            "token_hash" => duplicate.token_hash = Set("dummy-hash-1".into()),
            _ => unreachable!(),
        }
        duplicate.insert(&db).await.unwrap();
        let before = agent::Entity::find()
            .order_by_asc(agent::Column::Id)
            .all(&db)
            .await
            .unwrap();
        let error = migrate(&db).await.unwrap_err().to_string();
        assert!(error.contains(column), "{error}");
        assert!(
            !error.contains("dummy-"),
            "error exposed identity data: {error}"
        );
        assert_eq!(latest_schema_version(&db).await.unwrap(), Some(274));
        assert_eq!(
            agent::Entity::find()
                .order_by_asc(agent::Column::Id)
                .all(&db)
                .await
                .unwrap(),
            before
        );
    }
}
