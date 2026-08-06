#[cfg(test)]
mod tests {
    #![allow(deprecated)]

    use crate::models::game::{CreateGameRequest, UpdateGameRequest};
    use crate::services::game_service::{
        create_game, delete_game, get_game_by_id, list_games, update_game,
    };

    use sqlx::{
        MySqlPool,
        mysql::{MySqlConnectOptions, MySqlPoolOptions},
    };

    const TEST_PREFIX: &str = "svc_test_";

    // Opt-in legacy DB suite runbook:
    //   Provision and migrate a disposable MySQL database using deployment
    //   tooling that verifies the target and does not load a repository .env.
    //   export TEST_DATABASE_URL='mysql://...@127.0.0.1/hiveweb_test_legacy_game'
    //   cargo test -p hiveweb --lib services::game_service_test -- --ignored --test-threads=1
    async fn pool() -> MySqlPool {
        let url = std::env::var("TEST_DATABASE_URL")
            .expect("legacy game DB tests require a disposable TEST_DATABASE_URL");
        let options = url
            .parse::<MySqlConnectOptions>()
            .expect("TEST_DATABASE_URL must be a valid MySQL URL");
        let host = options.get_host();
        assert!(
            matches!(host, "127.0.0.1" | "localhost" | "::1"),
            "refusing non-loopback MySQL host `{host}` in TEST_DATABASE_URL"
        );
        let database = options
            .get_database()
            .expect("TEST_DATABASE_URL must name a disposable database");
        let database_lower = database.to_ascii_lowercase();
        assert!(
            database_lower == "hiveweb_test" || database_lower.starts_with("hiveweb_test_"),
            "refusing database `{database}`: use `hiveweb_test` or the `hiveweb_test_` prefix"
        );

        MySqlPoolOptions::new()
            .connect_with(options)
            .await
            .expect("disposable test DB connect failed")
    }

    async fn closed_lazy_pool() -> MySqlPool {
        let pool = MySqlPoolOptions::new().connect_lazy_with(MySqlConnectOptions::new());
        pool.close().await;
        pool
    }

    async fn cleanup(pool: &MySqlPool) {
        let pattern = format!("{}%", TEST_PREFIX);
        sqlx::query("DELETE FROM game_alias_entries WHERE game_id IN (SELECT id FROM games WHERE name LIKE ?)")
            .bind(&pattern)
            .execute(pool)
            .await
            .ok();
        sqlx::query("DELETE FROM games WHERE name LIKE ?")
            .bind(&pattern)
            .execute(pool)
            .await
            .ok();
    }

    #[tokio::test]
    async fn validate_name_empty() {
        let p = closed_lazy_pool().await;
        let r = create_game(
            &p,
            CreateGameRequest {
                name: "".to_string(),
                aliases: vec!["a".to_string()],
            },
        )
        .await;
        assert!(r.is_err());
        assert_eq!(r.unwrap_err().code(), 4003);
    }

    #[tokio::test]
    async fn validate_name_whitespace() {
        let p = closed_lazy_pool().await;
        let r = create_game(
            &p,
            CreateGameRequest {
                name: "   ".to_string(),
                aliases: vec!["a".to_string()],
            },
        )
        .await;
        assert!(r.is_err());
        assert_eq!(r.unwrap_err().code(), 4003);
    }

    #[tokio::test]
    async fn validate_name_too_long() {
        let p = closed_lazy_pool().await;
        let r = create_game(
            &p,
            CreateGameRequest {
                name: "a".repeat(51),
                aliases: vec!["a".to_string()],
            },
        )
        .await;
        assert!(r.is_err());
        assert_eq!(r.unwrap_err().code(), 4004);
    }

    #[tokio::test]
    async fn validate_aliases_empty() {
        let p = closed_lazy_pool().await;
        let r = create_game(
            &p,
            CreateGameRequest {
                name: format!("{}aliases_empty", TEST_PREFIX),
                aliases: vec![],
            },
        )
        .await;
        assert!(r.is_err());
        assert_eq!(r.unwrap_err().code(), 4005);
    }

    #[tokio::test]
    async fn validate_alias_too_long() {
        let p = closed_lazy_pool().await;
        let r = create_game(
            &p,
            CreateGameRequest {
                name: format!("{}alias_long", TEST_PREFIX),
                aliases: vec!["a".repeat(51)],
            },
        )
        .await;
        assert!(r.is_err());
        assert_eq!(r.unwrap_err().code(), 4006);
    }

