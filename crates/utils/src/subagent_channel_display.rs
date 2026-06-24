/// Strip internal subagent inject scaffolding for human-facing channel surfaces.
///
/// Persisted subagent announcements mirror `agent/subagent_announce.md`: header,
/// full `Task:` assignment (model context), `Result:`, and a trailing model-only
/// `Summarize…` instruction. External channels (embedded WebUI, session previews)
/// should show only the header plus a truncated result body.

/// Cap Result section length so WebSocket session replay stays readable; full text
/// remains on disk for LLM replay (we only mutate outgoing API copies in websocket).
const SUBAGENT_CHANNEL_RESULT_MAX_CHARS: usize = 800;

/// Return channel-safe text derived from a full subagent announce blob.
pub fn scrub_subagent_announce_body(content: &str) -> String {
    let stripped = content.replace("\r\n", "\n").trim().to_string();
    let lines: Vec<&str> = stripped.lines().collect();

    let header = if !lines.is_empty() && lines[0].starts_with("[Subagent") {
        lines[0].trim().to_string()
    } else {
        String::new()
    };

    let lower = stripped.to_lowercase();

    let ri = lower.find("\nresult:\n").or_else(|| lower.find("\nresult:"));

    if ri.is_none() {
        if !header.is_empty() {
            return header;
        }
        return stripped;
    }

    let ri = ri.unwrap();
    let key = if stripped[ri..].starts_with("\nresult:\n") {
        "\nresult:\n"
    } else {
        "\nresult:"
    };

    let after = stripped[ri + key.len()..].trim_start();

    let summ_marker = "summarize this naturally";
    let after = if let Some(si) = after.to_lowercase().find(summ_marker) {
        after[..si].trim_end()
    } else {
        after
    };

    let mut body = after.trim().to_string();
    let limit = SUBAGENT_CHANNEL_RESULT_MAX_CHARS;
    if body.len() > limit {
        let truncated: String = body.chars().take(limit - 1).collect();
        body = format!("{}\u{2026}", truncated.trim_end());
    }

    if !header.is_empty() && !body.is_empty() {
        return format!("{header}\n\n{body}");
    }

    if !header.is_empty() {
        return header;
    }
    if !body.is_empty() {
        return body;
    }

    stripped
}

/// Mutate message dicts in place when they carry `subagent_result` inject.
pub fn scrub_subagent_messages_for_channel(messages: &mut [serde_json::Value]) {
    for msg in messages {
        let Some(obj) = msg.as_object_mut() else {
            continue;
        };

        let is_subagent = obj
            .get("injected_event")
            .and_then(|v| v.as_str())
            .map(|s| s == "subagent_result")
            .unwrap_or(false);

        if !is_subagent {
            continue;
        }

        let raw = obj.get("content").and_then(|v| v.as_str());
        let Some(raw) = raw else { continue };
        if raw.trim().is_empty() {
            continue;
        }

        let cleaned = scrub_subagent_announce_body(raw);
        obj.insert("content".to_string(), serde_json::Value::String(cleaned));
    }
}
