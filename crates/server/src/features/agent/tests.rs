use super::*;
use ::entity::{companion_agent, peer, strategy, strategy_assignment, user, user_token};
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use sea_orm::*;
use tower::ServiceExt;

async fn setup(url: &str) -> (Router, DatabaseConnection) {
    let db = Database::connect(url).await.unwrap();
    let schema = Schema::new(DbBackend::Sqlite);
    for mut statement in [
        schema.create_table_from_entity(companion_agent::Entity),
        schema.create_table_from_entity(peer::Entity),
        schema.create_table_from_entity(strategy::Entity),
        schema.create_table_from_entity(strategy_assignment::Entity),
        schema.create_table_from_entity(user::Entity),
        schema.create_table_from_entity(user_token::Entity),
    ] {
        db.execute(
            db.get_database_backend()
                .build(&statement.if_not_exists().to_owned()),
        )
        .await
        .unwrap();
    }
    for id in [1, 2] {
        user::ActiveModel {
            id: Set(id),
            username: Set(format!("user-{id}")),
            is_admin: Set(Some(id == 1)),
            status: Set(1),
            ..Default::default()
        }
        .insert(&db)
        .await
        .unwrap();
        user_token::ActiveModel {
            user_id: Set(id),
            token: Set(format!("session-{id}")),
            expired_at: Set(chrono::Utc::now().timestamp() + 3600),
            ..Default::default()
        }
        .insert(&db)
        .await
        .unwrap();
        peer::ActiveModel {
            row_id: Set(id),
            id: Set(format!("device-{id}")),
            status: Set(1),
            ..Default::default()
        }
        .insert(&db)
        .await
        .unwrap();
    }
    let app = routes().with_state(crate::http::api::deployment_tests::test_state(db.clone()));
    (app, db)
}
async fn request(
    app: &Router,
    method: &str,
    path: &str,
    token: &str,
    admin: &str,
    body: &str,
) -> (StatusCode, serde_json::Value) {
    let result = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header("api-token", admin)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = result.status();
    let bytes = to_bytes(result.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        if bytes.is_empty() {
            json!(null)
        } else {
            serde_json::from_slice(&bytes).unwrap()
        },
    )
}
async fn enroll(app: &Router, peer: i32) -> (i32, String, String) {
    let (status, body) = request(
        app,
        "POST",
        "/api/admin/agent/create",
        "",
        "session-1",
        &json!({"peer_row_id":peer}).to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let id = body["data"]["id"].as_i64().unwrap() as i32;
    let credential = body["data"]["credential"].as_str().unwrap().to_owned();
    let uuid = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        request(
            app,
            "POST",
            &format!("/api/admin/agent/{id}/bind"),
            "",
            "session-1",
            &json!({"agent_uuid":uuid}).to_string()
        )
        .await
        .0,
        StatusCode::OK
    );
    (id, credential, uuid)
}
async fn assign(db: &DatabaseConnection, peer: &str, value: &str) -> strategy::Model {
    let s = crate::services::strategy::create(
        db,
        &uuid::Uuid::new_v4().to_string(),
        "",
        1,
        std::collections::HashMap::from([
            ("approve-mode".into(), value.into()),
            ("proxy-password".into(), "never-forward-this".into()),
        ]),
        Default::default(),
    )
    .await
    .unwrap();
    crate::services::strategy::assign(db, s.id, "peer", peer, 100)
        .await
        .unwrap();
    s
}
fn report(uuid: &str, revision: u64, sequence: u64) -> serde_json::Value {
    json!({"schema_version":1,"agent_uuid":uuid,"rustdesk_id":null,"rustdesk_version":"1.4.7","desired_revision":revision,"observation_sequence":sequence,"applied_revision":null,"results":{"approve_mode":"unsupported"}})
}

