//! Smoke test for FR-002: the HiveClaw binary starts, logs its name and
//! version in a structured (JSON) line on stderr, and exits with success
//! when sent SIGINT.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn binary_starts_logs_version_and_exits_clean_on_sigint() {
    let bin = env!("CARGO_BIN_EXE_hiveclaw");

    let mut child = Command::new(bin)
        // Bind to an ephemeral port so the test doesn't conflict with
        // another instance.
        .env("HIVECLAW_BIND_ADDR", "127.0.0.1:0")
        .env("HIVECLAW_LOG_LEVEL", "info")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hiveclaw binary should be runnable");

    let stderr = child.stderr.take().expect("piped stderr");
    let reader = BufReader::new(stderr);

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut listening_line: Option<String> = None;

    for line in reader.lines().map_while(Result::ok) {
        if line.contains("HiveClaw listening") {
            listening_line = Some(line);
            break;
        }
        if Instant::now() > deadline {
            break;
        }
    }

    let line = listening_line.expect("HiveClaw should log a 'HiveClaw listening' line");

    // Give the runtime a moment to install its SIGINT handler so the kill
    // below is delivered to our graceful-shutdown path rather than the
    // default terminate-on-signal handler.
    std::thread::sleep(Duration::from_millis(200));

    let parsed: serde_json::Value =
        serde_json::from_str(&line).expect("stderr log line must be JSON");
    let fields = parsed
        .get("fields")
        .expect("log line must carry a 'fields' object");
    assert_eq!(
        fields.get("message").and_then(|v| v.as_str()),
        Some("HiveClaw listening"),
        "expected message field 'HiveClaw listening'"
    );
    assert_eq!(
        fields.get("version").and_then(|v| v.as_str()),
        Some(env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(
        parsed.get("target").and_then(|v| v.as_str()),
        Some("hiveclaw")
    );

    // Send SIGINT (graceful shutdown) and assert clean exit.
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        // SAFETY: kill() is async-signal-safe; we are not in a signal handler.
        unsafe {
            libc::kill(pid, libc::SIGINT);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }

    let status = child.wait().expect("child should be waitable");
    assert!(
        status.success(),
        "hiveclaw should exit success after SIGINT, got {status:?}"
    );
}
