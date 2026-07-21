use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use serde_json::json;
use sqlx::MySqlPool;
use tower::ServiceExt;

use crate::api::AppState;
use crate::utils::jwt::{create_admin_token, Claims};

fn make_admin_token(role: i8) -> String {
    create_admin_token(1, role).expect("Failed to create test token")
}

fn make_auth_header(role: i8) -> String {
    format!("Bearer {}", make_admin_token(role))
}

async fn cleanup_test_data(pool: &MySqlPool) {
    sqlx::query("DELETE FROM game_alias_entries WHERE game_id IN (SELECT id FROM games WHERE name LIKE 'api_test_%')")
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM games WHERE name LIKE 'api_test_%'")
        .execute(pool)
        .await
        .ok();
}

fn app_state() -> AppState {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "mysql://root:root@localhost:3306/hive_claw".to_string());
    let pool = sqlx::MySqlPool::connect_lazy(&database_url).expect("Failed to create pool");
    AppState {
        pool,
        redis: redis::Client::open("redis://localhost:6379")
            .expect("Failed to create redis client")
            .into(),
        s3: aws_sdk_s3::Client::new(&aws_config::from_env().load_sync()),
        runtime_state: Default::default(),
        ext_pool: None,
    }
}

async fn get_list(app: Router, auth: &str, params: &str) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/game-aliases{}", params))
                .header("Authorization", auth)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    (status, json)
}

async fn get_detail(app: Router, auth: &str, id: i64) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/game-aliases/{}", id))
                .header("Authorization", auth)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::from_slice(&body).unwrap();
    (status, json)
}

async fn post_create(app: Router, auth: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/game-aliases")
                .header("Authorization", auth)
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::from_slice(&body).unwrap();
    (status, json)
}

async fn put_update(app: Router, auth: &str, id: i64, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/game-aliases/{}", id))
                .header("Authorization", auth)
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::from_slice(&body).unwrap();
    (status, json)
}

async fn delete_game(app: Router, auth: &str, id: i64) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/game-aliases/{}", id))
                .header("Authorization", auth)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::from_slice(&body).unwrap();
    (status, json)
}

#[tokio::test]
async fn test_get_list_success() {
    let app = crate::api::create_router_from_state(app_state());
    let auth = make_auth_header(2);

    let (status, json) = get_list(app, &auth, "?page=1&page_size=10").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["code"], 0);
    assert!(json["data"].is_object());
    assert!(json["data"]["games"].is_array());
    assert!(json["data"]["total"].is_number());
}

#[tokio::test]
async fn test_get_list_search_by_name() {
    let state = app_state();
    cleanup_test_data(&state.pool).await;

    let _ = sqlx::query("INSERT INTO games (name) VALUES ('api_test_search_name')")
        .execute(&state.pool)
        .await
        .unwrap();

    let app = crate::api::create_router_from_state(state.clone());
    let auth = make_auth_header(2);

    let (status, json) = get_list(app, &auth, "?q=api_test_search_name").await;
    assert_eq!(status, StatusCode::OK);
    assert!(json["data"]["total"].as_u64().unwrap() >= 1);

    cleanup_test_data(&state.pool).await;
}

#[tokio::test]
async fn test_get_list_search_by_alias() {
    let state = app_state();
    cleanup_test_data(&state.pool).await;

    let result = sqlx::query("INSERT INTO games (name) VALUES ('api_test_alias_search_game')")
        .execute(&state.pool)
        .await
        .unwrap();
    let game_id = result.last_insert_id();
    sqlx::query("INSERT INTO game_alias_entries (game_id, alias) VALUES (?, 'unique_api_test_alias')")
        .bind(game_id)
        .execute(&state.pool)
        .await
        .unwrap();

    let app = crate::api::create_router_from_state(state.clone());
    let auth = make_auth_header(2);

    let (status, json) = get_list(app, &auth, "?q=unique_api_test_alias").await;
    assert_eq!(status, StatusCode::OK);
    assert!(json["data"]["total"].as_u64().unwrap() >= 1);

    cleanup_test_data(&state.pool).await;
}