#[tokio::test]
async fn rotation_recovers_credentials_without_resetting_identity_or_history() {
    let (app, db) = setup("sqlite::memory:").await;
    let (id, old, uuid) = enroll(&app, 1).await;
    assign(&db, "device-1", "click").await;
    let body = report(&uuid, 1, 7).to_string();
    assert_eq!(
        request(&app, "POST", "/v1/agent/status", &old, "", &body)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    let path = format!("/api/admin/agent/{id}/rotate");
    for admin in ["", "session-2"] {
        assert_ne!(
            request(&app, "POST", &path, &old, admin, "").await.1["code"],
            0
        );
    }
    // A lost response is recovered by issuing again; revocation can also be recovered.
    let mut previous = old.clone();
    for revoked in [false, true] {
        let before = store::status(&db, 1).await.unwrap();
        if revoked {
            store::revoke(&db, id).await.unwrap();
        }
        let (code, response) = request(&app, "POST", &path, "", "session-1", "").await;
        assert_eq!(code, StatusCode::OK);
        let replacement = response["data"]["credential"].as_str().unwrap().to_owned();
        assert_ne!(replacement, previous);
        assert_eq!(store::status(&db, 1).await.unwrap(), before);
        for rejected in [&old, &previous] {
            assert_eq!(
                request(&app, "GET", "/v1/agent/desired", rejected, "", "")
                    .await
                    .0,
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(
                request(&app, "POST", "/v1/agent/status", rejected, "", &body)
                    .await
                    .0,
                StatusCode::UNAUTHORIZED
            );
        }
        let row = companion_agent::Entity::find_by_id(id)
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(row.token_hash, replacement);
        assert_eq!(row.token_hash.len(), 64);
        assert_eq!(
            request(&app, "POST", "/v1/agent/status", &replacement, "", &body)
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            request(
                &app,
                "POST",
                "/v1/agent/status",
                &replacement,
                "",
                &report(&uuid, 1, 6).to_string()
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
        previous = replacement;
    }
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/admin/agent/999/rotate",
            "",
            "session-1",
            ""
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn rotation_does_not_activate_unbound_or_disabled_devices() {
    let (app, db) = setup("sqlite::memory:").await;
    let (id, original) = store::create(&db, 1).await.unwrap();
    let replacement = store::rotate(&db, id).await.unwrap();
    for token in [&original, &replacement] {
        assert_eq!(
            request(&app, "GET", "/v1/agent/desired", token, "", "")
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
    }
    store::bind(&db, id, &uuid::Uuid::new_v4().to_string())
        .await
        .unwrap();
    assert_eq!(
        request(&app, "GET", "/v1/agent/desired", &replacement, "", "")
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    let mut device: peer::ActiveModel = peer::Entity::find_by_id(1)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into();
    device.status = Set(0);
    device.update(&db).await.unwrap();
    let replacement = store::rotate(&db, id).await.unwrap();
    assert_eq!(
        request(&app, "GET", "/v1/agent/desired", &replacement, "", "")
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn enrollment_is_admin_only_credentials_are_device_bound_and_revocable() {
    let (app, db) = setup("sqlite::memory:").await;
    for admin in ["", "session-2"] {
        assert_ne!(
            request(
                &app,
                "POST",
                "/api/admin/agent/create",
                "",
                admin,
                r#"{"peer_row_id":1}"#
            )
            .await
            .1["code"],
            0
        );
    }
    let (id, token, uuid) = enroll(&app, 1).await;
    let (_, other, other_uuid) = enroll(&app, 2).await;
    assign(&db, "device-1", "click").await;
    assert_eq!(
        request(&app, "GET", "/v1/agent/desired", &other, "", "")
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    let (code, desired) = request(&app, "GET", "/v1/agent/desired", &token, "", "").await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(
        desired,
        json!({"schema_version":1,"revision":1,"settings":{"approve_mode":"click"}})
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/v1/agent/status",
            &token,
            "",
            &report(&other_uuid, 1, 1).to_string()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/v1/agent/status",
            &token,
            "",
            &report(&uuid, 1, 1).to_string()
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let status = request(&app, "GET", "/api/admin/agent/peer/1", "", "session-1", "")
        .await
        .1;
    assert_eq!(
        status["data"]["report"]["results"]["approve_mode"],
        "unsupported"
    );
    assert!(!status.to_string().contains(&token));
    assert!(!status.to_string().contains("token_hash"));
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("/api/admin/agent/{id}/revoke"),
            "",
            "session-1",
            ""
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "GET", "/v1/agent/desired", &token, "", "")
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/v1/agent/status",
            &token,
            "",
            &report(&uuid, 1, 2).to_string()
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn revisions_reports_and_retries_survive_reopen_and_assignment_changes() {
    let path = std::env::temp_dir().join(format!("console-agent-{}.db", uuid::Uuid::new_v4()));
    let url = format!(
        "sqlite://{}?mode=rwc",
        path.to_string_lossy().replace('\\', "/")
    );
    let (app, db) = setup(&url).await;
    let (_, token, uuid) = enroll(&app, 1).await;
    let first = assign(&db, "device-1", "click").await;
    let one = request(&app, "GET", "/v1/agent/desired", &token, "", "")
        .await
        .1;
    assert_eq!(one["revision"], 1);
    let body = report(&uuid, 1, 7).to_string();
    assert_eq!(
        request(&app, "POST", "/v1/agent/status", &token, "", &body)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(&app, "POST", "/v1/agent/status", &token, "", &body)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    drop(app);
    db.close().await.unwrap();
    let db = Database::connect(&url).await.unwrap();
    let app = routes().with_state(crate::http::api::deployment_tests::test_state(db.clone()));
    assert_eq!(
        request(&app, "GET", "/v1/agent/desired", &token, "", "")
            .await
            .1,
        one
    );
    assert_eq!(
        request(&app, "POST", "/v1/agent/status", &token, "", &body)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    let mut changed = report(&uuid, 1, 7);
    changed["rustdesk_version"] = json!("other");
    assert_eq!(
        request(
            &app,
            "POST",
            "/v1/agent/status",
            &token,
            "",
            &changed.to_string()
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/v1/agent/status",
            &token,
            "",
            &report(&uuid, 1, 6).to_string()
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let second = assign(&db, "device-1", "password").await;
    // Even if timestamps collide, a different effective payload gets a new revision.
    let mut second: strategy::ActiveModel = second.into();
    second.modified_at = Set(first.modified_at);
    second.update(&db).await.unwrap();
    assert_eq!(
        request(&app, "POST", "/v1/agent/status", &token, "", &body)
            .await
            .0,
        StatusCode::CONFLICT
    );
    let two = request(&app, "GET", "/v1/agent/desired", &token, "", "")
        .await
        .1;
    assert_eq!(two["revision"], 2);
    assert_eq!(two["settings"]["approve_mode"], "password");
    assert_eq!(
        request(
            &app,
            "POST",
            "/v1/agent/status",
            &token,
            "",
            &report(&uuid, 2, 8).to_string()
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    crate::services::strategy::assign(&db, 0, "peer", "device-1", 100)
        .await
        .unwrap();
    assert_eq!(
        request(&app, "GET", "/v1/agent/desired", &token, "", "")
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assign(&db, "device-1", "password").await;
    assert_eq!(
        request(&app, "GET", "/v1/agent/desired", &token, "", "")
            .await
            .1["revision"],
        3
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/v1/agent/status",
            &token,
            "",
            &report(&uuid, 3, 7).to_string()
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    drop(app);
    db.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn rejects_malformed_reports_and_keeps_last_good_status() {
    let (app, db) = setup("sqlite::memory:").await;
    let (_, token, uuid) = enroll(&app, 1).await;
    assign(&db, "device-1", "click").await;
    request(&app, "GET", "/v1/agent/desired", &token, "", "").await;
    let good = report(&uuid, 1, 1);
    assert_eq!(
        request(
            &app,
            "POST",
            "/v1/agent/status",
            &token,
            "",
            &good.to_string()
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    for (field, value) in [
        ("schema_version", json!(2)),
        ("observation_sequence", json!(0)),
        ("applied_revision", json!(1)),
        ("unexpected", json!(true)),
        ("results", json!({})),
        ("results", json!({"approve_mode":"verified"})),
        ("results", json!({"approve_mode":null})),
        (
            "results",
            json!({"approve_mode":"unsupported","verification_method":"unsupported"}),
        ),
    ] {
        let mut bad = good.clone();
        bad[field] = value;
        assert_eq!(
            request(
                &app,
                "POST",
                "/v1/agent/status",
                &token,
                "",
                &bad.to_string()
            )
            .await
            .0,
            StatusCode::BAD_REQUEST,
            "{field}"
        );
    }
    let duplicate = good.to_string().replacen('{', "{\"schema_version\":1,", 1);
    assert_eq!(
        request(&app, "POST", "/v1/agent/status", &token, "", &duplicate)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let status = request(&app, "GET", "/api/admin/agent/peer/1", "", "session-1", "")
        .await
        .1;
    assert_eq!(status["data"]["report"], good);
    // A client-provided RustDesk ID is report metadata, never an identity remapping.
    let mut changed = report(&uuid, 1, u64::MAX);
    changed["rustdesk_id"] = json!("device-2");
    assert_eq!(
        request(
            &app,
            "POST",
            "/v1/agent/status",
            &token,
            "",
            &changed.to_string()
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(&app, "GET", "/api/admin/agent/peer/1", "", "session-1", "")
            .await
            .1["data"]["observation_sequence"],
        u64::MAX.to_string()
    );
    assert_eq!(
        request(&app, "GET", "/api/admin/agent/peer/2", "", "session-1", "")
            .await
            .1["data"],
        json!(null)
    );
}

#[tokio::test]
async fn concurrent_polls_and_reports_do_not_reuse_state_for_different_content() {
    let path = std::env::temp_dir().join(format!(
        "console-agent-concurrent-{}.db",
        uuid::Uuid::new_v4()
    ));
    let url = format!(
        "sqlite://{}?mode=rwc",
        path.to_string_lossy().replace('\\', "/")
    );
    let (app, db) = setup(&url).await;
    let other_db = Database::connect(&url).await.unwrap();
    let other_app = routes().with_state(crate::http::api::deployment_tests::test_state(
        other_db.clone(),
    ));
    let (_, token, uuid) = enroll(&app, 1).await;
    assign(&db, "device-1", "click").await;
    let mut tasks = vec![];
    for _ in 0..8 {
        let db = db.clone();
        let token = token.clone();
        tasks.push(tokio::spawn(async move {
            store::poll(&db, &token).await.unwrap().unwrap().revision
        }));
    }
    for task in tasks {
        assert_eq!(task.await.unwrap(), 1);
    }
    let first = report(&uuid, 1, 1);
    let mut different = first.clone();
    different["rustdesk_version"] = json!("different");
    let first = first.to_string();
    let different = different.to_string();
    let (a, b) = tokio::join!(
        request(&app, "POST", "/v1/agent/status", &token, "", &first),
        request(
            &other_app,
            "POST",
            "/v1/agent/status",
            &token,
            "",
            &different
        )
    );
    assert!(
        (a.0 == StatusCode::NO_CONTENT && b.0 == StatusCode::CONFLICT)
            || (b.0 == StatusCode::NO_CONTENT && a.0 == StatusCode::CONFLICT)
    );
    // A concurrent report must not restore the old credential hash.
    let id = store::status(&db, 1).await.unwrap()["id"].as_i64().unwrap() as i32;
    let (rotated, submitted) = tokio::join!(
        store::rotate(&db, id),
        store::report(
            &other_db,
            &token,
            serde_json::from_value(report(&uuid, 1, 2)).unwrap()
        )
    );
    let replacement = rotated.unwrap();
    if let Err(error) = submitted {
        assert_eq!(error.0, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(
        request(&other_app, "GET", "/v1/agent/desired", &token, "", "")
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&other_app, "GET", "/v1/agent/desired", &replacement, "", "")
            .await
            .0,
        StatusCode::OK
    );
    drop(app);
    drop(other_app);
    db.close().await.unwrap();
    other_db.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn pending_binding_is_inactive_and_identity_cannot_be_reassigned() {
    let (app, db) = setup("sqlite::memory:").await;
    let created = request(
        &app,
        "POST",
        "/api/admin/agent/create",
        "",
        "session-1",
        r#"{"peer_row_id":1}"#,
    )
    .await
    .1;
    let token = created["data"]["credential"].as_str().unwrap();
    let id = created["data"]["id"].as_i64().unwrap();
    assert_eq!(
        request(&app, "GET", "/v1/agent/desired", token, "", "")
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let stored = companion_agent::Entity::find_by_id(id as i32)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(stored.token_hash, token);
    assert_eq!(stored.token_hash.len(), 64);
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/admin/agent/create",
            "",
            "session-1",
            r#"{"peer_row_id":1}"#
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (_, _, other_uuid) = enroll(&app, 2).await;
    let binding = format!("/api/admin/agent/{id}/bind");
    assert_eq!(
        request(
            &app,
            "POST",
            &binding,
            "",
            "session-1",
            &json!({"agent_uuid":other_uuid}).to_string()
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &binding,
            "",
            "session-1",
            r#"{"agent_uuid":"invalid"}"#
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let own = json!({"agent_uuid":uuid::Uuid::new_v4().to_string()}).to_string();
    for _ in 0..2 {
        assert_eq!(
            request(&app, "POST", &binding, "", "session-1", &own)
                .await
                .0,
            StatusCode::OK
        );
    }
    assert_eq!(
        request(
            &app,
            "POST",
            &binding,
            "",
            "session-1",
            &json!({"agent_uuid":uuid::Uuid::new_v4().to_string()}).to_string()
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    for (method, path, body) in [
        ("GET", "/api/admin/agent/peer/1".to_owned(), ""),
        ("POST", binding, own.as_str()),
        ("POST", format!("/api/admin/agent/{id}/revoke"), ""),
    ] {
        assert_ne!(
            request(&app, method, &path, "", "session-2", body).await.1["code"],
            0
        );
    }
    let mut admin: user::ActiveModel = user::Entity::find_by_id(1)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into();
    admin.is_admin = Set(Some(false));
    admin.update(&db).await.unwrap();
    assert_ne!(
        request(&app, "GET", "/api/admin/agent/peer/1", "", "session-1", "")
            .await
            .1["code"],
        0
    );
}

#[tokio::test]
async fn disabled_devices_invalid_strategies_and_exhausted_revisions_fail_closed() {
    let (app, db) = setup("sqlite::memory:").await;
    let (id, token, _) = enroll(&app, 1).await;
    assign(&db, "device-1", "invalid").await;
    assert_eq!(
        request(&app, "GET", "/v1/agent/desired", &token, "", "")
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assign(&db, "device-1", "click").await;
    assert_eq!(
        request(&app, "GET", "/v1/agent/desired", &token, "", "")
            .await
            .1["revision"],
        1
    );
    let mut row: companion_agent::ActiveModel = companion_agent::Entity::find_by_id(id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into();
    row.revision = Set(u64::MAX.to_string());
    row.update(&db).await.unwrap();
    assign(&db, "device-1", "password").await;
    assert_eq!(
        request(&app, "GET", "/v1/agent/desired", &token, "", "")
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        companion_agent::Entity::find_by_id(id)
            .one(&db)
            .await
            .unwrap()
            .unwrap()
            .revision,
        u64::MAX.to_string()
    );
    let mut peer: peer::ActiveModel = peer::Entity::find_by_id(1)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into();
    peer.status = Set(0);
    peer.update(&db).await.unwrap();
    assert_eq!(
        request(&app, "GET", "/v1/agent/desired", &token, "", "")
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/agent/status")
                .body(Body::from(vec![b' '; 65537]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}