    #[tokio::test]
    async fn validate_aliases_too_many() {
        let p = closed_lazy_pool().await;
        let aliases: Vec<_> = (0..21).map(|i| format!("a{}", i)).collect();
        let r = create_game(
            &p,
            CreateGameRequest {
                name: format!("{}too_many", TEST_PREFIX),
                aliases,
            },
        )
        .await;
        assert!(r.is_err());
        assert_eq!(r.unwrap_err().code(), 4007);
    }

    #[tokio::test]
    #[ignore = "requires a migrated disposable TEST_DATABASE_URL; run with --ignored --test-threads=1"]
    async fn create_success() {
        let p = pool().await;
        cleanup(&p).await;
        let r = create_game(
            &p,
            CreateGameRequest {
                name: format!("{}create_ok", TEST_PREFIX),
                aliases: vec!["c1".to_string(), "c2".to_string()],
            },
        )
        .await;
        assert!(r.is_ok());
        let g = r.unwrap();
        assert_eq!(g.name, format!("{}create_ok", TEST_PREFIX));
        assert_eq!(g.aliases.len(), 2);
        cleanup(&p).await;
    }

    #[tokio::test]
    #[ignore = "requires a migrated disposable TEST_DATABASE_URL; run with --ignored --test-threads=1"]
    async fn create_dedup_aliases() {
        let p = pool().await;
        cleanup(&p).await;
        let r = create_game(
            &p,
            CreateGameRequest {
                name: format!("{}dedup", TEST_PREFIX),
                aliases: vec![
                    "x".to_string(),
                    "x".to_string(),
                    "y".to_string(),
                    "x".to_string(),
                ],
            },
        )
        .await;
        assert!(r.is_ok());
        assert_eq!(r.unwrap().aliases.len(), 2);
        cleanup(&p).await;
    }

    #[tokio::test]
    #[ignore = "requires a migrated disposable TEST_DATABASE_URL; run with --ignored --test-threads=1"]
    async fn create_dup_name() {
        let p = pool().await;
        cleanup(&p).await;
        let name = format!("{}dup_name", TEST_PREFIX);
        create_game(
            &p,
            CreateGameRequest {
                name: name.clone(),
                aliases: vec!["a1".to_string()],
            },
        )
        .await
        .unwrap();
        let r = create_game(
            &p,
            CreateGameRequest {
                name,
                aliases: vec!["a2".to_string()],
            },
        )
        .await;
        assert!(r.is_err());
        assert_eq!(r.unwrap_err().code(), 4002);
        cleanup(&p).await;
    }

    #[tokio::test]
    #[ignore = "requires a migrated disposable TEST_DATABASE_URL; run with --ignored --test-threads=1"]
    async fn create_dup_alias_global() {
        let p = pool().await;
        cleanup(&p).await;
        let shared = "shared_global_alias";
        create_game(
            &p,
            CreateGameRequest {
                name: format!("{}owner", TEST_PREFIX),
                aliases: vec![shared.to_string()],
            },
        )
        .await
        .unwrap();
        let r = create_game(
            &p,
            CreateGameRequest {
                name: format!("{}thief", TEST_PREFIX),
                aliases: vec![shared.to_string()],
            },
        )
        .await;
        assert!(r.is_err());
        assert_eq!(r.unwrap_err().code(), 4008);
        cleanup(&p).await;
    }

    #[tokio::test]
    #[ignore = "requires a migrated disposable TEST_DATABASE_URL; run with --ignored --test-threads=1"]
    async fn get_by_id_ok() {
        let p = pool().await;
        cleanup(&p).await;
        let g = create_game(
            &p,
            CreateGameRequest {
                name: format!("{}fetch", TEST_PREFIX),
                aliases: vec!["f1".to_string(), "f2".to_string()],
            },
        )
        .await
        .unwrap();
        let r = get_game_by_id(&p, g.id).await.unwrap().unwrap();
        assert_eq!(r.name, format!("{}fetch", TEST_PREFIX));
        assert_eq!(r.aliases.len(), 2);
        cleanup(&p).await;
    }