#[tokio::test]
async fn test_get_detail_success() {
    let state = app_state();
    cleanup_test_data(&state.pool).await;

    let result = sqlx::query("INSERT INTO games (name) VALUES ('api_test_detail_game')")
        .execute(&state.pool)
        .await
        .unwrap();
    let game_id = result.last_insert_id();
    sqlx::query("INSERT INTO game_alias_entries (game_id, alias) VALUES (?, 'detail_alias_1')")
        .bind(game_id)
        .execute(&state.pool)
        .await
        .unwrap();

    let app = crate::api::create_router_from_state(state.clone());
    let auth = make_auth_header(2);

    let (status, json) = get_detail(app, &auth, game_id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["code"], 0);
    assert_eq!(json["data"]["name"], "api_test_detail_game");
    assert!(json["data"]["aliases"].is_array());

    cleanup_test_data(&state.pool).await;
}

#[tokio::test]
async fn test_get_detail_not_found() {
    let app = crate::api::create_router_from_state(app_state());
    let auth = make_auth_header(2);

    let (status, json) = get_detail(app, &auth, 999999).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["code"], 4001);
}

#[tokio::test]
async fn test_post_create_success() {
    let state = app_state();
    cleanup_test_data(&state.pool).await;

    let app = crate::api::create_router_from_state(state.clone());
    let auth = make_auth_header(2);

    let (status, json) = post_create(
        app,
        &auth,
        json!({
            "name": "api_test_create_game",
            "aliases": ["create_alias_1", "create_alias_2"]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["code"], 0);
    assert_eq!(json["data"]["name"], "api_test_create_game");
    assert_eq!(json["data"]["aliases"].as_array().unwrap().len(), 2);

    cleanup_test_data(&state.pool).await;
}

#[tokio::test]
async fn test_post_create_forbidden_for_normal() {
    let state = app_state();
    cleanup_test_data(&state.pool).await;

    let app = crate::api::create_router_from_state(state.clone());
    let auth = make_auth_header(1);

    let (status, json) = post_create(
        app,
        &auth,
        json!({
            "name": "api_test_forbidden",
            "aliases": ["forbidden_alias"]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(json["code"], 2001);

    cleanup_test_data(&state.pool).await;
}

#[tokio::test]
async fn test_post_create_empty_name() {
    let app = crate::api::create_router_from_state(app_state());
    let auth = make_auth_header(2);

    let (status, json) = post_create(
        app,
        &auth,
        json!({
            "name": "",
            "aliases": ["alias1"]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["code"], 4003);
}

#[tokio::test]
async fn test_post_create_name_too_long() {
    let app = crate::api::create_router_from_state(app_state());
    let auth = make_auth_header(2);

    let (status, json) = post_create(
        app,
        &auth,
        json!({
            "name": "a".repeat(51),
            "aliases": ["alias1"]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["code"], 4004);
}

#[tokio::test]
async fn test_post_create_empty_aliases() {
    let app = crate::api::create_router_from_state(app_state());
    let auth = make_auth_header(2);

    let (status, json) = post_create(
        app,
        &auth,
        json!({
            "name": "valid_name",
            "aliases": []
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["code"], 4005);
}

#[tokio::test]
async fn test_post_create_alias_too_long() {
    let app = crate::api::create_router_from_state(app_state());
    let auth = make_auth_header(2);

    let (status, json) = post_create(
        app,
        &auth,
        json!({
            "name": "valid_name",
            "aliases": ["a".repeat(51)]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["code"], 4006);
}

#[tokio::test]
async fn test_post_create_too_many_aliases() {
    let aliases: Vec<String> = (0..21).map(|i| format!("alias{}", i)).collect();
    let app = crate::api::create_router_from_state(app_state());
    let auth = make_auth_header(2);

    let (status, json) = post_create(app, &auth, json!({ "name": "valid_name", "aliases": aliases })).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["code"], 4007);
}

#[tokio::test]
async fn test_post_create_duplicate_name() {
    let state = app_state();
    cleanup_test_data(&state.pool).await;

    sqlx::query("INSERT INTO games (name) VALUES ('api_test_dup_name')")
        .execute(&state.pool)
        .await
        .unwrap();

    let app = crate::api::create_router_from_state(state.clone());
    let auth = make_auth_header(2);

    let (status, json) = post_create(
        app,
        &auth,
        json!({
            "name": "api_test_dup_name",
            "aliases": ["dup_name_alias"]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["code"], 4002);

    cleanup_test_data(&state.pool).await;
}

#[tokio::test]
async fn test_post_create_duplicate_alias() {
    let state = app_state();
    cleanup_test_data(&state.pool).await;

    let result = sqlx::query("INSERT INTO games (name) VALUES ('api_test_alias_owner')")
        .execute(&state.pool)
        .await
        .unwrap();
    let game_id = result.last_insert_id();
    sqlx::query("INSERT INTO game_alias_entries (game_id, alias) VALUES (?, 'protected_api_alias')")
        .bind(game_id)
        .execute(&state.pool)
        .await
        .unwrap();

    let app = crate::api::create_router_from_state(state.clone());
    let auth = make_auth_header(2);

    let (status, json) = post_create(
        app,
        &auth,
        json!({
            "name": "api_test_alias_thief",
            "aliases": ["protected_api_alias"]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["code"], 4008);

    cleanup_test_data(&state.pool).await;
}

#[tokio::test]
async fn test_put_update_success() {
    let state = app_state();
    cleanup_test_data(&state.pool).await;

    let result = sqlx::query("INSERT INTO games (name) VALUES ('api_test_update_game')")
        .execute(&state.pool)
        .await
        .unwrap();
    let game_id = result.last_insert_id();
    sqlx::query("INSERT INTO game_alias_entries (game_id, alias) VALUES (?, 'old_update_alias')")
        .bind(game_id)
        .execute(&state.pool)
        .await
        .unwrap();

    let app = crate::api::create_router_from_state(state.clone());
    let auth = make_auth_header(2);

    let (status, json) = put_update(
        app,
        &auth,
        game_id,
        json!({
            "name": "api_test_updated_game",
            "aliases": ["new_update_alias_1", "new_update_alias_2"]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"]["name"], "api_test_updated_game");
    assert_eq!(json["data"]["aliases"].as_array().unwrap().len(), 2);

    cleanup_test_data(&state.pool).await;
}

#[tokio::test]
async fn test_put_update_forbidden_for_normal() {
    let app = crate::api::create_router_from_state(app_state());
    let auth = make_auth_header(1);

    let (status, json) = put_update(app, &auth, 1, json!({ "name": "should_fail" })).await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(json["code"], 2001);
}

#[tokio::test]
async fn test_put_update_not_found() {
    let app = crate::api::create_router_from_state(app_state());
    let auth = make_auth_header(2);

    let (status, json) = put_update(app, &auth, 999999, json!({ "name": "nonexistent" })).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["code"], 4001);
}

#[tokio::test]
async fn test_delete_success() {
    let state = app_state();
    cleanup_test_data(&state.pool).await;

    let result = sqlx::query("INSERT INTO games (name) VALUES ('api_test_delete_game')")
        .execute(&state.pool)
        .await
        .unwrap();
    let game_id = result.last_insert_id();
    sqlx::query("INSERT INTO game_alias_entries (game_id, alias) VALUES (?, 'delete_api_alias')")
        .bind(game_id)
        .execute(&state.pool)
        .await
        .unwrap();

    let app = crate::api::create_router_from_state(state.clone());
    let auth = make_auth_header(2);

    let (status, json) = delete_game(app, &auth, game_id).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["code"], 0);

    cleanup_test_data(&state.pool).await;
}

#[tokio::test]
async fn test_delete_forbidden_for_normal() {
    let app = crate::api::create_router_from_state(app_state());
    let auth = make_auth_header(1);

    let (status, json) = delete_game(app, &auth, 1).await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(json["code"], 2001);
}

#[tokio::test]
async fn test_delete_not_found() {
    let app = crate::api::create_router_from_state(app_state());
    let auth = make_auth_header(2);

    let (status, json) = delete_game(app, &auth, 999999).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["code"], 4001);
}
