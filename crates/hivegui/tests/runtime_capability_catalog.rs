use hivegui::datasource::{Store, entity_store::Capability};

#[tokio::test]
async fn store_registers_runtime_capability_catalog_and_preserves_custom_entries() {
    let db_dir = tempfile::tempdir().expect("create temporary HiveGUI data directory");
    let store = Store::new(db_dir.path())
        .await
        .expect("create HiveGUI store");

    Capability::create(
        store.pool(),
        "net".to_string(),
        "user-defined network group".to_string(),
        false,
        None,
    )
    .await
    .expect("create custom capability");
    drop(store);

    let reopened = Store::new(db_dir.path())
        .await
        .expect("reopen HiveGUI store");
    let network_http = Capability::get(reopened.pool(), "network.http")
        .await
        .expect("query standard capability")
        .expect("network.http is registered at startup");

    assert!(network_http.is_dangerous);
    assert!(
        Capability::get(reopened.pool(), "net")
            .await
            .expect("query custom capability")
            .is_some(),
        "startup catalog registration must preserve user-defined capabilities"
    );
}
