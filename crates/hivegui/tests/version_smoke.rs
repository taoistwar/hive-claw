//! Smoke test for HiveGUI: when `HIVEGUI_HEADLESS=1` is set, the binary
//! initialises config + logging, prints a JSON line carrying `version`,
//! and exits 0 before opening the gpui window. This is the test hook
//! that lets CI verify the crate builds and links to gpui without
//! requiring a display server.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

#[test]
fn headless_binary_prints_version_and_exits_clean() {
    let bin = env!("CARGO_BIN_EXE_hivegui");
    let tmp = tempdir_or_skip();

    let mut child = Command::new(bin)
        .env("HIVEGUI_HEADLESS", "1")
        .env("HIVEGUI_LOG_DIR", &tmp)
        .env("HIVEGUI_LOG_LEVEL", "info")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hivegui binary should be runnable");

    let stderr = child.stderr.take().expect("piped stderr");
    let reader = BufReader::new(stderr);
    let mut saw_version_line = false;
    let pkg_version = env!("CARGO_PKG_VERSION");

    for line in reader.lines().map_while(Result::ok) {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&line) {
            let fields = parsed.get("fields").cloned().unwrap_or_default();
            if fields
                .get("version")
                .and_then(|v| v.as_str())
                .map(|s| s == pkg_version)
                .unwrap_or(false)
            {
                saw_version_line = true;
            }
        }
    }

    let status = child.wait().expect("child should be waitable");
    assert!(
        saw_version_line,
        "expected a JSON log line containing version={pkg_version}"
    );
    assert!(
        status.success(),
        "hivegui --headless should exit success, got {status:?}"
    );
}

fn tempdir_or_skip() -> String {
    let p = std::env::temp_dir().join(format!("hivegui-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&p).expect("create tempdir");
    p.to_string_lossy().to_string()
}
