//! T129/T130 [US13] Age-encrypted backup export + restore contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T129
//! and §T130 (export + restore / switch verification).
//!
//! Red public boundaries the T129/T130 implementation must satisfy:
//!   - `hivegui::datasource::backup::BackupExporter::export_age` writes
//!     a passphrase-encrypted age stream that round-trips through
//!     [`BackupImporter::import_age`] using a matching passphrase.
//!   - The portable export carries the exact v4 `manifest.json`, all 22 user
//!     entity JSON files (including empty types), and owned Plugin WASM. The
//!     separate restore instance carries the v1 six-tuple unarmed manifest.
//!   - The export refuses to overwrite an existing target file (the
//!     caller is responsible for choosing a fresh target).
//!   - The export refuses symlinks / hardlinks / device files /
//!     FIFOs / sockets and other non-regular entries.
//!   - Format 1/2/3 archives are accepted and upgraded once. Legacy integer
//!     kinds and dotted Builtins are mapped only for legacy formats; current
//!     archives reject them and all short `node_type` values.
//!   - The restore side writes to a staging directory and refuses to
//!     touch the canonical `datasources.db` directly.

#![allow(missing_docs)]

mod support;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use hivegui::datasource::backup::{
    BACKUP_CONFIRMATION_CRASH_POINTS, BackupCoordinator, BackupExporter, BackupImporter,
    ExportError, ImportError, RESTORE_SWITCH_CRASH_POINTS, RETIREMENT_CRASH_POINTS,
    RestoreCoordinator, RetirementOutcome, SIDECAR_CLEANUP_CRASH_POINTS,
};
use hivegui::datasource::{Crypto, Store, StoreOpenOptions};
use sha2::{Digest, Sha256};
use support::TestWorkspace;
use support::sensitive_canary::{
    SensitiveField, place_canary_for_test, scan_all_mediums_for_test, unique_canary_payload,
};
use uuid::Uuid;

fn unique_target_path(workspace: &TestWorkspace, label: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    workspace.root().join(format!("{label}-{nanos}.age.tar"))
}

async fn write_seed_database(workspace: &TestWorkspace) -> std::path::PathBuf {
    let store = Store::open_local(StoreOpenOptions::new(
        workspace.database_path(),
        workspace.plugin_root(),
    ))
    .await
    .expect("open real v4 seed Store");
    store
        .create(
            "backup-seed",
            "127.0.0.1",
            3306,
            "seed-user",
            b"seed-password",
        )
        .await
        .expect("seed real user entity");
    drop(store);
    workspace.database_path().to_path_buf()
}

async fn write_restore_current_database(root: &Path) -> std::path::PathBuf {
    let database = root.join("datasources.db");
    let store = Store::open_local(StoreOpenOptions::new(&database, root.join("plugins")))
        .await
        .expect("open exact current restore layout");
    store
        .create(
            "sidecar-current-seed",
            "127.0.0.1",
            3306,
            "sidecar-user",
            b"sidecar-password",
        )
        .await
        .expect("seed exact current restore database");
    drop(store);
    database
}

fn sha256_path(path: &Path) -> String {
    hex::encode(Sha256::digest(fs::read(path).expect("read hash input")))
}

fn sidecar_file_identity(path: &Path) -> String {
    let metadata = fs::metadata(path).expect("read sidecar identity");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;

        format!("unix:{}:{}", metadata.dev(), metadata.ino())
    }
    #[cfg(not(unix))]
    {
        let modified = metadata
            .modified()
            .expect("sidecar modified time")
            .duration_since(UNIX_EPOCH)
            .expect("sidecar modified after epoch")
            .as_nanos();
        format!("portable:{}:{modified}", metadata.len())
    }
}

fn sidecar_cleanup_token(db_id: &str) -> String {
    let bytes = db_id.as_bytes();
    let mut digest = Sha256::new();
    digest.update(b"hivegui-sidecar-cleanup-v1");
    digest.update([0]);
    digest.update((bytes.len() as u32).to_be_bytes());
    digest.update(bytes);
    hex::encode(digest.finalize())
}

struct SidecarCleanupJournalFixture<'a> {
    artifact: &'a str,
    canonical_name: &'a str,
    sidecar_identity: &'a str,
    sidecar_size: u64,
    sidecar_sha256: &'a str,
    cleanup_operation_id: Uuid,
    state: &'a str,
}

fn write_sidecar_cleanup_journal(
    root: &Path,
    database: &Path,
    fixture: SidecarCleanupJournalFixture<'_>,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let SidecarCleanupJournalFixture {
        artifact,
        canonical_name,
        sidecar_identity,
        sidecar_size,
        sidecar_sha256,
        cleanup_operation_id,
        state,
    } = fixture;
    let token = sidecar_cleanup_token("current");
    let journal = root.join(format!(
        ".hivegui-sidecar-cleanup-v1-{token}-{artifact}.json"
    ));
    let quarantine_name =
        format!(".hivegui-sidecar-quarantine-v1-{cleanup_operation_id}-{artifact}");
    let quarantine = root.join(&quarantine_name);
    let record = serde_json::json!({
        "schema_version": 1,
        "cleanup_operation_id": cleanup_operation_id,
        "db_id": "current",
        "database_identity": sidecar_file_identity(database),
        "artifact": artifact,
        "canonical_name": canonical_name,
        "expected_identity": sidecar_identity,
        "expected_size_bytes": sidecar_size,
        "expected_sha256": sidecar_sha256,
        "quarantine_name": quarantine_name,
        "state": state,
    });
    fs::write(
        &journal,
        serde_json::to_vec(&record).expect("encode cleanup journal"),
    )
    .expect("write cleanup journal fixture");
    (journal, quarantine)
}

fn write_authenticated_tar_fixture(path: &Path, passphrase: &str, entries: &[(&str, &[u8])]) {
    use age::Encryptor;
    use age::secrecy::SecretString;
    use flate2::write::GzEncoder;

    let output = fs::File::create(path).expect("create authenticated fixture");
    let encryptor =
        Encryptor::with_user_passphrase(SecretString::new(passphrase.to_string().into_boxed_str()));
    let age = encryptor.wrap_output(output).expect("wrap age output");
    let gzip = GzEncoder::new(age, flate2::Compression::default());
    let mut tar = tar::Builder::new(gzip);
    for (name, bytes) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o600);
        header.set_path(name).expect("fixture path");
        header.set_cksum();
        tar.append(&header, *bytes).expect("append fixture entry");
    }
    let gzip = tar.into_inner().expect("finish fixture tar");
    let age = gzip.finish().expect("finish fixture gzip");
    age.finish().expect("finish fixture age");
}

fn write_authenticated_entry_map(
    path: &Path,
    passphrase: &str,
    entries: &BTreeMap<String, Vec<u8>>,
) {
    let borrowed = entries
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect::<Vec<_>>();
    write_authenticated_tar_fixture(path, passphrase, &borrowed);
}

fn rewrite_portable_entity_archive(
    source: &Path,
    target: &Path,
    passphrase: &str,
    format_version: u64,
    mut rewrite: impl FnMut(&str, &mut serde_json::Value),
) {
    let mut entries = read_authenticated_tar_entries(source, passphrase);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(entries.get("manifest.json").expect("source manifest"))
            .expect("parse source manifest");
    manifest["format_version"] = serde_json::json!(format_version);
    for descriptor in manifest["entity_files"]
        .as_array_mut()
        .expect("entity descriptors")
    {
        let name = descriptor["name"]
            .as_str()
            .expect("entity name")
            .to_string();
        let path = descriptor["path"]
            .as_str()
            .expect("entity path")
            .to_string();
        let mut body: serde_json::Value =
            serde_json::from_slice(entries.get(&path).expect("entity body"))
                .expect("parse entity body");
        rewrite(&name, &mut body);
        let bytes = serde_json::to_vec(&body).expect("encode rewritten entity body");
        descriptor["count"] = serde_json::json!(
            body["rows"]
                .as_array()
                .expect("rewritten entity rows")
                .len()
        );
        descriptor["sha256"] = serde_json::json!(hex::encode(Sha256::digest(&bytes)));
        entries.insert(path, bytes);
    }
    entries.insert(
        "manifest.json".into(),
        serde_json::to_vec(&manifest).expect("encode rewritten manifest"),
    );
    write_authenticated_entry_map(target, passphrase, &entries);
}

fn read_authenticated_tar_entries(path: &Path, passphrase: &str) -> BTreeMap<String, Vec<u8>> {
    use age::Decryptor;
    use age::secrecy::SecretString;
    use flate2::read::GzDecoder;

    let decryptor = Decryptor::new(fs::File::open(path).expect("open authenticated archive"))
        .expect("parse age envelope");
    let identity =
        age::scrypt::Identity::new(SecretString::new(passphrase.to_string().into_boxed_str()));
    let reader = decryptor
        .decrypt(std::iter::once(&identity as &dyn age::Identity))
        .expect("authenticate age envelope");
    let mut archive = tar::Archive::new(GzDecoder::new(reader));
    let mut entries = BTreeMap::new();
    for entry in archive.entries().expect("read tar entries") {
        let mut entry = entry.expect("read tar entry");
        let path = entry
            .path()
            .expect("read UTF-8 archive path")
            .to_str()
            .expect("archive paths are UTF-8")
            .to_string();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).expect("read archive entry");
        assert!(
            entries.insert(path, bytes).is_none(),
            "archive paths are unique"
        );
    }
    entries
}

async fn seed_complete_portable_fixture(
    workspace: &TestWorkspace,
    store: &Store,
) -> BTreeMap<&'static str, Vec<u8>> {
    const AT: &str = "2026-08-26T00:00:00Z";
    let device_key: [u8; 32] = fs::read(
        workspace
            .database_path()
            .parent()
            .expect("source database parent")
            .join("encryption.key"),
    )
    .expect("read source device key")
    .try_into()
    .expect("source device key is exactly 32 bytes");
    let crypto = Crypto::new(&device_key);
    let secrets = BTreeMap::from([
        ("datasource", b"portable-datasource-secret".to_vec()),
        ("provider", b"portable-provider-token".to_vec()),
        ("session", b"portable-session-title".to_vec()),
        ("message", b"portable-message-content".to_vec()),
        ("tool_calls", br#"[{"name":"portable-tool"}]"#.to_vec()),
        ("execution", br#"{"step":"portable-state"}"#.to_vec()),
    ]);

    store
        .create(
            "portable-complete-datasource",
            "127.0.0.1",
            3307,
            "portable-user",
            secrets["datasource"].as_slice(),
        )
        .await
        .expect("seed portable DataSource");

    let category_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO categories (parent_id,name,slug,description,created_at,updated_at) \
         VALUES (NULL,'Portable Root','portable-root','root category',?,?) RETURNING id",
    )
    .bind(AT)
    .bind(AT)
    .fetch_one(store.pool())
    .await
    .expect("seed root Category");
    sqlx::query(
        "INSERT INTO categories (parent_id,name,slug,description,created_at,updated_at) \
         VALUES (?,'Portable Child','portable-child','child category',?,?)",
    )
    .bind(category_id)
    .bind(AT)
    .bind(AT)
    .execute(store.pool())
    .await
    .expect("seed child Category relation");
    sqlx::query(
        "INSERT INTO capabilities (name,description,is_dangerous,category_id,normalized_name,created_at) \
         VALUES ('backup.full.capability','portable capability',1,?,'backup.full.capability',?)",
    )
    .bind(category_id)
    .bind(AT)
    .execute(store.pool())
    .await
    .expect("seed Capability");
    sqlx::query(
        "INSERT INTO global_configs (name,key,type,data,created_at,updated_at) \
         VALUES ('Portable Config','portable.complete','json','{\"enabled\":true}',?,?)",
    )
    .bind(AT)
    .bind(AT)
    .execute(store.pool())
    .await
    .expect("seed GlobalConfig");
    let provider_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO llm_providers (name,category,base_url,token_env,token_encrypted,created_at,updated_at) \
         VALUES ('portable-provider','openai','https://portable.invalid/v1','',?,?,?) RETURNING id",
    )
    .bind(crypto.encrypt(&secrets["provider"]).expect("encrypt Provider token"))
    .bind(AT)
    .bind(AT)
    .fetch_one(store.pool())
    .await
    .expect("seed LlmProvider");
    let preset_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO llm_presets (name,description,is_default,max_tokens,temperature,created_at,updated_at) \
         VALUES ('portable-preset','portable preset',0,4097,0.25,?,?) RETURNING id",
    )
    .bind(AT)
    .bind(AT)
    .fetch_one(store.pool())
    .await
    .expect("seed LlmPreset");
    sqlx::query(
        "INSERT INTO models (name,preset_id,provider_id,priority,created_at,updated_at) \
         VALUES ('portable-model',?,?,7,?,?)",
    )
    .bind(preset_id)
    .bind(provider_id)
    .bind(AT)
    .bind(AT)
    .execute(store.pool())
    .await
    .expect("seed Model relation");
    sqlx::query(
        "INSERT INTO tags (name,color,normalized_name,created_at,updated_at) \
         VALUES ('Portable Tag','#123456','portable tag',?,?)",
    )
    .bind(AT)
    .bind(AT)
    .execute(store.pool())
    .await
    .expect("seed Tag");

    let wasm_fixtures = [
        ("portable-one", b"\0asmT129-complete-one".as_slice()),
        ("portable-two", b"\0asmT129-complete-two".as_slice()),
    ];
    let mut plugin_ids = Vec::new();
    for (index, (identifier, wasm)) in wasm_fixtures.into_iter().enumerate() {
        let key = format!("complete/{identifier}/plugin.wasm");
        let path = workspace.plugin_root().join(&key);
        fs::create_dir_all(path.parent().expect("Plugin artifact parent"))
            .expect("create Plugin artifact parent");
        fs::write(&path, wasm).expect("write managed Plugin artifact");
        let plugin_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO plugins \
             (identifier,name,description,manifest,version,author,repository_url,s3_key,sha256,size_bytes,runtime,category_id,capabilities,resource_limits,row_revision,created_at,updated_at,deleted_at) \
             VALUES (?,?,?,?,'1.2.3','portable-author','https://portable.invalid/repo',?,?,?,'wasm32',?,'[\"backup.full.capability\"]','{\"timeout_ms\":30000,\"memory_mib\":128,\"max_output_bytes\":1048576}',?,?,?,NULL) \
             RETURNING id",
        )
        .bind(identifier)
        .bind(format!("Portable Plugin {index}"))
        .bind(format!("portable Plugin description {index}"))
        .bind(format!("{{\"name\":\"{identifier}\",\"abi\":\"hive-extism/v1\"}}"))
        .bind(&key)
        .bind(hex::encode(Sha256::digest(wasm)))
        .bind(i64::try_from(wasm.len()).expect("WASM fixture size"))
        .bind(category_id)
        .bind(i64::try_from(index + 1).expect("row revision"))
        .bind(AT)
        .bind(AT)
        .fetch_one(store.pool())
        .await
        .expect("seed Plugin row");
        plugin_ids.push(plugin_id);
    }

    let function_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO functions \
         (identifier,name,description,kind,input_schema,output_schema,plugin_id,plugin_export,category_id,required_capabilities,created_at,updated_at) \
         VALUES ('portable-custom','Portable Custom','portable function','custom','{\"type\":\"object\"}','{\"type\":\"object\"}',?,'echo',?,'[\"backup.full.capability\"]',?,?) RETURNING id",
    )
    .bind(plugin_ids[0])
    .bind(category_id)
    .bind(AT)
    .bind(AT)
    .fetch_one(store.pool())
    .await
    .expect("seed Function relation");
    let workflow_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO workflows \
         (identifier,name,description,timeout_ms,category_id,input_schema,start_description,output_schema,required_capabilities,created_at,updated_at) \
         VALUES ('portable-workflow','Portable Workflow','portable workflow',45678,?,'{\"type\":\"object\"}','portable start','{\"type\":\"object\"}','[\"backup.full.capability\"]',?,?) RETURNING id",
    )
    .bind(category_id)
    .bind(AT)
    .bind(AT)
    .fetch_one(store.pool())
    .await
    .expect("seed Workflow");
    for (node_key, node_type, linked_function, x, y) in [
        ("start", "start_node", None, 1.5, 2.5),
        ("function", "function_node", Some(function_id), 3.5, 4.5),
        ("answer", "generate_answer_node", None, 5.5, 6.5),
        ("end", "end_node", None, 7.5, 8.5),
    ] {
        sqlx::query(
            "INSERT INTO workflow_nodes \
             (workflow_id,node_key,node_type,function_id,position_x,position_y,node_config,created_at) \
             VALUES (?,?,?,?,?,?,?,?)",
        )
        .bind(workflow_id)
        .bind(node_key)
        .bind(node_type)
        .bind(linked_function)
        .bind(x)
        .bind(y)
        .bind(format!("{{\"fixture\":\"{node_key}\"}}"))
        .bind(AT)
        .execute(store.pool())
        .await
        .expect("seed WorkflowNode");
    }
    for (source, target) in [
        ("start", "function"),
        ("function", "answer"),
        ("answer", "end"),
    ] {
        sqlx::query(
            "INSERT INTO workflow_edges (workflow_id,src_node_key,dst_node_key,mapping) \
             VALUES (?,?,?,'{\"value\":\"result\"}')",
        )
        .bind(workflow_id)
        .bind(source)
        .bind(target)
        .execute(store.pool())
        .await
        .expect("seed WorkflowEdge");
    }
    let function_tool_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO tools \
         (identifier,name,description,kind,source,is_always,function_id,workflow_id,input_schema,output_schema,category_id,required_capabilities,created_at,updated_at) \
         VALUES ('portable-function-tool','Portable Function Tool','function tool','function-wrap','workspace',1,?,NULL,'{\"type\":\"object\"}','{\"type\":\"object\"}',?,'[\"backup.full.capability\"]',?,?) RETURNING id",
    )
    .bind(function_id)
    .bind(category_id)
    .bind(AT)
    .bind(AT)
    .fetch_one(store.pool())
    .await
    .expect("seed Function Tool");
    sqlx::query(
        "INSERT INTO tools \
         (identifier,name,description,kind,source,is_always,function_id,workflow_id,input_schema,output_schema,category_id,required_capabilities,created_at,updated_at) \
         VALUES ('portable-workflow-tool','Portable Workflow Tool','workflow tool','workflow-wrap','workspace',0,NULL,?,'{\"type\":\"object\"}','{\"type\":\"object\"}',?,NULL,?,?)",
    )
    .bind(workflow_id)
    .bind(category_id)
    .bind(AT)
    .bind(AT)
    .execute(store.pool())
    .await
    .expect("seed Workflow Tool");
    let skill_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO skills \
         (identifier,name,description,frontmatter,content,source,is_always,category_id,required_capabilities,created_at,updated_at) \
         VALUES ('portable-skill','Portable Skill','portable skill','---\ntitle: portable\n---','portable skill content','workspace',1,?,'[\"backup.full.capability\"]',?,?) RETURNING id",
    )
    .bind(category_id)
    .bind(AT)
    .bind(AT)
    .fetch_one(store.pool())
    .await
    .expect("seed Skill");
    let root_agent_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO agents \
         (identifier,name,description,system_prompt,parent_agent_id,depth,is_default,model_preset,category_id,name_normalized,created_at,updated_at) \
         VALUES ('portable-root-agent','Portable Root Agent','root agent','portable root prompt',NULL,0,1,'portable-preset',?,'portable root agent',?,?) RETURNING id",
    )
    .bind(category_id)
    .bind(AT)
    .bind(AT)
    .fetch_one(store.pool())
    .await
    .expect("seed root Agent");
    let child_agent_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO agents \
         (identifier,name,description,system_prompt,parent_agent_id,depth,is_default,model_preset,category_id,name_normalized,created_at,updated_at) \
         VALUES ('portable-child-agent','Portable Child Agent','child agent','portable child prompt',?,1,0,'portable-preset',?,'portable child agent',?,?) RETURNING id",
    )
    .bind(root_agent_id)
    .bind(category_id)
    .bind(AT)
    .bind(AT)
    .fetch_one(store.pool())
    .await
    .expect("seed child Agent relation");
    sqlx::query("INSERT INTO agent_tools (agent_id,tool_id,created_at) VALUES (?,?,?)")
        .bind(root_agent_id)
        .bind(function_tool_id)
        .bind(AT)
        .execute(store.pool())
        .await
        .expect("seed Agent Tool relation");
    sqlx::query("INSERT INTO agent_skills (agent_id,skill_id,created_at) VALUES (?,?,?)")
        .bind(child_agent_id)
        .bind(skill_id)
        .bind(AT)
        .execute(store.pool())
        .await
        .expect("seed Agent Skill relation");
    sqlx::query(
        "INSERT INTO agent_capabilities (agent_id,capability_name,created_at) VALUES (?,'backup.full.capability',?)",
    )
    .bind(root_agent_id)
    .bind(AT)
    .execute(store.pool())
    .await
    .expect("seed Agent Capability relation");

    let session_id = "00000000-0000-4000-8000-000000001219";
    let execution_id = "00000000-0000-4000-8000-000000001220";
    sqlx::query(
        "INSERT INTO chat_sessions \
         (id,entry_agent_id,current_agent_id,title_encrypted,status,execution_id,created_at,updated_at,expires_at) \
         VALUES (?,?,?,?, 'completed', ?,?,?, '2126-08-26T00:00:00Z')",
    )
    .bind(session_id)
    .bind(root_agent_id)
    .bind(child_agent_id)
    .bind(crypto.encrypt(&secrets["session"]).expect("encrypt session title"))
    .bind(execution_id)
    .bind(AT)
    .bind(AT)
    .execute(store.pool())
    .await
    .expect("seed ChatSession");
    sqlx::query(
        "INSERT INTO chat_messages \
         (session_id,seq,role,content_encrypted,tool_calls_encrypted,created_at) \
         VALUES (?,1,'assistant',?,?,?)",
    )
    .bind(session_id)
    .bind(
        crypto
            .encrypt(&secrets["message"])
            .expect("encrypt message content"),
    )
    .bind(
        crypto
            .encrypt(&secrets["tool_calls"])
            .expect("encrypt tool calls"),
    )
    .bind(AT)
    .execute(store.pool())
    .await
    .expect("seed ChatMessage");
    sqlx::query(
        "INSERT INTO agent_executions \
         (execution_id,session_id,current_agent_id,status,state_encrypted,started_at,finished_at,error_kind) \
         VALUES (?,?,?,'completed',?,?,?,'portable_terminal')",
    )
    .bind(execution_id)
    .bind(session_id)
    .bind(child_agent_id)
    .bind(
        crypto
            .encrypt(&secrets["execution"])
            .expect("encrypt execution state"),
    )
    .bind(AT)
    .bind(AT)
    .execute(store.pool())
    .await
    .expect("seed AgentExecution");

    secrets
}