    #[tokio::test]
    #[ignore = "requires a migrated disposable TEST_DATABASE_URL; run with --ignored --test-threads=1"]
    async fn get_by_id_not_found() {
        let p = pool().await;
        assert!(get_game_by_id(&p, 999_999).await.unwrap().is_none());
    }

    #[tokio::test]
    #[ignore = "requires a migrated disposable TEST_DATABASE_URL; run with --ignored --test-threads=1"]
    async fn list_empty() {
        let p = pool().await;
        let r = list_games(&p, 1, 10, Some("no_such_term_xyz"))
            .await
            .unwrap();
        assert_eq!(r.total, 0);
        assert!(r.games.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires a migrated disposable TEST_DATABASE_URL; run with --ignored --test-threads=1"]
    async fn list_search_by_alias() {
        let p = pool().await;
        cleanup(&p).await;
        create_game(
            &p,
            CreateGameRequest {
                name: format!("{}alias_search", TEST_PREFIX),
                aliases: vec!["unique_alias_xyz".to_string()],
            },
        )
        .await
        .unwrap();
        let r = list_games(&p, 1, 10, Some("unique_alias_xyz"))
            .await
            .unwrap();
        assert!(r.total >= 1);
        assert!(r.games.iter().any(|g| g.name.contains("alias_search")));
        cleanup(&p).await;
    }

    #[tokio::test]
    #[ignore = "requires a migrated disposable TEST_DATABASE_URL; run with --ignored --test-threads=1"]
    async fn list_pagination() {
        let p = pool().await;
        cleanup(&p).await;
        for i in 0..5 {
            create_game(
                &p,
                CreateGameRequest {
                    name: format!("{}page_{}", TEST_PREFIX, i),
                    aliases: vec![format!("pa{}", i)],
                },
            )
            .await
            .unwrap();
        }
        let r1 = list_games(&p, 1, 2, Some(&format!("{}page_", TEST_PREFIX)))
            .await
            .unwrap();
        assert_eq!(r1.games.len(), 2);
        assert!(r1.total >= 5);
        let r2 = list_games(&p, 2, 2, Some(&format!("{}page_", TEST_PREFIX)))
            .await
            .unwrap();
        assert_eq!(r2.games.len(), 2);
        let ids1: Vec<_> = r1.games.iter().map(|g| g.id).collect();
        let ids2: Vec<_> = r2.games.iter().map(|g| g.id).collect();
        for id in &ids1 {
            assert!(!ids2.contains(id));
        }
        cleanup(&p).await;
    }

    #[tokio::test]
    #[ignore = "requires a migrated disposable TEST_DATABASE_URL; run with --ignored --test-threads=1"]
    async fn update_name_only() {
        let p = pool().await;
        cleanup(&p).await;
        let g = create_game(
            &p,
            CreateGameRequest {
                name: format!("{}upd_name_old", TEST_PREFIX),
                aliases: vec!["ua".to_string()],
            },
        )
        .await
        .unwrap();
        let r = update_game(
            &p,
            g.id,
            UpdateGameRequest {
                name: Some(format!("{}upd_name_new", TEST_PREFIX)),
                aliases: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(r.name, format!("{}upd_name_new", TEST_PREFIX));
        assert_eq!(r.aliases, vec!["ua".to_string()]);
        cleanup(&p).await;
    }

    #[tokio::test]
    #[ignore = "requires a migrated disposable TEST_DATABASE_URL; run with --ignored --test-threads=1"]
    async fn update_aliases_only() {
        let p = pool().await;
        cleanup(&p).await;
        let g = create_game(
            &p,
            CreateGameRequest {
                name: format!("{}upd_alias", TEST_PREFIX),
                aliases: vec!["old".to_string()],
            },
        )
        .await
        .unwrap();
        let r = update_game(
            &p,
            g.id,
            UpdateGameRequest {
                name: None,
                aliases: Some(vec!["n1".to_string(), "n2".to_string()]),
            },
        )
        .await
        .unwrap();
        assert_eq!(r.aliases.len(), 2);
        cleanup(&p).await;
    }

    #[tokio::test]
    #[ignore = "requires a migrated disposable TEST_DATABASE_URL; run with --ignored --test-threads=1"]
    async fn update_both() {
        let p = pool().await;
        cleanup(&p).await;
        let g = create_game(
            &p,
            CreateGameRequest {
                name: format!("{}upd_both_old", TEST_PREFIX),
                aliases: vec!["oba".to_string()],
            },
        )
        .await
        .unwrap();
        let r = update_game(
            &p,
            g.id,
            UpdateGameRequest {
                name: Some(format!("{}upd_both_new", TEST_PREFIX)),
                aliases: Some(vec!["nba".to_string()]),
            },
        )
        .await
        .unwrap();
        assert_eq!(r.name, format!("{}upd_both_new", TEST_PREFIX));
        assert_eq!(r.aliases, vec!["nba".to_string()]);
        cleanup(&p).await;
    }

    #[tokio::test]
    #[ignore = "requires a migrated disposable TEST_DATABASE_URL; run with --ignored --test-threads=1"]
    async fn update_no_fields() {
        let p = pool().await;
        cleanup(&p).await;
        let g = create_game(
            &p,
            CreateGameRequest {
                name: format!("{}upd_empty", TEST_PREFIX),
                aliases: vec!["ea".to_string()],
            },
        )
        .await
        .unwrap();
        let r = update_game(
            &p,
            g.id,
            UpdateGameRequest {
                name: None,
                aliases: None,
            },
        )
        .await;
        assert!(r.is_err());
        assert_eq!(r.unwrap_err().code(), 4000);
        cleanup(&p).await;
    }

    #[tokio::test]
    #[ignore = "requires a migrated disposable TEST_DATABASE_URL; run with --ignored --test-threads=1"]
    async fn update_not_found() {
        let p = pool().await;
        let r = update_game(
            &p,
            999_999,
            UpdateGameRequest {
                name: Some("x".to_string()),
                aliases: None,
            },
        )
        .await;
        assert!(r.is_err());
        assert_eq!(r.unwrap_err().code(), 4001);
    }

    #[tokio::test]
    #[ignore = "requires a migrated disposable TEST_DATABASE_URL; run with --ignored --test-threads=1"]
    async fn update_dup_alias() {
        let p = pool().await;
        cleanup(&p).await;
        create_game(
            &p,
            CreateGameRequest {
                name: format!("{}ea_owner", TEST_PREFIX),
                aliases: vec!["protected".to_string()],
            },
        )
        .await
        .unwrap();
        let g = create_game(
            &p,
            CreateGameRequest {
                name: format!("{}ea_target", TEST_PREFIX),
                aliases: vec!["t1".to_string()],
            },
        )
        .await
        .unwrap();
        let r = update_game(
            &p,
            g.id,
            UpdateGameRequest {
                name: None,
                aliases: Some(vec!["protected".to_string()]),
            },
        )
        .await;
        assert!(r.is_err());
        assert_eq!(r.unwrap_err().code(), 4008);
        cleanup(&p).await;
    }

    #[tokio::test]
    #[ignore = "requires a migrated disposable TEST_DATABASE_URL; run with --ignored --test-threads=1"]
    async fn delete_ok() {
        let p = pool().await;
        cleanup(&p).await;
        let g = create_game(
            &p,
            CreateGameRequest {
                name: format!("{}del", TEST_PREFIX),
                aliases: vec!["d1".to_string(), "d2".to_string()],
            },
        )
        .await
        .unwrap();
        assert!(delete_game(&p, g.id).await.unwrap());
        assert!(get_game_by_id(&p, g.id).await.unwrap().is_none());
        cleanup(&p).await;
    }

    #[tokio::test]
    #[ignore = "requires a migrated disposable TEST_DATABASE_URL; run with --ignored --test-threads=1"]
    async fn delete_cascade() {
        let p = pool().await;
        cleanup(&p).await;
        let g = create_game(
            &p,
            CreateGameRequest {
                name: format!("{}cascade", TEST_PREFIX),
                aliases: vec!["ca1".to_string(), "ca2".to_string()],
            },
        )
        .await
        .unwrap();
        delete_game(&p, g.id).await.unwrap();
        let (cnt,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM game_alias_entries WHERE game_id = ?")
                .bind(g.id)
                .fetch_one(&p)
                .await
                .unwrap();
        assert_eq!(cnt, 0);
        cleanup(&p).await;
    }

    #[tokio::test]
    #[ignore = "requires a migrated disposable TEST_DATABASE_URL; run with --ignored --test-threads=1"]
    async fn delete_not_found() {
        let p = pool().await;
        assert!(!delete_game(&p, 999_999).await.unwrap());
    }
}