#[tokio::test(flavor = "current_thread")]
async fn export_then_import_round_trip_recovers_seed_database() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = write_seed_database(&workspace).await;
    let target = unique_target_path(&workspace, "roundtrip");

    let exporter = BackupExporter::new(&database);
    let passphrase = "T129-roundtrip-passphrase";
    exporter
        .export_age(&target, passphrase)
        .await
        .expect("export must succeed for a fresh target with a valid source");

    assert!(target.exists(), "export target must be created");
    assert!(
        target.metadata().expect("metadata").len() > 0,
        "export target must contain ciphertext"
    );

    let importer = BackupImporter::new(workspace.root().join("restore-staging"));
    let restored_database = importer
        .import_age(&target, passphrase, workspace.root().join("restore-out"))
        .await
        .expect("import must succeed with matching passphrase");
    let restored = Store::open_local(StoreOpenOptions::new(
        &restored_database,
        restored_database
            .parent()
            .expect("restore parent")
            .join("plugins"),
    ))
    .await
    .expect("open restored v4 Store");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM data_sources WHERE name='backup-seed'")
            .fetch_one(restored.pool())
            .await
            .expect("restored seed count"),
        1,
        "round-trip must preserve the user entity"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn export_refuses_overwrite_of_existing_target() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = write_seed_database(&workspace).await;
    let target = unique_target_path(&workspace, "overwrite");
    fs::write(&target, b"existing-artifact").expect("write target");

    let exporter = BackupExporter::new(&database);
    let result = exporter
        .export_age(&target, "T129-overwrite-passphrase")
        .await;
    match result {
        Err(ExportError::TargetExists(_)) => {}
        other => panic!("expected TargetExists, got {other:?}"),
    }
    // Original target must remain unchanged.
    let bytes = fs::read(&target).expect("read target");
    assert_eq!(
        bytes, b"existing-artifact",
        "target must not be overwritten"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn export_refuses_symlinked_source() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = write_seed_database(&workspace).await;
    // Replace the database with a symlink to a writable file.
    fs::remove_file(&database).expect("remove seed");
    let target_link = workspace.root().join("redirected-seed");
    fs::write(&target_link, b"redirected-bytes").expect("write link target");
    std::os::unix::fs::symlink(&target_link, &database).expect("symlink");

    let exporter = BackupExporter::new(&database);
    let result = exporter
        .export_age(
            &unique_target_path(&workspace, "symlink"),
            "T129-symlink-passphrase",
        )
        .await;
    match result {
        Err(ExportError::UnsafeSource(_)) => {}
        other => panic!("expected UnsafeSource, got {other:?}"),
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn export_refuses_an_intermediate_symlink_in_the_database_source_path() {
    use std::os::unix::fs::symlink;

    let workspace = TestWorkspace::new().expect("workspace");
    let outside = workspace.root().join("outside-source");
    fs::create_dir(&outside).expect("create outside source");
    let database = outside.join("datasources.db");
    fs::write(&database, b"SQLite format 3\0outside-source-canary\0")
        .expect("write outside database canary");
    let alias = workspace.root().join("source-alias");
    symlink(&outside, &alias).expect("plant source parent symlink");
    let archive = unique_target_path(&workspace, "source-intermediate-symlink");

    let error = BackupExporter::new(alias.join("datasources.db"))
        .export_age(&archive, "T129-source-parent-passphrase")
        .await
        .expect_err("source intermediate symlink must be rejected before reading");
    assert!(matches!(error, ExportError::UnsafeSource(_)));
    assert!(!archive.exists());
    assert_eq!(
        fs::read(database).expect("outside database canary remains"),
        b"SQLite format 3\0outside-source-canary\0"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn import_with_wrong_passphrase_is_rejected() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = write_seed_database(&workspace).await;
    let target = unique_target_path(&workspace, "wrongpass");

    let exporter = BackupExporter::new(&database);
    exporter
        .export_age(&target, "T129-correct-passphrase")
        .await
        .expect("export");

    let importer = BackupImporter::new(workspace.root().join("restore-staging"));
    let result = importer
        .import_age(
            &target,
            "T129-wrong-passphrase",
            workspace.root().join("restore-out"),
        )
        .await;
    match result {
        Err(ImportError::AuthenticationFailed) => {}
        other => panic!("expected AuthenticationFailed, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn authenticated_stream_truncation_and_tail_tamper_publish_no_target() {
    let workspace = TestWorkspace::new().expect("workspace");
    let database = write_seed_database(&workspace).await;
    let archive = unique_target_path(&workspace, "authenticated-stream");
    BackupExporter::new(database)
        .export_age(&archive, "T129-authenticated-stream-passphrase")
        .await
        .expect("create valid authenticated archive");
    let original = fs::read(&archive).expect("read authenticated archive");
    assert!(
        original.len() > 64,
        "fixture must contain an authenticated body"
    );

    let mut tampered = original.clone();
    let tail_index = tampered.len() - 17;
    tampered[tail_index] ^= 0x80;
    let variants = [
        ("truncated", original[..original.len() - 1].to_vec()),
        ("tampered", tampered),
    ];
    let importer = BackupImporter::new(workspace.root().join("auth-failure-staging"));
    for (label, bytes) in variants {
        let invalid = unique_target_path(&workspace, &format!("authenticated-{label}"));
        fs::write(&invalid, bytes).expect("write private invalid archive copy");
        let target = workspace
            .root()
            .join(format!("authenticated-{label}-target"));
        let error = importer
            .import_age(&invalid, "T129-authenticated-stream-passphrase", &target)
            .await
            .expect_err("authentication end failure must reject the whole archive");
        assert!(
            matches!(
                error,
                ImportError::AuthenticationFailed | ImportError::Age(_)
            ),
            "authenticated {label} error must stay in the authentication envelope: {error:?}"
        );
        assert!(!target.exists());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn import_rejects_duplicate_manifest_and_database_entries_before_staging() {
    let workspace = TestWorkspace::new().expect("workspace");
    let manifest = serde_json::to_vec(&serde_json::json!({
        "schema_version": 4,
        "role": "primary",
        "uuid": "00000000-0000-4000-8000-000000000129",
        "restore_db_id": "00000000-0000-4000-8000-000000000130",
        "database_name": "datasources.db",
        "ownership_state": "unarmed",
        "format": 3
    }))
    .expect("manifest fixture");
    let database = b"SQLite format 3\0duplicate-entry-fixture\0";
    let importer = BackupImporter::new(workspace.root().join("duplicate-entry-staging"));
    for (label, entries) in [
        (
            "manifest",
            vec![
                ("manifest.json", manifest.as_slice()),
                ("manifest.json", manifest.as_slice()),
                ("datasources.db", database.as_slice()),
            ],
        ),
        (
            "database",
            vec![
                ("manifest.json", manifest.as_slice()),
                ("datasources.db", database.as_slice()),
                ("datasources.db", database.as_slice()),
            ],
        ),
    ] {
        let archive = unique_target_path(&workspace, &format!("duplicate-{label}"));
        write_authenticated_tar_fixture(&archive, "T129-duplicate-entry-passphrase", &entries);
        let target = workspace.root().join(format!("duplicate-{label}-target"));
        let error = importer
            .import_age(&archive, "T129-duplicate-entry-passphrase", &target)
            .await
            .expect_err("duplicate canonical entry must be rejected");
        assert!(matches!(error, ImportError::InvalidManifest(_)));
        assert!(!target.exists(), "invalid archive must create no target");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn import_missing_manifested_entity_cleans_every_staged_and_target_byte() {
    let workspace = TestWorkspace::new().expect("workspace");
    let database_path = write_seed_database(&workspace).await;
    let valid = unique_target_path(&workspace, "missing-entity-source");
    let passphrase = "T129-missing-entity-passphrase";
    BackupExporter::new(database_path)
        .export_age(&valid, passphrase)
        .await
        .expect("export valid entity archive");
    let mut entries = read_authenticated_tar_entries(&valid, passphrase);
    entries.remove("entities/data_sources.json");
    let archive = unique_target_path(&workspace, "missing-entity");
    write_authenticated_entry_map(&archive, passphrase, &entries);
    let staging = workspace.root().join("missing-entity-staging");
    let target = workspace.root().join("missing-entity-target");
    let error = BackupImporter::new(&staging)
        .import_age(&archive, passphrase, &target)
        .await
        .expect_err("a current portable archive missing a manifested entity must fail closed");
    assert!(matches!(error, ImportError::InvalidManifest(_)));
    assert!(
        !target.exists(),
        "failed import must publish no target tree"
    );
    assert!(
        !staging.exists()
            || fs::read_dir(&staging)
                .expect("staging directory")
                .next()
                .is_none(),
        "failed import must leave no staged instance"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn import_rejects_noncanonical_or_mismatched_portable_manifest_fields() {
    let workspace = TestWorkspace::new().expect("workspace");
    let database = write_seed_database(&workspace).await;
    let passphrase = "T129-manifest-identity-passphrase";
    let valid_archive = unique_target_path(&workspace, "manifest-valid-source");
    BackupExporter::new(database)
        .export_age(&valid_archive, passphrase)
        .await
        .expect("export valid portable manifest source");
    let valid_entries = read_authenticated_tar_entries(&valid_archive, passphrase);
    let valid: serde_json::Value =
        serde_json::from_slice(valid_entries.get("manifest.json").expect("valid manifest"))
            .expect("parse valid manifest");
    let importer = BackupImporter::new(workspace.root().join("manifest-identity-staging"));
    for (label, field, value) in [
        ("schema", "schema_version", serde_json::json!(3)),
        ("format-name", "format", serde_json::json!("other-backup")),
        (
            "exported-at",
            "exported_at",
            serde_json::json!("not-rfc3339"),
        ),
    ] {
        let mut manifest = valid.clone();
        manifest[field] = value;
        let mut entries = valid_entries.clone();
        entries.insert(
            "manifest.json".into(),
            serde_json::to_vec(&manifest).expect("encode invalid manifest fixture"),
        );
        let archive = unique_target_path(&workspace, &format!("manifest-{label}"));
        write_authenticated_entry_map(&archive, passphrase, &entries);
        let target = workspace.root().join(format!("manifest-{label}-target"));
        let error = importer
            .import_age(&archive, passphrase, &target)
            .await
            .expect_err("invalid manifest identity must fail before staging");
        assert!(matches!(error, ImportError::InvalidManifest(_)));
        assert!(!target.exists());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn format_2_archive_is_accepted_and_normalised() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = write_seed_database(&workspace).await;
    let target = unique_target_path(&workspace, "format2");

    let exporter = BackupExporter::with_format(&database, 2);
    exporter
        .export_age(&target, "T129-format2-passphrase")
        .await
        .expect("export format 2 must succeed");

    let importer = BackupImporter::new(workspace.root().join("restore-staging"));
    let restored_database = importer
        .import_age(
            &target,
            "T129-format2-passphrase",
            workspace.root().join("restore-out"),
        )
        .await
        .expect("import format 2 must succeed");
    let restored = Store::open_local(StoreOpenOptions::new(
        &restored_database,
        restored_database
            .parent()
            .expect("restore parent")
            .join("plugins"),
    ))
    .await
    .expect("format 2 archive must materialize a current Store");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM data_sources WHERE name='backup-seed'")
            .fetch_one(restored.pool())
            .await
            .expect("format 2 restored seed count"),
        1
    );
}

#[tokio::test(flavor = "current_thread")]
async fn current_writer_uses_format_3_and_version_bounds_fail_closed() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = write_seed_database(&workspace).await;
    let current = unique_target_path(&workspace, "format-current");
    BackupExporter::new(&database)
        .export_age(&current, "T129-format-current-passphrase")
        .await
        .expect("current export");
    let importer = BackupImporter::new(workspace.root().join("format-staging"));
    assert_eq!(
        importer
            .inspect_manifest(&current, "T129-format-current-passphrase")
            .await
            .expect("inspect current manifest")
            .format_version,
        3,
        "new archives must be written at the current format"
    );

    for (format, expected) in [(0, "backup_too_old"), (4, "backup_newer_version")] {
        let archive = unique_target_path(&workspace, &format!("format-{format}"));
        BackupExporter::with_format(&database, format)
            .export_age(&archive, "T129-format-bound-passphrase")
            .await
            .expect("construct authenticated version-bound fixture");
        let error = importer
            .import_age(
                &archive,
                "T129-format-bound-passphrase",
                workspace.root().join(format!("format-{format}-out")),
            )
            .await
            .expect_err("unsupported version must fail before publishing a target");
        assert_eq!(error.to_string(), expected);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn portable_manifest_carries_current_format_and_complete_entity_inventory() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let database = write_seed_database(&workspace).await;
    let target = unique_target_path(&workspace, "manifest");

    let exporter = BackupExporter::new(&database);
    exporter
        .export_age(&target, "T129-manifest-passphrase")
        .await
        .expect("export");

    let importer = BackupImporter::new(workspace.root().join("restore-staging"));
    let manifest = importer
        .inspect_manifest(&target, "T129-manifest-passphrase")
        .await
        .expect("manifest inspect must succeed");
    assert_eq!(manifest.schema_version, 4);
    assert_eq!(manifest.format, "hivegui-backup");
    assert_eq!(manifest.format_version, 3);
    assert_eq!(manifest.entity_files.len(), 22);
    assert!(manifest.artifacts.is_empty());
    assert!(chrono::DateTime::parse_from_rfc3339(&manifest.exported_at).is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn open_store_backup_includes_committed_wal_and_reencrypts_for_the_target_device() {
    let source = TestWorkspace::new().expect("source workspace");
    let target_device = TestWorkspace::new().expect("target device workspace");
    let source_store = Store::open_local(StoreOpenOptions::new(
        source.database_path(),
        source.plugin_root(),
    ))
    .await
    .expect("open source Store");
    let password = b"T119-cross-device-password-canary";
    source_store
        .create(
            "committed-after-open",
            "127.0.0.1",
            3306,
            "backup-user",
            password,
        )
        .await
        .expect("commit source row while Store remains open");

    let archive = unique_target_path(&source, "open-store-wal");
    BackupExporter::new(source.database_path())
        .export_age(&archive, "T119-open-store-passphrase")
        .await
        .expect("export open Store through the safe snapshot boundary");
    let restored_database = BackupImporter::new(target_device.root().join("restore-staging"))
        .import_age(
            &archive,
            "T119-open-store-passphrase",
            target_device.data_home().join("hivegui"),
        )
        .await
        .expect("authenticated import on the target device");
    let restored_store = Store::open_local(StoreOpenOptions::new(
        &restored_database,
        target_device.plugin_root(),
    ))
    .await
    .expect("open restored Store with the target device key");
    let rows = restored_store.list().await.expect("list restored rows");
    assert_eq!(
        rows.len(),
        1,
        "the committed WAL frame must be in the backup"
    );
    assert_eq!(rows[0].name, "committed-after-open");
    assert_eq!(
        restored_store
            .decrypt_password(&rows[0].encrypted_password)
            .expect("target key decrypts re-encrypted secret"),
        password,
        "portable restore must re-encrypt sensitive fields with the target device key"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn portable_archive_carries_managed_wasm_but_rebuilds_derived_search_and_excludes_ledgers() {
    let source = TestWorkspace::new().expect("source workspace");
    let target_device = TestWorkspace::new().expect("target workspace");
    let source_store = Store::open_local(StoreOpenOptions::new(
        source.database_path(),
        source.plugin_root(),
    ))
    .await
    .expect("open source Store");
    let artifact_key = "portable-fixture/1.0.0/plugin.wasm";
    let artifact_path = source.plugin_root().join(artifact_key);
    fs::create_dir_all(artifact_path.parent().expect("artifact parent")).expect("mkdir artifact");
    let wasm = b"\0asmT119-managed-wasm-fixture";
    fs::write(&artifact_path, wasm).expect("write managed WASM fixture");
    let unowned_artifact = source.plugin_root().join("unowned/plugin.wasm");
    fs::create_dir_all(unowned_artifact.parent().expect("unowned parent"))
        .expect("mkdir unowned artifact");
    fs::write(&unowned_artifact, b"must-not-enter-portable-backup")
        .expect("write unowned artifact");
    let wasm_sha256 = hex::encode(Sha256::digest(wasm));
    sqlx::query(
        "INSERT INTO plugins (identifier,name,version,s3_key,sha256,size_bytes,runtime,created_at,updated_at) \
         VALUES ('portable-fixture','Portable Fixture','1.0.0',?,?,?,'wasm32','2026-08-25T00:00:00Z','2026-08-25T00:00:00Z')",
    )
    .bind(artifact_key)
    .bind(wasm_sha256)
    .bind(wasm.len() as i64)
    .execute(source_store.pool())
    .await
    .expect("seed Plugin row");
    sqlx::query(
        "INSERT INTO plugin_artifact_operations \
         (operation_id,kind,staging_name,state,created_at,updated_at) \
         VALUES ('00000000-0000-4000-8000-000000000119','create','t119-ledger','done','2026-08-25T00:00:00Z','2026-08-25T00:00:00Z')",
    )
    .execute(source_store.pool())
    .await
    .expect("seed operation ledger");
    sqlx::query(
        "INSERT INTO plugin_artifact_gc \
         (artifact_key,source_operation_id,last_attempt_at,reason,state) \
         VALUES ('obsolete/plugin.wasm','00000000-0000-4000-8000-000000000119',0,'test','pending')",
    )
    .execute(source_store.pool())
    .await
    .expect("seed GC ledger");
    sqlx::query(
        "INSERT INTO search_documents (entity_type,entity_key,field,normalized_text) \
         VALUES ('plugin','nonexistent-derived-row','name','must-not-survive-portable-restore')",
    )
    .execute(source_store.pool())
    .await
    .expect("seed deliberately stale derived search row");

    let archive = unique_target_path(&source, "portable-structure");
    BackupExporter::new(source.database_path())
        .export_age(&archive, "T119-portable-structure-passphrase")
        .await
        .expect("export portable archive");
    let restored_database = BackupImporter::new(target_device.root().join("restore-staging"))
        .import_age(
            &archive,
            "T119-portable-structure-passphrase",
            target_device.data_home().join("hivegui"),
        )
        .await
        .expect("restore portable archive");
    let restored_store = Store::open_local(StoreOpenOptions::new(
        &restored_database,
        target_device.plugin_root(),
    ))
    .await
    .expect("open restored Store");
    let internal_counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM plugin_artifact_operations), \
                (SELECT COUNT(*) FROM plugin_artifact_gc)",
    )
    .fetch_one(restored_store.pool())
    .await
    .expect("portable ledger counts");
    assert_eq!(
        internal_counts,
        (0, 0),
        "portable archives must exclude operation and GC ledgers"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM search_documents WHERE entity_key = 'nonexistent-derived-row'",
        )
        .fetch_one(restored_store.pool())
        .await
        .expect("derived row count"),
        0,
        "portable restore must rebuild derived search from entities"
    );
    assert_eq!(
        fs::read(target_device.plugin_root().join(artifact_key)).expect("restored managed WASM"),
        wasm,
        "every managed Plugin artifact must round-trip with its entity"
    );
    assert!(
        !target_device
            .plugin_root()
            .join("unowned/plugin.wasm")
            .exists(),
        "portable archive must be driven by active Plugin ownership rows, not directory traversal"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn every_portable_entity_relation_sensitive_field_and_managed_wasm_round_trips_exactly() {
    let source = TestWorkspace::new().expect("source workspace");
    let target = TestWorkspace::new().expect("target workspace");
    let source_store = Store::open_local(StoreOpenOptions::new(
        source.database_path(),
        source.plugin_root(),
    ))
    .await
    .expect("open source Store");
    let expected_secrets = seed_complete_portable_fixture(&source, &source_store).await;
    let source_device_key = fs::read(
        source
            .database_path()
            .parent()
            .expect("source database parent")
            .join("encryption.key"),
    )
    .expect("source device key");

    let passphrase = "T129-complete-entity-roundtrip-passphrase";
    let source_archive = unique_target_path(&source, "complete-entities-source");
    BackupExporter::new(source.database_path())
        .export_age(&source_archive, passphrase)
        .await
        .expect("export complete portable fixture");
    let source_entries = read_authenticated_tar_entries(&source_archive, passphrase);
    let source_manifest: serde_json::Value = serde_json::from_slice(
        source_entries
            .get("manifest.json")
            .expect("source portable manifest"),
    )
    .expect("parse source portable manifest");
    let descriptors = source_manifest["entity_files"]
        .as_array()
        .expect("complete entity descriptor array");
    assert_eq!(descriptors.len(), 22);
    for descriptor in descriptors {
        let name = descriptor["name"].as_str().expect("entity name");
        let count = descriptor["count"].as_u64().expect("entity row count");
        assert!(
            count > 0,
            "portable entity {name} must be physically nonempty"
        );
    }
    assert_eq!(source_manifest["artifacts"].as_array().unwrap().len(), 2);

    let restored_database = BackupImporter::new(target.root().join("complete-import-staging"))
        .import_age(
            &source_archive,
            passphrase,
            target.data_home().join("hivegui"),
        )
        .await
        .expect("import all entity and relation rows");
    let target_device_key = fs::read(
        restored_database
            .parent()
            .expect("restored database parent")
            .join("encryption.key"),
    )
    .expect("target device key");
    assert_ne!(
        source_device_key, target_device_key,
        "cross-device restore must use a distinct target device key"
    );

    let target_archive = unique_target_path(&target, "complete-entities-target");
    BackupExporter::new(&restored_database)
        .export_age(&target_archive, passphrase)
        .await
        .expect("re-export restored portable fixture");
    let target_entries = read_authenticated_tar_entries(&target_archive, passphrase);
    let portable_paths = source_entries
        .keys()
        .filter(|path| path.starts_with("entities/") || path.starts_with("plugins/"))
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        portable_paths,
        target_entries
            .keys()
            .filter(|path| path.starts_with("entities/") || path.starts_with("plugins/"))
            .cloned()
            .collect(),
        "the restored archive must carry the exact same entity and artifact inventory"
    );
    for path in portable_paths {
        assert_eq!(
            source_entries.get(&path),
            target_entries.get(&path),
            "every portable field/relation/artifact byte must round-trip at {path}"
        );
    }

    let target_options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&restored_database)
        .create_if_missing(false)
        .foreign_keys(true);
    let target_pool = sqlx::sqlite::SqlitePoolOptions::new()
        .min_connections(1)
        .max_connections(1)
        .connect_with(target_options)
        .await
        .expect("open restored database for physical verification");
    let target_crypto = Crypto::new(
        &target_device_key
            .as_slice()
            .try_into()
            .expect("target device key is exactly 32 bytes"),
    );
    for (label, query) in [
        (
            "datasource",
            "SELECT encrypted_password FROM data_sources WHERE name='portable-complete-datasource'",
        ),
        (
            "provider",
            "SELECT token_encrypted FROM llm_providers WHERE name='portable-provider'",
        ),
        (
            "session",
            "SELECT title_encrypted FROM chat_sessions WHERE id='00000000-0000-4000-8000-000000001219'",
        ),
        (
            "message",
            "SELECT content_encrypted FROM chat_messages WHERE session_id='00000000-0000-4000-8000-000000001219'",
        ),
        (
            "tool_calls",
            "SELECT tool_calls_encrypted FROM chat_messages WHERE session_id='00000000-0000-4000-8000-000000001219'",
        ),
        (
            "execution",
            "SELECT state_encrypted FROM agent_executions WHERE execution_id='00000000-0000-4000-8000-000000001220'",
        ),
    ] {
        let source_ciphertext: Vec<u8> = sqlx::query_scalar(query)
            .fetch_one(source_store.pool())
            .await
            .unwrap_or_else(|_| panic!("read source ciphertext for {label}"));
        let target_ciphertext: Vec<u8> = sqlx::query_scalar(query)
            .fetch_one(&target_pool)
            .await
            .unwrap_or_else(|_| panic!("read target ciphertext for {label}"));
        assert_ne!(
            source_ciphertext, target_ciphertext,
            "{label} must be freshly encrypted under the target device key"
        );
        assert_eq!(
            target_crypto
                .decrypt(&target_ciphertext)
                .unwrap_or_else(|_| panic!("decrypt restored {label}")),
            expected_secrets[label],
            "{label} plaintext must round-trip exactly"
        );
    }
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&target_pool)
            .await
            .expect("target foreign_key_check")
            .is_empty(),
        "every restored relationship must satisfy the target v4 foreign keys"
    );
    let search_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM search_documents WHERE entity_type IN ('agent','function','plugin','workflow')",
    )
    .fetch_one(&target_pool)
    .await
    .expect("count rebuilt derived search rows");
    assert!(
        search_rows > 0,
        "derived search rows must be rebuilt from entities"
    );
    let internal_ledgers: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM plugin_artifact_operations), \
                (SELECT COUNT(*) FROM plugin_artifact_gc)",
    )
    .fetch_one(&target_pool)
    .await
    .expect("read restored internal ledgers");
    assert_eq!(internal_ledgers, (0, 0));
    target_pool.close().await;
}

#[tokio::test(flavor = "current_thread")]
async fn portable_archive_is_manifested_entity_json_without_a_sqlite_or_local_key_payload() {
    let source = TestWorkspace::new().expect("source workspace");
    let database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "portable-entity-layout");
    BackupExporter::new(database)
        .export_age(&archive, "T129-portable-entity-layout-passphrase")
        .await
        .expect("export current portable archive");

    let entries =
        read_authenticated_tar_entries(&archive, "T129-portable-entity-layout-passphrase");
    let manifest: serde_json::Value = serde_json::from_slice(
        entries
            .get("manifest.json")
            .expect("portable manifest entry"),
    )
    .expect("parse portable manifest");
    assert_eq!(
        manifest
            .as_object()
            .expect("manifest object")
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        [
            "artifacts",
            "entity_files",
            "exported_at",
            "format",
            "format_version",
            "schema_version",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        "portable manifest has one exact versioned schema"
    );
    assert_eq!(manifest["format"], "hivegui-backup");
    assert_eq!(manifest["format_version"], 3);
    assert_eq!(manifest["schema_version"], 4);
    assert!(
        manifest["exported_at"]
            .as_str()
            .is_some_and(|value| chrono::DateTime::parse_from_rfc3339(value).is_ok()),
        "exported_at is RFC3339"
    );

    let expected_entities = [
        "agent_capabilities",
        "agent_executions",
        "agent_skills",
        "agent_tools",
        "agents",
        "capabilities",
        "categories",
        "chat_messages",
        "chat_sessions",
        "data_sources",
        "functions",
        "global_configs",
        "llm_presets",
        "llm_providers",
        "models",
        "plugins",
        "skills",
        "tags",
        "tools",
        "workflow_edges",
        "workflow_nodes",
        "workflows",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<BTreeSet<_>>();
    let entity_files = manifest["entity_files"]
        .as_array()
        .expect("entity file inventory");
    assert_eq!(
        entity_files
            .iter()
            .map(|entry| entry["name"].as_str().expect("entity name").to_string())
            .collect::<BTreeSet<_>>(),
        expected_entities,
        "empty entity types remain explicitly represented"
    );
    for entity in entity_files {
        let path = entity["path"].as_str().expect("entity path");
        let bytes = entries.get(path).expect("manifested entity file exists");
        assert_eq!(
            hex::encode(Sha256::digest(bytes)),
            entity["sha256"].as_str().expect("entity digest")
        );
        let body: serde_json::Value = serde_json::from_slice(bytes).expect("entity JSON");
        assert_eq!(body["schema_version"], 4);
        assert_eq!(body["entity"], entity["name"]);
        assert_eq!(
            body["rows"].as_array().expect("entity rows").len() as u64,
            entity["count"].as_u64().expect("entity row count")
        );
    }
    assert!(!entries.contains_key("datasources.db"));
    assert!(!entries.contains_key("portable-field-key.bin"));
    assert!(entries.keys().all(|path| {
        path == "manifest.json" || path.starts_with("entities/") || path.starts_with("plugins/")
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn legacy_entity_archives_upgrade_integer_kinds_and_dotted_builtins_once() {
    let source = TestWorkspace::new().expect("source workspace");
    let database = write_seed_database(&source).await;
    let current = unique_target_path(&source, "legacy-upgrade-source");
    let passphrase = "T129-legacy-entity-upgrade-passphrase";
    BackupExporter::new(database)
        .export_age(&current, passphrase)
        .await
        .expect("export current entity archive");
    let legacy = unique_target_path(&source, "legacy-upgrade-fixture");
    rewrite_portable_entity_archive(&current, &legacy, passphrase, 2, |name, body| {
        if name != "functions" {
            return;
        }
        let columns = body["columns"].as_array().expect("Function columns");
        let identifier = columns
            .iter()
            .position(|column| column == "identifier")
            .expect("identifier column");
        let kind = columns
            .iter()
            .position(|column| column == "kind")
            .expect("kind column");
        for row in body["rows"].as_array_mut().expect("Function rows") {
            let row = row.as_array_mut().expect("Function row");
            if row[kind]["value"] == "builtin" {
                row[kind] = serde_json::json!({"type":"integer","value":1});
                let legacy_identifier = match row[identifier]["value"].as_str().expect("Builtin") {
                    "format_template" => "format.template",
                    "json_parse" => "json.parse",
                    "json_stringify" => "json.stringify",
                    "text_regex_match" => "text.regex_match",
                    other => panic!("unexpected Builtin {other}"),
                };
                row[identifier] = serde_json::json!({"type":"text","value":legacy_identifier});
            }
        }
    });

    let target = TestWorkspace::new().expect("target workspace");
    let restored = BackupImporter::new(target.root().join("legacy-upgrade-staging"))
        .import_age(&legacy, passphrase, target.data_home().join("hivegui"))
        .await
        .expect("trusted format 2 upgrade");
    let store = Store::open_local(StoreOpenOptions::new(&restored, target.plugin_root()))
        .await
        .expect("open upgraded Store");
    let builtins = sqlx::query_as::<_, (String, String)>(
        "SELECT identifier, kind FROM functions WHERE kind='builtin' ORDER BY identifier",
    )
    .fetch_all(store.pool())
    .await
    .expect("read upgraded Builtins");
    assert_eq!(
        builtins,
        vec![
            ("format_template".into(), "builtin".into()),
            ("json_parse".into(), "builtin".into()),
            ("json_stringify".into(), "builtin".into()),
            ("text_regex_match".into(), "builtin".into()),
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn current_integer_kind_and_legacy_builtin_collision_fail_before_target_creation() {
    let source = TestWorkspace::new().expect("source workspace");
    let database = write_seed_database(&source).await;
    let current = unique_target_path(&source, "format-validation-source");
    let passphrase = "T129-format-validation-passphrase";
    BackupExporter::new(database)
        .export_age(&current, passphrase)
        .await
        .expect("export current entity archive");

    for (label, format_version, collision) in [
        ("current-integer-kind", 3, false),
        ("legacy-builtin-collision", 2, true),
    ] {
        let invalid = unique_target_path(&source, label);
        rewrite_portable_entity_archive(
            &current,
            &invalid,
            passphrase,
            format_version,
            |name, body| {
                if name != "functions" {
                    return;
                }
                let columns = body["columns"].as_array().expect("Function columns");
                let id = columns
                    .iter()
                    .position(|column| column == "id")
                    .expect("id column");
                let identifier = columns
                    .iter()
                    .position(|column| column == "identifier")
                    .expect("identifier column");
                let kind = columns
                    .iter()
                    .position(|column| column == "kind")
                    .expect("kind column");
                let rows = body["rows"].as_array_mut().expect("Function rows");
                if collision {
                    let mut duplicate = rows[0].clone();
                    let duplicate = duplicate.as_array_mut().expect("duplicate Function row");
                    duplicate[id] = serde_json::json!({"type":"integer","value":999999});
                    duplicate[identifier] =
                        serde_json::json!({"type":"text","value":"format.template"});
                    duplicate[kind] = serde_json::json!({"type":"integer","value":1});
                    rows.push(serde_json::Value::Array(duplicate.clone()));
                } else {
                    rows[0].as_array_mut().expect("Function row")[kind] =
                        serde_json::json!({"type":"integer","value":1});
                }
            },
        );
        let target = source.root().join(format!("{label}-target"));
        let error = BackupImporter::new(source.root().join(format!("{label}-staging")))
            .import_age(&invalid, passphrase, &target)
            .await
            .expect_err("invalid or colliding portable values must fail closed");
        assert!(matches!(error, ImportError::InvalidManifest(_)));
        assert!(!target.exists(), "validation must precede target creation");
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn portable_export_rejects_an_intermediate_symlink_before_reading_outside_plugin_root() {
    use std::os::unix::fs::symlink;

    let source = TestWorkspace::new().expect("source workspace");
    let source_store = Store::open_local(StoreOpenOptions::new(
        source.database_path(),
        source.plugin_root(),
    ))
    .await
    .expect("open source Store");
    let outside = source.root().join("outside-plugin-root");
    fs::create_dir(&outside).expect("create outside directory");
    let outside_artifact = outside.join("plugin.wasm");
    let bytes = b"\0asmT129-intermediate-symlink-canary";
    fs::write(&outside_artifact, bytes).expect("write outside canary");
    symlink(&outside, source.plugin_root().join("escaped"))
        .expect("plant intermediate directory symlink");
    let artifact_key = "escaped/plugin.wasm";
    sqlx::query(
        "INSERT INTO plugins (identifier,name,version,s3_key,sha256,size_bytes,runtime,created_at,updated_at) \
         VALUES ('escaped-plugin','Escaped Plugin','1.0.0',?,?,?,'wasm32','2026-08-25T00:00:00Z','2026-08-25T00:00:00Z')",
    )
    .bind(artifact_key)
    .bind(hex::encode(Sha256::digest(bytes)))
    .bind(i64::try_from(bytes.len()).expect("fixture size"))
    .execute(source_store.pool())
    .await
    .expect("seed escaped Plugin row");

    let archive = unique_target_path(&source, "intermediate-symlink");
    let error = BackupExporter::new(source.database_path())
        .export_age(&archive, "T129-intermediate-symlink-passphrase")
        .await
        .expect_err("an intermediate symlink must fail before outside bytes are read");
    assert!(matches!(error, ExportError::UnsafeSource(_)));
    assert!(!archive.exists(), "unsafe export must publish no archive");
    assert_eq!(
        fs::read(outside_artifact).expect("outside canary remains readable"),
        bytes,
        "outside bytes must remain untouched"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn portable_export_rejects_a_symlinked_managed_plugin_root() {
    use std::os::unix::fs::symlink;

    let source = TestWorkspace::new().expect("source workspace");
    let source_store = Store::open_local(StoreOpenOptions::new(
        source.database_path(),
        source.plugin_root(),
    ))
    .await
    .expect("open source Store");
    let outside = source.root().join("outside-managed-root");
    fs::create_dir_all(outside.join("fixture")).expect("create outside Plugin tree");
    let bytes = b"\0asmT129-plugin-root-symlink-canary";
    fs::write(outside.join("fixture/plugin.wasm"), bytes).expect("write outside artifact");
    sqlx::query(
        "INSERT INTO plugins (identifier,name,version,s3_key,sha256,size_bytes,runtime,created_at,updated_at) \
         VALUES ('root-link-plugin','Root Link Plugin','1.0.0','fixture/plugin.wasm',?,?,\
                 'wasm32','2026-08-25T00:00:00Z','2026-08-25T00:00:00Z')",
    )
    .bind(hex::encode(Sha256::digest(bytes)))
    .bind(i64::try_from(bytes.len()).expect("fixture size"))
    .execute(source_store.pool())
    .await
    .expect("seed Plugin row");
    fs::remove_dir(source.plugin_root()).expect("remove empty managed root");
    symlink(&outside, source.plugin_root()).expect("replace managed root with symlink");

    let archive = unique_target_path(&source, "plugin-root-symlink");
    let error = BackupExporter::new(source.database_path())
        .export_age(&archive, "T129-plugin-root-symlink-passphrase")
        .await
        .expect_err("managed Plugin root symlink must be rejected");
    assert!(matches!(error, ExportError::UnsafeSource(_)));
    assert!(!archive.exists());
    assert_eq!(
        fs::read(outside.join("fixture/plugin.wasm")).expect("outside artifact remains"),
        bytes
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn export_rejects_a_symlinked_target_parent_without_writing_outside() {
    use std::os::unix::fs::symlink;

    let workspace = TestWorkspace::new().expect("workspace");
    let database = write_seed_database(&workspace).await;
    let outside = workspace.root().join("outside-target");
    fs::create_dir(&outside).expect("create outside target");
    let canary = outside.join("must-remain-only-entry");
    fs::write(&canary, b"T129-target-parent-canary").expect("write outside canary");
    let target_alias = workspace.root().join("target-alias");
    symlink(&outside, &target_alias).expect("plant target parent symlink");
    let target = target_alias.join("backup.age");

    let error = BackupExporter::new(database)
        .export_age(&target, "T129-target-parent-passphrase")
        .await
        .expect_err("target parent symlink must fail before staging creation");
    assert!(matches!(error, ExportError::UnsafeSource(_)));
    assert_eq!(
        fs::read_dir(&outside)
            .expect("outside directory")
            .map(|entry| entry.expect("outside entry").file_name())
            .collect::<Vec<_>>(),
        vec![canary.file_name().expect("canary name").to_os_string()],
        "unsafe target resolution must perform zero writes outside the selected parent"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn startup_replay_rejects_a_symlinked_restore_registry_without_external_io() {
    use std::os::unix::fs::symlink;

    let target = TestWorkspace::new().expect("target workspace");
    let outside = TestWorkspace::new().expect("outside workspace");
    let canary = outside.root().join("must-not-touch");
    fs::write(&canary, b"T130-registry-symlink-canary").expect("write outside canary");
    let empty_external_registry = outside.root().join("empty-registry");
    fs::create_dir(&empty_external_registry).expect("create empty external registry");
    symlink(
        &empty_external_registry,
        target.root().join(".hivegui-db-staging-v1"),
    )
    .expect("plant registry symlink");

    let error = match RestoreCoordinator::new(target.root()) {
        Ok(coordinator) => coordinator
            .recover_startup()
            .await
            .expect_err("startup replay must reject a symlinked registry root"),
        Err(error) => error,
    };
    assert!(matches!(error, ImportError::UnsafeArchiveEntry(_)));
    assert_eq!(
        fs::read(canary).expect("outside canary remains"),
        b"T130-registry-symlink-canary",
        "startup replay must perform no external I/O"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn startup_replay_rejects_a_symlinked_live_instance_before_manifest_io() {
    use std::os::unix::fs::symlink;

    let target = TestWorkspace::new().expect("target workspace");
    let registry = target.root().join(".hivegui-db-staging-v1");
    fs::create_dir(&registry).expect("create registry");
    let outside = target.root().join("outside-live-instance");
    fs::create_dir(&outside).expect("create outside live directory");
    let canary = outside.join("must-not-touch");
    fs::write(&canary, b"T130-live-symlink-canary").expect("write outside canary");
    symlink(
        &outside,
        registry.join("restore-00000000-0000-4000-8000-000000000130"),
    )
    .expect("plant live instance symlink");

    let error = RestoreCoordinator::new(target.root())
        .expect("open coordinator")
        .recover_startup()
        .await
        .expect_err("live instance symlink must fail before manifest lookup");
    assert!(matches!(error, ImportError::UnsafeArchiveEntry(_)));
    assert_eq!(
        fs::read(canary).expect("outside canary remains"),
        b"T130-live-symlink-canary"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn startup_replay_keeps_using_the_open_root_after_ambient_path_replacement() {
    use std::os::unix::fs::symlink;

    let workspace = TestWorkspace::new().expect("workspace");
    let coordinator_root = workspace.root().join("coordinator-root");
    fs::create_dir(&coordinator_root).expect("create coordinator root");
    let coordinator =
        RestoreCoordinator::new(&coordinator_root).expect("open stable coordinator root");

    let retained_root = workspace.root().join("retained-root");
    fs::rename(&coordinator_root, &retained_root)
        .expect("move opened root without replacing inode");
    let outside = workspace.root().join("outside-root");
    let outside_registry = outside.join(".hivegui-db-staging-v1");
    fs::create_dir_all(&outside_registry).expect("create outside registry");
    let canary = outside_registry.join("must-not-read-or-delete");
    fs::write(&canary, b"T130-open-root-canary").expect("write outside canary");
    symlink(&outside, &coordinator_root).expect("replace ambient root path with symlink");

    let recovery = coordinator
        .recover_startup()
        .await
        .expect("replay must remain rooted at the descriptor opened by new");
    assert!(recovery.store_may_open());
    assert_eq!(
        fs::read(&canary).expect("outside canary remains"),
        b"T130-open-root-canary",
        "root replacement must cause zero I/O beneath the ambient symlink target"
    );
    assert!(
        !retained_root.join(".hivegui-db-staging-v1").exists(),
        "an empty retained root needs no synthetic registry"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn import_rejects_symlinked_and_hardlinked_archive_descriptors() {
    use std::os::unix::fs::symlink;

    let source = TestWorkspace::new().expect("source workspace");
    let database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "archive-descriptor");
    BackupExporter::new(database)
        .export_age(&archive, "T129-archive-descriptor-passphrase")
        .await
        .expect("create valid source archive");
    let symlink_alias = source.root().join("archive-symlink.age");
    symlink(&archive, &symlink_alias).expect("create archive symlink");
    let importer = BackupImporter::new(source.root().join("archive-input-staging"));
    let symlink_error = importer
        .inspect_manifest(&symlink_alias, "T129-archive-descriptor-passphrase")
        .await
        .expect_err("archive symlink must be rejected before decryption");
    assert!(matches!(symlink_error, ImportError::UnsafeArchiveEntry(_)));

    fs::remove_file(&symlink_alias).expect("remove symlink alias");
    let hardlink_alias = source.root().join("archive-hardlink.age");
    fs::hard_link(&archive, &hardlink_alias).expect("create archive hardlink");
    let hardlink_error = importer
        .inspect_manifest(&hardlink_alias, "T129-archive-descriptor-passphrase")
        .await
        .expect_err("archive hardlink must be rejected before decryption");
    assert!(matches!(hardlink_error, ImportError::UnsafeArchiveEntry(_)));
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn import_rejects_an_intermediate_symlink_in_the_archive_source_path() {
    use std::os::unix::fs::symlink;

    let workspace = TestWorkspace::new().expect("workspace");
    let database = write_seed_database(&workspace).await;
    let outside = workspace.root().join("outside-archive-source");
    fs::create_dir(&outside).expect("create outside archive directory");
    let archive = outside.join("backup.age");
    BackupExporter::new(database)
        .export_age(&archive, "T129-archive-parent-passphrase")
        .await
        .expect("create authenticated outside archive fixture");
    let alias = workspace.root().join("archive-source-alias");
    symlink(&outside, &alias).expect("plant archive parent symlink");

    let error = BackupImporter::new(workspace.root().join("archive-parent-staging"))
        .inspect_manifest(&alias.join("backup.age"), "T129-archive-parent-passphrase")
        .await
        .expect_err("archive intermediate symlink must fail before decryption");
    assert!(matches!(error, ImportError::UnsafeArchiveEntry(_)));
    assert!(archive.exists(), "outside archive must remain untouched");
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn import_rejects_an_intermediate_symlink_in_the_final_target_parent() {
    use std::os::unix::fs::symlink;

    let workspace = TestWorkspace::new().expect("workspace");
    let database = write_seed_database(&workspace).await;
    let archive = unique_target_path(&workspace, "import-target-parent");
    BackupExporter::new(database)
        .export_age(&archive, "T129-import-target-parent-passphrase")
        .await
        .expect("create authenticated archive");

    let outside = workspace.root().join("outside-import-target");
    fs::create_dir_all(outside.join("nested")).expect("create outside nested target");
    let canary = outside.join("must-remain-only-entry");
    fs::write(&canary, b"T129-import-target-canary").expect("write outside canary");
    let alias = workspace.root().join("import-target-alias");
    symlink(&outside, &alias).expect("plant intermediate target symlink");
    let importer = BackupImporter::new(workspace.root().join("safe-import-staging"));
    let error = importer
        .import_age(
            &archive,
            "T129-import-target-parent-passphrase",
            alias.join("nested/restore"),
        )
        .await
        .expect_err("intermediate target symlink must fail before target creation");
    assert!(matches!(error, ImportError::UnsafeArchiveEntry(_)));
    assert_eq!(
        fs::read_dir(&outside)
            .expect("outside directory")
            .map(|entry| entry.expect("outside entry").file_name())
            .collect::<std::collections::BTreeSet<_>>(),
        [
            canary.file_name().expect("canary name").to_os_string(),
            std::ffi::OsString::from("nested"),
        ]
        .into_iter()
        .collect(),
        "unsafe import target must perform zero writes outside the controlled root"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn confirmation_freezes_after_the_last_legal_write_and_exports_that_write() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_store = Store::open_local(StoreOpenOptions::new(
        source.database_path(),
        source.plugin_root(),
    ))
    .await
    .expect("open source Store");
    source_store
        .create("before-preview", "127.0.0.1", 3306, "user", b"first")
        .await
        .expect("seed before preview");
    let archive = unique_target_path(&source, "confirmation-freeze");
    let coordinator = BackupCoordinator::from_store(source_store.clone())
        .expect("single production backup coordinator");
    let preview = coordinator
        .preview_export(&archive)
        .await
        .expect("preview backup");
    assert_eq!(preview.entity_count("data_sources"), 1);

    source_store
        .create("last-legal-write", "127.0.0.1", 3307, "user", b"last")
        .await
        .expect("the write between preview and confirmation is legal");
    let confirmed = coordinator
        .confirm_export(preview, "T119-confirmation-passphrase")
        .await
        .expect("confirmation freezes and exports");
    let blocked = source_store
        .create("after-confirmation", "127.0.0.1", 3308, "user", b"blocked")
        .await
        .expect_err("writes stay frozen through checkpoint/close/export verification");
    assert!(blocked.to_string().contains("write_gate_closed"));
    assert_eq!(confirmed.entity_count("data_sources"), 2);
    assert!(confirmed.includes_entity("data_sources", "last-legal-write"));
    assert!(confirmed.current_checkpoint_complete());
    assert!(confirmed.sidecars_converged());
}

#[tokio::test(flavor = "current_thread")]
async fn every_backup_confirmation_crash_point_executes_the_real_frozen_snapshot_boundary() {
    for point in BACKUP_CONFIRMATION_CRASH_POINTS {
        let source = TestWorkspace::new().expect("isolated confirmation workspace");
        let source_store = Store::open_local(StoreOpenOptions::new(
            source.database_path(),
            source.plugin_root(),
        ))
        .await
        .expect("open confirmation Store");
        source_store
            .create("before-preview", "127.0.0.1", 3306, "user", b"first")
            .await
            .expect("seed before preview");
        let archive = unique_target_path(&source, point.as_str());
        let coordinator = BackupCoordinator::from_store(source_store.clone())
            .expect("production confirmation coordinator");
        let preview = coordinator
            .preview_export(&archive)
            .await
            .expect("preview before last legal write");
        source_store
            .create("last-legal-write", "127.0.0.1", 3307, "user", b"last")
            .await
            .expect("persist the last legal write");

        let interrupted = coordinator
            .confirm_with_crash(preview, "T130-confirmation-crash-passphrase", point)
            .await
            .expect_err("the selected real confirmation boundary must interrupt");
        assert_eq!(interrupted.crash_point(), point.as_str());
        assert!(
            interrupted.reached_requested_boundary(),
            "confirmation must reach the requested real boundary: {interrupted}"
        );
        let blocked = source_store
            .create("after-confirmation", "127.0.0.1", 3308, "user", b"blocked")
            .await
            .expect_err("the write gate remains closed after interruption");
        assert!(blocked.to_string().contains("write_gate_closed"));

        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(source.database_path())
            .create_if_missing(false)
            .foreign_keys(true);
        let read_pool = sqlx::sqlite::SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .connect_with(options)
            .await
            .expect("open interrupted current read-only fixture");
        let names = sqlx::query_scalar::<_, String>("SELECT name FROM data_sources ORDER BY name")
            .fetch_all(&read_pool)
            .await
            .expect("read current after confirmation interruption");
        read_pool.close().await;
        assert_eq!(names, vec!["before-preview", "last-legal-write"]);

        if point.as_str() == "safe_snapshot_publish" {
            let manifest = BackupImporter::new(source.root().join("crash-inspect"))
                .inspect_manifest(&archive, "T130-confirmation-crash-passphrase")
                .await
                .expect("durably published final archive remains authenticated");
            assert_eq!(manifest.schema_version, 4);
        } else {
            assert!(
                !archive.exists(),
                "pre-publish interruption must not expose a final archive"
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn sidecar_cleanup_journal_replays_all_five_durable_states_before_store_open() {
    for (state, location) in [
        ("prepared", "canonical"),
        ("prepared", "quarantine"),
        ("quarantined", "quarantine"),
        ("quarantined", "absent"),
        ("done", "absent"),
    ] {
        let workspace = TestWorkspace::new().expect("isolated cleanup replay workspace");
        let database = write_restore_current_database(workspace.root()).await;
        let database_sha256 = sha256_path(&database);
        let canonical_name = "datasources.db-wal";
        let canonical = workspace.root().join(canonical_name);
        let bytes = format!("safe-residual-{state}-{location}").into_bytes();
        fs::write(&canonical, &bytes).expect("seed proven-safe residual sidecar");
        let identity = sidecar_file_identity(&canonical);
        let cleanup_operation_id = Uuid::new_v4();
        let (journal, quarantine) = write_sidecar_cleanup_journal(
            workspace.root(),
            &database,
            SidecarCleanupJournalFixture {
                artifact: "wal",
                canonical_name,
                sidecar_identity: &identity,
                sidecar_size: bytes.len() as u64,
                sidecar_sha256: &hex::encode(Sha256::digest(&bytes)),
                cleanup_operation_id,
                state,
            },
        );
        match location {
            "canonical" => {}
            "quarantine" => fs::rename(&canonical, &quarantine)
                .expect("simulate durable canonical-to-quarantine rename"),
            "absent" => fs::remove_file(&canonical)
                .expect("simulate quarantine unlink before state durability"),
            _ => unreachable!("fixed cleanup fixture location"),
        }

        let replay = RestoreCoordinator::new(workspace.root())
            .expect("startup coordinator")
            .recover_startup()
            .await
            .expect("valid cleanup journal must converge before Store open");
        assert!(
            replay.store_may_open(),
            "state={state}, location={location}"
        );
        assert!(!journal.exists(), "journal must retire: {state}/{location}");
        assert!(
            !canonical.exists(),
            "canonical safe residual must be gone: {state}/{location}"
        );
        assert!(
            !quarantine.exists(),
            "quarantine safe residual must be gone: {state}/{location}"
        );
        assert_eq!(
            sha256_path(&database),
            database_sha256,
            "cleanup replay must not switch or rewrite the database"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn sidecar_cleanup_ambiguity_preserves_every_byte_and_blocks_store_open() {
    let workspace = TestWorkspace::new().expect("cleanup ambiguity workspace");
    let database = write_restore_current_database(workspace.root()).await;
    let canonical_name = "datasources.db-wal";
    let canonical = workspace.root().join(canonical_name);
    let canonical_bytes = b"owned-safe-residual";
    fs::write(&canonical, canonical_bytes).expect("seed canonical residual");
    let identity = sidecar_file_identity(&canonical);
    let (journal, quarantine) = write_sidecar_cleanup_journal(
        workspace.root(),
        &database,
        SidecarCleanupJournalFixture {
            artifact: "wal",
            canonical_name,
            sidecar_identity: &identity,
            sidecar_size: canonical_bytes.len() as u64,
            sidecar_sha256: &hex::encode(Sha256::digest(canonical_bytes)),
            cleanup_operation_id: Uuid::new_v4(),
            state: "prepared",
        },
    );
    let quarantine_bytes = b"concurrent-quarantine-identity";
    fs::write(&quarantine, quarantine_bytes).expect("plant ambiguous quarantine");

    let error = RestoreCoordinator::new(workspace.root())
        .expect("startup coordinator")
        .recover_startup()
        .await
        .expect_err("dual canonical/quarantine ownership must fail closed");
    assert_eq!(
        error.to_string(),
        "storage_recovery_blocked { reason: sidecar_unknown_owner, artifact: wal }"
    );
    assert_eq!(
        fs::read(&canonical).expect("canonical preserved"),
        canonical_bytes
    );
    assert_eq!(
        fs::read(&quarantine).expect("quarantine preserved"),
        quarantine_bytes
    );
    assert!(journal.exists(), "ambiguous ownership evidence is retained");
}

#[tokio::test(flavor = "current_thread")]
async fn sidecar_cleanup_selects_reason_priority_before_artifact_priority() {
    let workspace = TestWorkspace::new().expect("cleanup priority workspace");
    let database = write_restore_current_database(workspace.root()).await;

    let wal = workspace.root().join("datasources.db-wal");
    fs::write(&wal, b"retired-wal").expect("seed WAL evidence");
    let wal_identity = sidecar_file_identity(&wal);
    let (wal_journal, wal_quarantine) = write_sidecar_cleanup_journal(
        workspace.root(),
        &database,
        SidecarCleanupJournalFixture {
            artifact: "wal",
            canonical_name: "datasources.db-wal",
            sidecar_identity: &wal_identity,
            sidecar_size: 11,
            sidecar_sha256: &hex::encode(Sha256::digest(b"retired-wal")),
            cleanup_operation_id: Uuid::new_v4(),
            state: "done",
        },
    );
    fs::remove_file(&wal).expect("simulate completed WAL unlink");
    fs::write(&wal_quarantine, b"reappeared-quarantine")
        .expect("plant lower-priority WAL unknown-owner state");

    let rollback = workspace.root().join("datasources.db-journal");
    fs::write(&rollback, b"owned-rollback").expect("seed rollback evidence");
    let rollback_identity = sidecar_file_identity(&rollback);
    let (rollback_journal, _) = write_sidecar_cleanup_journal(
        workspace.root(),
        &database,
        SidecarCleanupJournalFixture {
            artifact: "rollback_journal",
            canonical_name: "datasources.db-journal",
            sidecar_identity: &rollback_identity,
            sidecar_size: 14,
            sidecar_sha256: &hex::encode(Sha256::digest(b"owned-rollback")),
            cleanup_operation_id: Uuid::new_v4(),
            state: "prepared",
        },
    );
    fs::remove_file(&rollback).expect("replace rollback identity");
    fs::write(&rollback, b"new-rollback-identity")
        .expect("plant higher-priority reappeared rollback");

    let error = RestoreCoordinator::new(workspace.root())
        .expect("startup coordinator")
        .recover_startup()
        .await
        .expect_err("all candidates must be classified before choosing one error");
    assert_eq!(
        error.to_string(),
        "storage_recovery_blocked { reason: sidecar_reappeared, artifact: rollback_journal }"
    );
    assert!(wal_journal.exists());
    assert!(wal_quarantine.exists());
    assert!(rollback_journal.exists());
    assert_eq!(
        fs::read(&rollback).expect("rollback preserved"),
        b"new-rollback-identity"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn orphan_or_misnamed_sidecar_cleanup_control_state_blocks_without_deletion() {
    for (name, bytes) in [
        (
            format!(".hivegui-sidecar-quarantine-v1-{}-wal", Uuid::new_v4()),
            b"orphan-quarantine".as_slice(),
        ),
        (
            format!(".hivegui-sidecar-cleanup-v1-{}-wal.json", "0".repeat(64)),
            b"{}".as_slice(),
        ),
    ] {
        let workspace = TestWorkspace::new().expect("isolated unknown control workspace");
        write_restore_current_database(workspace.root()).await;
        let unknown = workspace.root().join(name);
        fs::write(&unknown, bytes).expect("seed unknown cleanup control state");

        let error = RestoreCoordinator::new(workspace.root())
            .expect("startup coordinator")
            .recover_startup()
            .await
            .expect_err("unowned cleanup control state must keep Store closed");
        assert_eq!(
            error.to_string(),
            "storage_recovery_blocked { reason: sidecar_unknown_owner, artifact: wal }"
        );
        assert_eq!(
            fs::read(&unknown).expect("unknown evidence retained"),
            bytes
        );
    }
}

#[test]
fn backup_restore_crash_point_inventory_is_complete_and_non_overlapping() {
    let backup = BACKUP_CONFIRMATION_CRASH_POINTS
        .iter()
        .map(|point| point.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let switch = RESTORE_SWITCH_CRASH_POINTS
        .iter()
        .map(|point| point.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let retirement = RETIREMENT_CRASH_POINTS
        .iter()
        .map(|point| point.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let sidecar = SIDECAR_CLEANUP_CRASH_POINTS
        .iter()
        .map(|point| point.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(backup.len(), BACKUP_CONFIRMATION_CRASH_POINTS.len());
    assert_eq!(switch.len(), RESTORE_SWITCH_CRASH_POINTS.len());
    assert_eq!(retirement.len(), RETIREMENT_CRASH_POINTS.len());
    assert_eq!(sidecar.len(), SIDECAR_CLEANUP_CRASH_POINTS.len());
    assert!(backup.is_disjoint(&switch));
    assert!(backup.is_disjoint(&retirement));
    assert!(switch.is_disjoint(&retirement));
    assert!(sidecar.is_disjoint(&backup));
    assert!(sidecar.is_disjoint(&switch));
    assert!(sidecar.is_disjoint(&retirement));
    for required in [
        "write_gate_close",
        "checkpoint_current",
        "checkpoint_staging",
        "close_connections",
        "sidecar_convergence",
        "safe_snapshot_publish",
        "manifest_arm_staging_write",
        "manifest_arm_staging_fsync",
        "manifest_arm_rename",
        "manifest_arm_parent_fsync",
        "manifest_arm",
        "owner_prepared_staging_write",
        "owner_prepared_staging_fsync",
        "owner_prepared_rename",
        "owner_prepared_parent_fsync",
        "owner_prepared_publish",
        "database_switch",
        "plugin_tree_switch",
        "owner_applying_staging_write",
        "owner_applying_staging_fsync",
        "owner_applying_rename",
        "owner_applying_parent_fsync",
        "owner_applying_publish",
        "new_health_verify",
        "new_search_verify",
        "new_artifact_verify",
        "new_identity_verify",
        "owner_committed_staging_write",
        "owner_committed_staging_fsync",
        "owner_committed_rename",
        "owner_committed_parent_fsync",
        "owner_committed_publish",
        "retirement_prepared_staging_write",
        "retirement_prepared_staging_fsync",
        "retirement_prepared_rename",
        "retirement_prepared_parent_fsync",
        "retirement_prepared_publish",
        "live_to_tombstone_rename",
        "live_to_tombstone_parent_fsync",
        "live_to_tombstone_identity_verify",
        "retirement_renamed_staging_write",
        "retirement_renamed_staging_fsync",
        "retirement_renamed_rename",
        "retirement_renamed_parent_fsync",
        "retirement_renamed_publish",
        "tombstone_leaf_unlink",
        "tombstone_directory_fsync",
        "tombstone_rmdir",
        "retirement_done_staging_write",
        "retirement_done_staging_fsync",
        "retirement_done_rename",
        "retirement_done_parent_fsync",
        "retirement_done_publish",
        "retirement_journal_delete",
        "retirement_journal_unlink",
        "retirement_journal_unlink_parent_fsync",
        "sidecar_prepared_staging_fsync",
        "sidecar_prepared_publish",
        "sidecar_prepared_parent_fsync",
        "sidecar_quarantine_rename",
        "sidecar_quarantine_parent_fsync",
        "sidecar_quarantine_identity_verify",
        "sidecar_quarantined_staging_fsync",
        "sidecar_quarantined_publish",
        "sidecar_quarantined_parent_fsync",
        "sidecar_quarantine_unlink",
        "sidecar_quarantine_unlink_parent_fsync",
        "sidecar_done_staging_fsync",
        "sidecar_done_publish",
        "sidecar_done_parent_fsync",
        "sidecar_journal_unlink",
        "sidecar_journal_unlink_parent_fsync",
    ] {
        assert!(
            backup.contains(required)
                || switch.contains(required)
                || retirement.contains(required)
                || sidecar.contains(required),
            "missing crash boundary {required}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn prepared_owner_binds_every_controlled_old_new_and_safety_object() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "owner-evidence");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-owner-evidence-passphrase")
        .await
        .expect("export replacement state");

    let target = TestWorkspace::new().expect("target workspace");
    let root = target.root().join("owner-evidence-root");
    fs::create_dir_all(&root).expect("create target root");
    let current_database = root.join("datasources.db");
    let current_plugins = root.join("plugins");
    let old_store = Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
        .await
        .expect("open old current");
    old_store
        .create(
            "old-owner-evidence",
            "127.0.0.1",
            3306,
            "old-user",
            b"old-password",
        )
        .await
        .expect("seed old state");
    old_store.pool().close().await;
    drop(old_store);
    let old_database_sha256 = sha256_path(&current_database);
    let old_database_identity = sidecar_file_identity(&current_database);
    let old_plugin_identity = sidecar_file_identity(&current_plugins);

    let coordinator = RestoreCoordinator::new(&root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-owner-evidence-passphrase")
        .await
        .expect("prepare authenticated restore");
    let interrupted = coordinator
        .apply_with_crash(
            &plan,
            RESTORE_SWITCH_CRASH_POINTS
                .iter()
                .copied()
                .find(|point| point.as_str() == "owner_prepared_publish")
                .expect("owner prepared point"),
        )
        .await
        .expect_err("interrupt after durable prepared owner");
    assert!(interrupted.reached_requested_boundary());

    let live_relative = format!(
        ".hivegui-db-staging-v1/restore-{}",
        plan.db_instance_operation_id()
    );
    let live = root.join(&live_relative);
    let owner: serde_json::Value = serde_json::from_slice(
        &fs::read(live.join(".hivegui-db-recovery-v1.json")).expect("durable prepared owner"),
    )
    .expect("parse owner");
    assert_eq!(owner["phase"], "prepared");
    assert_eq!(
        owner["manifest_identity"],
        sidecar_file_identity(&live.join(".hivegui-db-instance-v1.json"))
    );
    assert_eq!(owner["old_database"]["relative_path"], "datasources.db");
    assert_eq!(owner["old_database"]["identity"], old_database_identity);
    assert_eq!(owner["old_database"]["sha256"], old_database_sha256);
    assert_eq!(
        owner["new_database"]["relative_path"],
        format!("{live_relative}/datasources.db")
    );
    assert_eq!(
        owner["new_database"]["sha256"],
        sha256_path(&live.join("datasources.db"))
    );
    assert_eq!(owner["old_plugin_root"]["relative_path"], "plugins");
    assert_eq!(owner["old_plugin_root"]["identity"], old_plugin_identity);
    assert_eq!(
        owner["new_plugin_root"]["relative_path"],
        format!("{live_relative}/plugins")
    );
    assert_eq!(
        owner["safety_snapshot"]["relative_path"],
        format!("backups/restore-safety-{}", plan.db_instance_operation_id())
    );
    for descriptor in [
        &owner["old_database"],
        &owner["new_database"],
        &owner["old_plugin_root"],
        &owner["new_plugin_root"],
        &owner["safety_snapshot"],
    ] {
        assert_eq!(descriptor["sha256"].as_str().map(str::len), Some(64));
        assert!(
            descriptor["identity"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        );
        assert!(
            descriptor["relative_path"]
                .as_str()
                .is_some_and(|value| !value.starts_with('/'))
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn prepared_retirement_binds_live_owner_terminal_current_and_plugin_tree() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "retirement-evidence");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-retirement-evidence-passphrase")
        .await
        .expect("export replacement state");

    let target = TestWorkspace::new().expect("target workspace");
    let root = target.root().join("retirement-evidence-root");
    fs::create_dir_all(&root).expect("create target root");
    let current_database = root.join("datasources.db");
    let current_plugins = root.join("plugins");
    let old_store = Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
        .await
        .expect("open old current");
    old_store.pool().close().await;
    drop(old_store);

    let coordinator = RestoreCoordinator::new(&root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-retirement-evidence-passphrase")
        .await
        .expect("prepare authenticated restore");
    let point = RETIREMENT_CRASH_POINTS
        .iter()
        .copied()
        .find(|point| point.as_str() == "retirement_prepared_publish")
        .expect("retirement prepared point");
    let interrupted = coordinator
        .apply_with_crash(&plan, point)
        .await
        .expect_err("interrupt after durable prepared retirement");
    assert!(interrupted.reached_requested_boundary());

    let live_basename = format!("restore-{}", plan.db_instance_operation_id());
    let registry = root.join(".hivegui-db-staging-v1");
    let live = registry.join(&live_basename);
    let journal: serde_json::Value = serde_json::from_slice(
        &fs::read(registry.join(format!(
            ".hivegui-db-retirement-v1-restore-{}.json",
            plan.db_instance_operation_id()
        )))
        .expect("durable prepared retirement"),
    )
    .expect("parse retirement journal");
    assert_eq!(journal["state"], "prepared");
    assert_eq!(journal["terminal_outcome"], "new");
    assert_eq!(
        journal["live_directory_identity"],
        sidecar_file_identity(&live)
    );
    assert_eq!(journal["manifest_ownership_state"], "armed");
    assert_eq!(journal["owner_phase"], "committed");
    assert!(
        journal["manifest_identity"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert!(
        journal["owner_identity"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert_eq!(
        journal["terminal_database"]["relative_path"],
        "datasources.db"
    );
    assert_eq!(
        journal["terminal_database"]["sha256"],
        sha256_path(&current_database)
    );
    assert_eq!(journal["terminal_plugin_root"]["relative_path"], "plugins");
    assert_eq!(
        journal["terminal_plugin_root"]["identity"],
        sidecar_file_identity(&current_plugins)
    );
    assert_eq!(
        journal["terminal_plugin_root"]["sha256"]
            .as_str()
            .map(str::len),
        Some(64)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn startup_reconciles_exact_next_manifest_staging_before_aborted_retirement() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "manifest-next-staging");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-manifest-next-passphrase")
        .await
        .expect("export replacement state");
    let target = TestWorkspace::new().expect("target workspace");
    let root = target.root().join("manifest-next-root");
    fs::create_dir_all(&root).expect("create target root");
    let coordinator = RestoreCoordinator::new(&root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-manifest-next-passphrase")
        .await
        .expect("prepare unarmed restore");
    let live = root
        .join(".hivegui-db-staging-v1")
        .join(format!("restore-{}", plan.db_instance_operation_id()));
    let final_path = live.join(".hivegui-db-instance-v1.json");
    let mut armed: serde_json::Value =
        serde_json::from_slice(&fs::read(&final_path).expect("read final unarmed manifest"))
            .expect("parse final unarmed manifest");
    armed["ownership_state"] = serde_json::Value::String("armed".into());
    fs::write(
        live.join(".hivegui-db-instance-v1.json.staging"),
        serde_json::to_vec(&armed).expect("encode next armed staging"),
    )
    .expect("plant exact next manifest staging");

    let replay = RestoreCoordinator::new(&root)
        .expect("restart coordinator")
        .recover_startup()
        .await
        .expect("exact unarmed-to-armed staging is discarded before retrying final");
    assert_eq!(
        replay.retirement_outcome(),
        RetirementOutcome::AbortedPreSwitch
    );
    assert_eq!(
        fs::read_dir(root.join(".hivegui-db-staging-v1"))
            .expect("registry")
            .count(),
        0
    );
}

#[tokio::test(flavor = "current_thread")]
async fn startup_reconciles_exact_next_owner_staging_and_restores_prepared_old() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "owner-next-staging");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-owner-next-passphrase")
        .await
        .expect("export replacement state");
    let target = TestWorkspace::new().expect("target workspace");
    let root = target.root().join("owner-next-root");
    fs::create_dir_all(&root).expect("create target root");
    let current_database = root.join("datasources.db");
    let current_plugins = root.join("plugins");
    let old_store = Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
        .await
        .expect("open old current");
    old_store
        .create(
            "old-owner-next",
            "127.0.0.1",
            3306,
            "old-user",
            b"old-password",
        )
        .await
        .expect("seed old current");
    old_store.pool().close().await;
    drop(old_store);
    let coordinator = RestoreCoordinator::new(&root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-owner-next-passphrase")
        .await
        .expect("prepare restore");
    let point = RESTORE_SWITCH_CRASH_POINTS
        .iter()
        .copied()
        .find(|point| point.as_str() == "owner_prepared_publish")
        .expect("prepared owner point");
    coordinator
        .apply_with_crash(&plan, point)
        .await
        .expect_err("interrupt after prepared owner");
    let live = root
        .join(".hivegui-db-staging-v1")
        .join(format!("restore-{}", plan.db_instance_operation_id()));
    let owner_path = live.join(".hivegui-db-recovery-v1.json");
    let mut applying: serde_json::Value =
        serde_json::from_slice(&fs::read(&owner_path).expect("read prepared owner"))
            .expect("parse prepared owner");
    applying["phase"] = serde_json::Value::String("applying".into());
    fs::write(
        live.join(".hivegui-db-recovery-v1.json.staging"),
        serde_json::to_vec(&applying).expect("encode applying owner staging"),
    )
    .expect("plant exact next owner staging");

    let replay = RestoreCoordinator::new(&root)
        .expect("restart coordinator")
        .recover_startup()
        .await
        .expect("discard next owner staging and replay durable prepared final");
    assert_eq!(replay.retirement_outcome(), RetirementOutcome::Old);
    let reopened = Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
        .await
        .expect("reopen restored old");
    assert_eq!(
        reopened
            .list()
            .await
            .expect("list old")
            .into_iter()
            .map(|row| row.name)
            .collect::<Vec<_>>(),
        vec!["old-owner-next"]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn startup_reconciles_exact_next_retirement_staging_before_replaying_final() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "retirement-next-staging");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-retirement-next-passphrase")
        .await
        .expect("export replacement state");
    let target = TestWorkspace::new().expect("target workspace");
    let root = target.root().join("retirement-next-root");
    fs::create_dir_all(&root).expect("create target root");
    let coordinator = RestoreCoordinator::new(&root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-retirement-next-passphrase")
        .await
        .expect("prepare restore");
    let point = RETIREMENT_CRASH_POINTS
        .iter()
        .copied()
        .find(|point| point.as_str() == "retirement_prepared_publish")
        .expect("prepared retirement point");
    coordinator
        .apply_with_crash(&plan, point)
        .await
        .expect_err("interrupt after prepared retirement");
    let registry = root.join(".hivegui-db-staging-v1");
    let journal_path = registry.join(format!(
        ".hivegui-db-retirement-v1-restore-{}.json",
        plan.db_instance_operation_id()
    ));
    let mut renamed: serde_json::Value =
        serde_json::from_slice(&fs::read(&journal_path).expect("read prepared retirement"))
            .expect("parse prepared retirement");
    renamed["state"] = serde_json::Value::String("renamed".into());
    fs::write(
        std::path::PathBuf::from(format!("{}.staging", journal_path.display())),
        serde_json::to_vec(&renamed).expect("encode renamed retirement staging"),
    )
    .expect("plant exact next retirement staging");

    let replay = RestoreCoordinator::new(&root)
        .expect("restart coordinator")
        .recover_startup()
        .await
        .expect("discard next retirement staging and replay prepared final");
    assert_eq!(replay.retirement_outcome(), RetirementOutcome::New);
    assert_eq!(
        fs::read_dir(&registry).expect("registry").count(),
        0,
        "retirement must converge completely"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn startup_discards_proven_unpublished_retirement_staging_and_retires_from_live_owner() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "retirement-staging-only");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-retirement-staging-only-passphrase")
        .await
        .expect("export replacement state");
    let target = TestWorkspace::new().expect("target workspace");
    let root = target.root().join("retirement-staging-only-root");
    fs::create_dir_all(&root).expect("create target root");
    let coordinator = RestoreCoordinator::new(&root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-retirement-staging-only-passphrase")
        .await
        .expect("prepare restore");
    let point = RETIREMENT_CRASH_POINTS
        .iter()
        .copied()
        .find(|point| point.as_str() == "retirement_prepared_publish")
        .expect("prepared retirement point");
    coordinator
        .apply_with_crash(&plan, point)
        .await
        .expect_err("interrupt after prepared retirement");
    let registry = root.join(".hivegui-db-staging-v1");
    let journal_path = registry.join(format!(
        ".hivegui-db-retirement-v1-restore-{}.json",
        plan.db_instance_operation_id()
    ));
    let staging = std::path::PathBuf::from(format!("{}.staging", journal_path.display()));
    fs::rename(&journal_path, &staging).expect("simulate crash before initial journal publish");

    let replay = RestoreCoordinator::new(&root)
        .expect("restart coordinator")
        .recover_startup()
        .await
        .expect("proven unpublished staging is discarded and rebuilt from live owner");
    assert_eq!(replay.retirement_outcome(), RetirementOutcome::New);
    assert_eq!(fs::read_dir(&registry).expect("registry").count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn every_restore_switch_and_retirement_crash_replays_or_fails_closed_at_armed_owner_gap() {
    let source = TestWorkspace::new().expect("source workspace");
    let database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "restore-crash-matrix");
    BackupExporter::new(&database)
        .export_age(&archive, "T119-crash-matrix-passphrase")
        .await
        .expect("seed authenticated archive");

    for point in RESTORE_SWITCH_CRASH_POINTS
        .iter()
        .chain(RETIREMENT_CRASH_POINTS.iter())
    {
        let target = TestWorkspace::new().expect("isolated target per crash point");
        let current_database = target.root().join("datasources.db");
        let current_plugins = target.root().join("plugins");
        let old_store =
            Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
                .await
                .expect("open old current per crash point");
        old_store
            .create(
                "old-before-crash",
                "127.0.0.1",
                3307,
                "old-user",
                b"old-password",
            )
            .await
            .expect("seed old current per crash point");
        drop(old_store);
        let coordinator =
            RestoreCoordinator::new(target.root()).expect("production restore coordinator");
        let plan = coordinator
            .prepare_restore(&archive, "T119-crash-matrix-passphrase")
            .await
            .expect("prepare unarmed restore instance");
        assert_ne!(
            plan.db_instance_operation_id(),
            plan.cleanup_operation_id(),
            "instance UUID and cleanup UUID are distinct"
        );
        assert_eq!(plan.manifest().format_version, 3);
        assert!(!plan.owner_exists());
        let injected = coordinator
            .apply_with_crash(&plan, *point)
            .await
            .expect_err("fault point must interrupt the first process");
        assert_eq!(injected.crash_point(), point.as_str());
        assert!(
            injected.reached_requested_boundary(),
            "fault injection must advance the real state machine to {}: {injected}",
            point.as_str()
        );

        let replay = RestoreCoordinator::new(target.root())
            .expect("restart coordinator")
            .recover_startup()
            .await;
        if matches!(
            point.as_str(),
            "manifest_arm_rename"
                | "manifest_arm_parent_fsync"
                | "manifest_arm"
                | "owner_prepared_staging_write"
                | "owner_prepared_staging_fsync"
        ) {
            assert_eq!(
                replay
                    .expect_err("armed manifest without a durable owner is blocked")
                    .to_string(),
                "storage_recovery_blocked { reason: sidecar_unknown_owner, artifact: wal }"
            );
            continue;
        }
        let replay = replay.expect("deterministic startup replay");
        assert!(replay.store_may_open());
        assert!(replay.has_exactly_one_live_database());
        assert!(replay.has_no_mixed_database_or_plugin_tree());
        assert!(replay.control_files_are_outside_live_tree());
        assert!(matches!(
            replay.retirement_outcome(),
            RetirementOutcome::AbortedPreSwitch | RetirementOutcome::Old | RetirementOutcome::New
        ));
        assert!(replay.retirement_is_done());
        assert!(replay.write_gate_is_open());
        let expected_aborted = matches!(
            point.as_str(),
            "manifest_arm_staging_write"
                | "manifest_arm_staging_fsync"
                | "retirement_journal_unlink"
                | "retirement_journal_unlink_parent_fsync"
        );
        let expected_new = matches!(
            point.as_str(),
            "owner_committed_rename"
                | "owner_committed_parent_fsync"
                | "owner_committed_publish"
                | "retirement_prepared_staging_write"
                | "retirement_prepared_staging_fsync"
                | "retirement_prepared_rename"
                | "retirement_prepared_parent_fsync"
                | "retirement_prepared_publish"
                | "live_to_tombstone_rename"
                | "live_to_tombstone_parent_fsync"
                | "live_to_tombstone_identity_verify"
                | "retirement_renamed_staging_write"
                | "retirement_renamed_staging_fsync"
                | "retirement_renamed_rename"
                | "retirement_renamed_parent_fsync"
                | "retirement_renamed_publish"
                | "tombstone_leaf_unlink"
                | "tombstone_directory_fsync"
                | "tombstone_rmdir"
                | "retirement_done_staging_write"
                | "retirement_done_staging_fsync"
                | "retirement_done_rename"
                | "retirement_done_parent_fsync"
                | "retirement_done_publish"
                | "retirement_journal_delete"
        );
        assert_eq!(
            replay.retirement_outcome(),
            if expected_aborted {
                RetirementOutcome::AbortedPreSwitch
            } else if expected_new {
                RetirementOutcome::New
            } else {
                RetirementOutcome::Old
            },
            "the replay outcome after {} is determined by the committed owner boundary",
            point.as_str()
        );
        let reopened =
            Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
                .await
                .expect("reopen the replay-selected current");
        let names = reopened
            .list()
            .await
            .expect("list replay-selected current")
            .into_iter()
            .map(|row| row.name)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            if expected_new
                || matches!(
                    point.as_str(),
                    "retirement_journal_unlink" | "retirement_journal_unlink_parent_fsync"
                )
            {
                vec!["backup-seed".to_string()]
            } else {
                vec!["old-before-crash".to_string()]
            },
            "selected state after {}",
            point.as_str()
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn startup_checks_current_cleanup_before_replaying_a_committed_live_owner() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "startup-cleanup-order");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-startup-cleanup-order-passphrase")
        .await
        .expect("export replacement state");

    let target = TestWorkspace::new().expect("target workspace");
    let current_database = write_restore_current_database(target.root()).await;
    let coordinator = RestoreCoordinator::new(target.root()).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-startup-cleanup-order-passphrase")
        .await
        .expect("prepare replacement");
    let committed = RESTORE_SWITCH_CRASH_POINTS
        .iter()
        .copied()
        .find(|point| point.as_str() == "owner_committed_publish")
        .expect("committed owner point");
    coordinator
        .apply_with_crash(&plan, committed)
        .await
        .expect_err("stop with a durable committed live owner");
    let live = target
        .root()
        .join(".hivegui-db-staging-v1")
        .join(format!("restore-{}", plan.db_instance_operation_id()));
    assert!(
        live.is_dir(),
        "committed owner is live before startup replay"
    );

    let residual = target.root().join("datasources.db-journal");
    let residual_bytes = vec![0_u8; 128];
    fs::write(&residual, &residual_bytes).expect("plant recoverable current sidecar");
    let current_before = sha256_path(&current_database);

    let error = RestoreCoordinator::new(target.root())
        .expect("startup coordinator")
        .recover_startup()
        .await
        .expect_err("current cleanup must block before owner replay");
    assert_eq!(
        error.to_string(),
        "storage_recovery_blocked { reason: sidecar_recoverable, artifact: rollback_journal }"
    );
    assert!(
        live.is_dir(),
        "blocked current cleanup must not retire or otherwise mutate the live owner"
    );
    assert_eq!(
        fs::read(&residual).expect("recoverable sidecar retained"),
        residual_bytes
    );
    assert_eq!(sha256_path(&current_database), current_before);
}

#[tokio::test(flavor = "current_thread")]
async fn confirmed_offline_restore_switches_complete_state_then_commits_and_retires() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_store = Store::open_local(StoreOpenOptions::new(
        source.database_path(),
        source.plugin_root(),
    ))
    .await
    .expect("open source Store");
    source_store
        .create(
            "restored-new",
            "127.0.0.1",
            3306,
            "new-user",
            b"new-password",
        )
        .await
        .expect("seed source state");
    let archive = unique_target_path(&source, "confirmed-offline-restore");
    BackupExporter::new(source.database_path())
        .export_age(&archive, "T130-confirmed-restore-passphrase")
        .await
        .expect("export source state");
    drop(source_store);

    let target = TestWorkspace::new().expect("target workspace");
    let target_root = target.root().join("current-data-root");
    fs::create_dir_all(&target_root).expect("create target data root");
    let current_database = target_root.join("datasources.db");
    let current_plugins = target_root.join("plugins");
    let old_store = Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
        .await
        .expect("open old current Store");
    old_store
        .create(
            "pre-restore-old",
            "127.0.0.1",
            3307,
            "old-user",
            b"old-password",
        )
        .await
        .expect("seed old state");
    drop(old_store);

    let coordinator = RestoreCoordinator::new(&target_root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-confirmed-restore-passphrase")
        .await
        .expect("prepare authenticated restore");
    let live = target_root
        .join(".hivegui-db-staging-v1")
        .join(format!("restore-{}", plan.db_instance_operation_id()));
    let instance_manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(live.join(".hivegui-db-instance-v1.json"))
            .expect("unarmed instance manifest must be durable before apply"),
    )
    .expect("parse unarmed instance manifest");
    assert_eq!(instance_manifest["schema_version"], 1);
    assert_eq!(instance_manifest["role"], "restore");
    assert_eq!(
        instance_manifest["db_instance_operation_id"],
        plan.db_instance_operation_id()
    );
    assert_eq!(
        instance_manifest["db_id"],
        format!("restore/{}", plan.db_instance_operation_id())
    );
    assert_eq!(instance_manifest["database_name"], "datasources.db");
    assert_eq!(instance_manifest["ownership_state"], "unarmed");
    assert_eq!(
        instance_manifest["cleanup_operation_id"],
        plan.cleanup_operation_id()
    );
    assert_ne!(
        plan.cleanup_operation_id(),
        plan.db_instance_operation_id(),
        "instance and cleanup lifetimes require distinct UUIDs"
    );
    assert!(!live.join(".hivegui-db-instance-v1.json.staging").exists());
    assert!(!live.join(".hivegui-db-recovery-v1.json").exists());
    assert!(!live.join(".hivegui-db-recovery-v1.json.staging").exists());
    let applied = coordinator
        .apply_restore(&plan)
        .await
        .expect("apply complete replacement");
    assert_eq!(applied.retirement_outcome(), RetirementOutcome::New);
    assert!(applied.retirement_is_done());
    assert!(applied.store_may_open());

    let restored = Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
        .await
        .expect("open committed new current");
    let names = restored
        .list()
        .await
        .expect("list committed state")
        .into_iter()
        .map(|row| row.name)
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["restored-new"]);
    assert!(
        target_root.join("backups").is_dir(),
        "the verified pre-switch safety snapshot remains available"
    );
    assert_eq!(
        fs::read_dir(target_root.join(".hivegui-db-staging-v1"))
            .expect("registry remains discoverable")
            .count(),
        0,
        "committed instance and retirement journal must be fully retired"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn restore_safety_snapshot_hashes_complete_database_and_owned_plugin_tree() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "safety-snapshot-completeness");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-safety-snapshot-passphrase")
        .await
        .expect("export replacement state");

    let target = TestWorkspace::new().expect("target workspace");
    let target_root = target.root().join("safety-current-root");
    fs::create_dir(&target_root).expect("create exact current root");
    let current_database = target_root.join("datasources.db");
    let current_plugins = target_root.join("plugins");
    let old_store = Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
        .await
        .expect("open old current Store");
    old_store
        .create(
            "safety-old-current",
            "127.0.0.1",
            3307,
            "old-user",
            b"old-secret",
        )
        .await
        .expect("seed old current entity");
    let artifact_key = "safety-old/1.0.0/plugin.wasm";
    let artifact_path = current_plugins.join(artifact_key);
    fs::create_dir_all(artifact_path.parent().expect("artifact parent"))
        .expect("create old Plugin directories");
    let wasm = b"\0asmT130-raw-safety-owned-wasm";
    fs::write(&artifact_path, wasm).expect("write old managed WASM");
    let wasm_sha256 = hex::encode(Sha256::digest(wasm));
    sqlx::query(
        "INSERT INTO plugins (identifier,name,version,s3_key,sha256,size_bytes,runtime,created_at,updated_at) \
         VALUES ('safety-old','Safety Old','1.0.0',?,?,?,'wasm32','2026-08-26T00:00:00Z','2026-08-26T00:00:00Z')",
    )
    .bind(artifact_key)
    .bind(&wasm_sha256)
    .bind(wasm.len() as i64)
    .execute(old_store.pool())
    .await
    .expect("seed old Plugin ownership");
    sqlx::query(
        "INSERT INTO plugin_artifact_operations \
         (operation_id,kind,staging_name,state,created_at,updated_at) \
         VALUES ('00000000-0000-4000-8000-000000001300','create','t130-safety-ledger','done','2026-08-26T00:00:00Z','2026-08-26T00:00:00Z')",
    )
    .execute(old_store.pool())
    .await
    .expect("seed old operation ledger");
    sqlx::query(
        "INSERT INTO plugin_artifact_gc \
         (artifact_key,source_operation_id,last_attempt_at,reason,state) \
         VALUES ('safety-obsolete/plugin.wasm','00000000-0000-4000-8000-000000001300',0,'safety-test','pending')",
    )
    .execute(old_store.pool())
    .await
    .expect("seed old GC ledger");
    drop(old_store);
    let old_database_sha256 = sha256_path(&current_database);

    let coordinator = RestoreCoordinator::new(&target_root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-safety-snapshot-passphrase")
        .await
        .expect("prepare replacement");
    coordinator
        .apply_restore(&plan)
        .await
        .expect("complete replacement and verified safety snapshot");

    let snapshots = target_root.join("backups");
    let mut snapshot_entries = fs::read_dir(&snapshots)
        .expect("list safety snapshots")
        .collect::<Result<Vec<_>, _>>()
        .expect("read safety snapshot entry");
    assert_eq!(snapshot_entries.len(), 1, "one operation owns one snapshot");
    let snapshot = snapshot_entries.pop().expect("single snapshot").path();
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(snapshot.join("manifest.json")).expect("read safety manifest"),
    )
    .expect("parse safety manifest");
    let database_descriptor = manifest["database"]
        .as_object()
        .expect("safety manifest must bind the raw database file");
    assert_eq!(database_descriptor["path"], "datasources.db");
    assert_eq!(database_descriptor["sha256"], old_database_sha256);
    assert_eq!(
        database_descriptor["size_bytes"],
        fs::metadata(snapshot.join("datasources.db"))
            .expect("snapshot database metadata")
            .len()
    );
    let plugin_descriptors = manifest["plugin_files"]
        .as_array()
        .expect("Plugin entries must be hash-bound descriptors");
    assert_eq!(plugin_descriptors.len(), 1);
    assert_eq!(
        plugin_descriptors[0]["path"],
        format!("plugins/{artifact_key}")
    );
    assert_eq!(plugin_descriptors[0]["sha256"], wasm_sha256);
    assert_eq!(plugin_descriptors[0]["size_bytes"], wasm.len() as u64);
    assert!(
        plugin_descriptors[0]["source_identity"]
            .as_str()
            .is_some_and(|identity| !identity.is_empty())
    );
    assert!(
        plugin_descriptors[0]["snapshot_identity"]
            .as_str()
            .is_some_and(|identity| !identity.is_empty())
    );
    assert_eq!(
        fs::read(snapshot.join("plugins").join(artifact_key)).expect("read safety snapshot WASM"),
        wasm
    );
    let snapshot_options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(snapshot.join("datasources.db"))
        .create_if_missing(false)
        .read_only(true);
    let snapshot_pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(snapshot_options)
        .await
        .expect("open raw safety database");
    let ledgers: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM plugin_artifact_operations), \
                (SELECT COUNT(*) FROM plugin_artifact_gc)",
    )
    .fetch_one(&snapshot_pool)
    .await
    .expect("read preserved internal ledgers");
    snapshot_pool.close().await;
    assert_eq!(
        ledgers,
        (1, 1),
        "raw safety snapshot preserves internal tables"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn applying_recovery_rebuilds_complete_old_state_from_verified_safety_snapshot() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "safety-fallback");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-safety-fallback-passphrase")
        .await
        .expect("export replacement state");

    let target = TestWorkspace::new().expect("target workspace");
    let root = target.root().join("safety-fallback-root");
    fs::create_dir_all(&root).expect("create target root");
    let current_database = root.join("datasources.db");
    let current_plugins = root.join("plugins");
    let old_store = Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
        .await
        .expect("open old current");
    old_store
        .create(
            "old-from-safety",
            "127.0.0.1",
            3306,
            "old-user",
            b"old-password",
        )
        .await
        .expect("seed old database");
    let wasm = include_bytes!("fixtures/plugins/shared-smoke/plugin.wasm");
    let wasm_sha256 = hex::encode(Sha256::digest(wasm));
    let old_wasm = current_plugins.join("old-plugin/1/plugin.wasm");
    fs::create_dir_all(old_wasm.parent().expect("old Plugin parent"))
        .expect("create old Plugin tree");
    fs::write(&old_wasm, wasm).expect("write old Plugin artifact");
    sqlx::query(
        "INSERT INTO plugins \
         (identifier,name,version,s3_key,sha256,size_bytes,runtime,created_at,updated_at) \
         VALUES ('old-plugin','Old Plugin','1.0.0','old-plugin/1/plugin.wasm',?,?,\
         'extism','2026-08-26T00:00:00Z','2026-08-26T00:00:00Z')",
    )
    .bind(&wasm_sha256)
    .bind(wasm.len() as i64)
    .execute(old_store.pool())
    .await
    .expect("seed old Plugin ownership");
    old_store.pool().close().await;
    drop(old_store);

    let coordinator = RestoreCoordinator::new(&root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-safety-fallback-passphrase")
        .await
        .expect("prepare authenticated restore");
    let point = RESTORE_SWITCH_CRASH_POINTS
        .iter()
        .copied()
        .find(|point| point.as_str() == "plugin_tree_switch")
        .expect("Plugin switch crash point");
    let interrupted = coordinator
        .apply_with_crash(&plan, point)
        .await
        .expect_err("interrupt after both database and Plugin switch");
    assert!(interrupted.reached_requested_boundary());

    let live = root
        .join(".hivegui-db-staging-v1")
        .join(format!("restore-{}", plan.db_instance_operation_id()));
    fs::remove_file(live.join("old-datasources.db")).expect("simulate unavailable old database");
    fs::remove_dir_all(live.join("old-plugins")).expect("simulate unavailable old Plugin tree");

    let replay = RestoreCoordinator::new(&root)
        .expect("restart coordinator")
        .recover_startup()
        .await
        .expect("verified safety snapshot must rebuild old state");
    assert_eq!(replay.retirement_outcome(), RetirementOutcome::Old);
    let reopened = Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
        .await
        .expect("reopen safety-restored old current");
    let names = reopened
        .list()
        .await
        .expect("list safety-restored database")
        .into_iter()
        .map(|row| row.name)
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["old-from-safety"]);
    assert_eq!(
        fs::read(current_plugins.join("old-plugin/1/plugin.wasm"))
            .expect("safety-restored Plugin artifact"),
        wasm
    );
}

#[tokio::test(flavor = "current_thread")]
async fn closed_checkpoint_converges_a_proven_safe_residual_through_cleanup_journal() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "safe-sidecar-cleanup");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-safe-sidecar-passphrase")
        .await
        .expect("export replacement state");

    let target = TestWorkspace::new().expect("target workspace");
    let target_root = target.root().join("safe-sidecar-current");
    fs::create_dir(&target_root).expect("create target root");
    let current_database = target_root.join("datasources.db");
    let current_plugins = target_root.join("plugins");
    let old_store = Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
        .await
        .expect("open old current");
    old_store
        .create(
            "old-before-safe-sidecar",
            "127.0.0.1",
            3307,
            "old-user",
            b"old-password",
        )
        .await
        .expect("seed old current");
    drop(old_store);

    let coordinator = RestoreCoordinator::new(&target_root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-safe-sidecar-passphrase")
        .await
        .expect("prepare unarmed restore");
    let live = target_root
        .join(".hivegui-db-staging-v1")
        .join(format!("restore-{}", plan.db_instance_operation_id()));
    let residual = live.join("datasources.db-journal");
    fs::write(&residual, [0_u8; 128]).expect("seed non-hot rollback-journal residual");

    let applied = coordinator
        .apply_restore(&plan)
        .await
        .expect("successful checkpoint proves and durably cleans the residual");
    assert_eq!(applied.retirement_outcome(), RetirementOutcome::New);
    assert!(!residual.exists());
    assert!(
        fs::read_dir(&target_root)
            .expect("scan current root controls")
            .all(|entry| {
                let name = entry
                    .expect("current root entry")
                    .file_name()
                    .to_string_lossy()
                    .to_string();
                !name.starts_with(".hivegui-sidecar-cleanup-v1-")
                    && !name.starts_with(".hivegui-sidecar-quarantine-v1-")
            }),
        "journal and quarantine must be durably retired before Store opens"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn every_sidecar_cleanup_durability_boundary_replays_without_switching_current() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "sidecar-crash-matrix");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-sidecar-crash-passphrase")
        .await
        .expect("export replacement state");

    for point in SIDECAR_CLEANUP_CRASH_POINTS {
        eprintln!("sidecar crash boundary: {}", point.as_str());
        let target = TestWorkspace::new().expect("isolated sidecar crash workspace");
        let root = target.root().join(point.as_str());
        fs::create_dir_all(&root).expect("create current root");
        let current_database = root.join("datasources.db");
        let current_plugins = root.join("plugins");
        let old_store =
            Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
                .await
                .expect("open old current");
        old_store
            .create(
                "old-before-sidecar-crash",
                "127.0.0.1",
                3306,
                "old-user",
                b"old-password",
            )
            .await
            .expect("seed old current");
        old_store.pool().close().await;
        drop(old_store);
        let old_sha256 = sha256_path(&current_database);

        let coordinator = RestoreCoordinator::new(&root).expect("restore coordinator");
        let plan = coordinator
            .prepare_restore(&archive, "T130-sidecar-crash-passphrase")
            .await
            .expect("prepare authenticated restore");
        let residual = root.join("datasources.db-journal");
        let residual_bytes = vec![0_u8; 128];
        fs::write(&residual, &residual_bytes).expect("plant proven closed residual");

        let interrupted = coordinator
            .apply_with_crash(&plan, point)
            .await
            .expect_err("the requested cleanup durability point must interrupt");
        assert_eq!(interrupted.crash_point(), point.as_str());
        assert!(
            interrupted.reached_requested_boundary(),
            "cleanup must execute the real durability boundary {}: {interrupted}",
            point.as_str()
        );
        assert_eq!(
            sha256_path(&current_database),
            old_sha256,
            "cleanup interruption must not switch current"
        );

        let replay = RestoreCoordinator::new(&root)
            .expect("restart coordinator")
            .recover_startup()
            .await;
        if matches!(
            point.as_str(),
            "sidecar_prepared_staging_fsync"
                | "sidecar_quarantined_staging_fsync"
                | "sidecar_done_staging_fsync"
        ) {
            let error = replay.expect_err("ambiguous staging state must remain fail-closed");
            let expected_reason = if point.as_str() == "sidecar_prepared_staging_fsync" {
                "sidecar_recoverable"
            } else {
                "sidecar_unknown_owner"
            };
            assert_eq!(
                error.to_string(),
                format!(
                    "storage_recovery_blocked {{ reason: {expected_reason}, artifact: rollback_journal }}"
                )
            );
            if point.as_str() == "sidecar_prepared_staging_fsync" {
                assert_eq!(
                    fs::read(&residual).expect("canonical residual retained"),
                    residual_bytes
                );
            }
            continue;
        }
        let replay = replay.unwrap_or_else(|error| {
            panic!(
                "durable cleanup state {} must replay to old current: {error}",
                point.as_str()
            )
        });
        assert_eq!(
            replay.retirement_outcome(),
            RetirementOutcome::AbortedPreSwitch
        );
        assert!(replay.store_may_open());
        assert!(!residual.exists());
        let reopened =
            Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
                .await
                .expect("reopen old current after cleanup replay");
        let names = reopened
            .list()
            .await
            .expect("read replay-selected old state")
            .into_iter()
            .map(|row| row.name)
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["old-before-sidecar-crash"]);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn restore_refuses_open_store_clones_before_freezing_or_checkpointing_current() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "connections-open");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-connections-open-passphrase")
        .await
        .expect("export replacement state");

    let target = TestWorkspace::new().expect("target workspace");
    let root = target.root().join("connections-open-root");
    fs::create_dir_all(&root).expect("create restore root");
    let database = root.join("datasources.db");
    let plugin_root = root.join("plugins");
    let store = Store::open_local(StoreOpenOptions::new(&database, &plugin_root))
        .await
        .expect("open current Store");
    store
        .create(
            "old-current",
            "127.0.0.1",
            3306,
            "old-user",
            b"old-password",
        )
        .await
        .expect("seed current state");
    let retained_clone = store.clone();
    drop(store);

    let coordinator = RestoreCoordinator::new(&root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-connections-open-passphrase")
        .await
        .expect("prepare authenticated restore");
    let error = coordinator
        .apply_restore(&plan)
        .await
        .expect_err("an open Store clone must block the closed-database boundary");
    assert_eq!(
        error.to_string(),
        "storage_recovery_blocked { reason: connections_open, artifact: connection }"
    );
    retained_clone
        .create(
            "still-open-after-refusal",
            "127.0.0.1",
            3307,
            "old-user",
            b"still-writable",
        )
        .await
        .expect("refusal occurs before the write gate is frozen");
    let names = retained_clone
        .list()
        .await
        .expect("read unchanged current")
        .into_iter()
        .map(|row| row.name)
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["old-current", "still-open-after-refusal"]);
}

#[tokio::test(flavor = "current_thread")]
async fn restore_reports_checkpoint_busy_without_reclassifying_or_switching_current() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "checkpoint-busy");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-checkpoint-busy-passphrase")
        .await
        .expect("export replacement state");

    let target = TestWorkspace::new().expect("target workspace");
    let root = target.root().join("checkpoint-busy-root");
    fs::create_dir_all(&root).expect("create restore root");
    let database = root.join("datasources.db");
    let plugin_root = root.join("plugins");
    let store = Store::open_local(StoreOpenOptions::new(&database, &plugin_root))
        .await
        .expect("open old current Store");
    store
        .create(
            "old-current",
            "127.0.0.1",
            3306,
            "old-user",
            b"old-password",
        )
        .await
        .expect("seed old current");
    store.pool().close().await;
    drop(store);

    let coordinator = RestoreCoordinator::new(&root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-checkpoint-busy-passphrase")
        .await
        .expect("prepare authenticated restore");
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&database)
        .create_if_missing(false)
        .foreign_keys(true);
    let reader_pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone())
        .await
        .expect("open raw reader");
    let writer_pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("open raw writer");
    sqlx::query("PRAGMA journal_mode=WAL")
        .execute(&writer_pool)
        .await
        .expect("force WAL mode");
    let mut reader = reader_pool.acquire().await.expect("acquire reader");
    sqlx::query("BEGIN")
        .execute(&mut *reader)
        .await
        .expect("start reader snapshot");
    let _: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM data_sources")
        .fetch_one(&mut *reader)
        .await
        .expect("materialize reader snapshot");
    sqlx::query(
        "INSERT INTO data_sources \
         (name,host,port,username,encrypted_password,created_at,updated_at) \
         VALUES ('committed-in-wal','127.0.0.1',3307,'wal-user',X'01',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)",
    )
    .execute(&writer_pool)
    .await
    .expect("commit a frame newer than the reader snapshot");
    writer_pool.close().await;
    let wal = std::path::PathBuf::from(format!("{}-wal", database.display()));
    let wal_before = fs::read(&wal).expect("hot WAL remains present");

    let error = coordinator
        .apply_restore(&plan)
        .await
        .expect_err("busy checkpoint must block before switching current");
    assert_eq!(
        error.to_string(),
        "storage_recovery_blocked { reason: checkpoint_busy, artifact: checkpoint }"
    );
    assert_eq!(
        fs::read(&wal).expect("busy WAL remains canonical"),
        wal_before,
        "a busy checkpoint must not route the WAL through cleanup"
    );
    sqlx::query("ROLLBACK")
        .execute(&mut *reader)
        .await
        .expect("release reader snapshot");
    drop(reader);
    reader_pool.close().await;
}

#[tokio::test(flavor = "current_thread")]
async fn restore_reports_checkpoint_failed_without_leaking_sqlite_text_or_switching_current() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_database = write_seed_database(&source).await;
    let archive = unique_target_path(&source, "checkpoint-failed");
    BackupExporter::new(&source_database)
        .export_age(&archive, "T130-checkpoint-failed-passphrase")
        .await
        .expect("export replacement state");

    let target = TestWorkspace::new().expect("target workspace");
    let root = target.root().join("checkpoint-failed-root");
    fs::create_dir_all(&root).expect("create restore root");
    let database = root.join("datasources.db");
    let plugin_root = root.join("plugins");
    let store = Store::open_local(StoreOpenOptions::new(&database, &plugin_root))
        .await
        .expect("open old current Store");
    store.pool().close().await;
    drop(store);

    let coordinator = RestoreCoordinator::new(&root).expect("restore coordinator");
    let plan = coordinator
        .prepare_restore(&archive, "T130-checkpoint-failed-passphrase")
        .await
        .expect("prepare authenticated restore");
    let corrupt = b"not-a-sqlite-database-and-never-a-secret";
    fs::write(&database, corrupt).expect("replace current with corrupt fixture");

    let error = coordinator
        .apply_restore(&plan)
        .await
        .expect_err("non-busy checkpoint failure must block");
    assert_eq!(
        error.to_string(),
        "storage_recovery_blocked { reason: checkpoint_failed, artifact: checkpoint }"
    );
    assert_eq!(
        fs::read(&database).expect("corrupt current retained"),
        corrupt,
        "checkpoint failure must not switch or rewrite current"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn startup_replay_restores_prepared_old_but_finishes_committed_new() {
    let source = TestWorkspace::new().expect("source workspace");
    let source_store = Store::open_local(StoreOpenOptions::new(
        source.database_path(),
        source.plugin_root(),
    ))
    .await
    .expect("open source Store");
    source_store
        .create(
            "new-from-archive",
            "127.0.0.1",
            3306,
            "new",
            b"new-password",
        )
        .await
        .expect("seed new archive row");
    let archive = unique_target_path(&source, "owner-replay");
    BackupExporter::new(source.database_path())
        .export_age(&archive, "T130-owner-replay-passphrase")
        .await
        .expect("export new state");
    drop(source_store);

    for phase in ["prepared", "committed"] {
        let target = TestWorkspace::new().expect("isolated target");
        let root = target.root().join(format!("{phase}-current"));
        fs::create_dir_all(&root).expect("create current root");
        let current_database = root.join("datasources.db");
        let current_plugins = root.join("plugins");
        let old_store =
            Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
                .await
                .expect("open old Store");
        old_store
            .create(
                "old-before-restore",
                "127.0.0.1",
                3307,
                "old",
                b"old-password",
            )
            .await
            .expect("seed old row");
        drop(old_store);
        let coordinator = RestoreCoordinator::new(&root).expect("coordinator");
        let plan = coordinator
            .prepare_restore(&archive, "T130-owner-replay-passphrase")
            .await
            .expect("prepare restore");
        let point_name = if phase == "prepared" {
            "owner_prepared_publish"
        } else {
            "owner_committed_publish"
        };
        let point = RESTORE_SWITCH_CRASH_POINTS
            .iter()
            .copied()
            .find(|point| point.as_str() == point_name)
            .expect("owner phase crash point");
        let interrupted = coordinator
            .apply_with_crash(&plan, point)
            .await
            .expect_err("interrupt at durable owner phase");
        assert!(interrupted.reached_requested_boundary());

        let replay = RestoreCoordinator::new(&root)
            .expect("restart coordinator")
            .recover_startup()
            .await
            .expect("replay owner state");
        assert_eq!(
            replay.retirement_outcome(),
            if phase == "prepared" {
                RetirementOutcome::Old
            } else {
                RetirementOutcome::New
            }
        );
        let reopened =
            Store::open_local(StoreOpenOptions::new(&current_database, &current_plugins))
                .await
                .expect("reopen proven outcome");
        let names = reopened
            .list()
            .await
            .expect("list proven outcome")
            .into_iter()
            .map(|row| row.name)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            if phase == "prepared" {
                vec!["old-before-restore"]
            } else {
                vec!["new-from-archive"]
            }
        );
        assert_eq!(
            fs::read_dir(root.join(".hivegui-db-staging-v1"))
                .expect("registry")
                .count(),
            0
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn backup_media_canary_is_absent_after_success_error_crash_and_cross_device_restore() {
    let source = TestWorkspace::new().expect("source workspace");
    let target = TestWorkspace::new().expect("target workspace");
    let source_store = Store::open_local(StoreOpenOptions::new(
        source.database_path(),
        source.plugin_root(),
    ))
    .await
    .expect("open source Store");
    let canary = place_canary_for_test(
        SensitiveField::DataSourcePassword,
        &unique_canary_payload("T119-backup-all-media"),
    )
    .expect("backup canary");
    source_store
        .create(
            "backup-canary",
            "127.0.0.1",
            3306,
            "canary-user",
            canary.plaintext_token().as_bytes(),
        )
        .await
        .expect("persist encrypted source canary");
    let archive = source.root().join("backups/final/t119-canary.age");
    let coordinator = BackupCoordinator::from_store(source_store).expect("backup coordinator");
    let preview = coordinator
        .preview_export(&archive)
        .await
        .expect("preview canary export");
    coordinator
        .confirm_export(preview, "T119-canary-passphrase")
        .await
        .expect("confirmed canary export");

    let restore = RestoreCoordinator::new(target.root()).expect("restore coordinator");
    let wrong = restore
        .prepare_restore(&archive, "wrong-passphrase")
        .await
        .expect_err("wrong passphrase must fail before staging plaintext");
    assert!(!wrong.to_string().contains(&canary.plaintext_token()));
    let plan = restore
        .prepare_restore(&archive, "T119-canary-passphrase")
        .await
        .expect("prepare cross-device restore");
    restore
        .apply_with_crash(&plan, RESTORE_SWITCH_CRASH_POINTS[1])
        .await
        .expect_err("inject crash after the durable prepared owner");
    let replay = RestoreCoordinator::new(target.root())
        .expect("restart restore coordinator")
        .recover_startup()
        .await
        .expect("replay crash without plaintext residue");
    assert!(replay.store_may_open());

    for root in [source.root(), target.root()] {
        let scan = scan_all_mediums_for_test(root, &canary).expect("scan all backup media");
        assert!(
            scan.hits.is_empty(),
            "backup canary leaked through success/error/crash/cross-device path: {:?}",
            scan.hits
        );
    }
}
