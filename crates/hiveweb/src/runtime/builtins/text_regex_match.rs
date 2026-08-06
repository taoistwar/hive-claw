use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{BuiltinContext, BuiltinError, BuiltinResult};

#[derive(Debug, Deserialize)]
struct RegexMatchArgs {
    text: String,
    pattern: String,
    #[serde(default)]
    all: bool,
}

#[derive(Debug, Serialize)]
struct RegexMatchReply {
    matches: Vec<RegexMatchEntry>,
}

#[derive(Debug, Serialize)]
struct RegexMatchEntry {
    full: String,
    groups: Vec<Option<String>>,
}

pub fn text_regex_match(args: Value, _ctx: &BuiltinContext) -> BuiltinResult {
    let parsed: RegexMatchArgs =
        serde_json::from_value(args).map_err(|e| BuiltinError::BadArgs(format!("{e}")))?;
    let re = Regex::new(&parsed.pattern)
        .map_err(|e| BuiltinError::BadArgs(format!("pattern compile: {e}")))?;
    let collect_one = |caps: regex::Captures| RegexMatchEntry {
        full: caps
            .get(0)
            .map(|m| m.as_str().to_string())
            .unwrap_or_default(),
        groups: caps
            .iter()
            .skip(1)
            .map(|opt| opt.map(|m| m.as_str().to_string()))
            .collect(),
    };
    let matches: Vec<RegexMatchEntry> = if parsed.all {
        re.captures_iter(&parsed.text).map(collect_one).collect()
    } else {
        re.captures(&parsed.text)
            .map(collect_one)
            .into_iter()
            .collect()
    };
    serde_json::to_value(RegexMatchReply { matches })
        .map_err(|e| BuiltinError::Exec(format!("{e}")))
}

pub const TEXT_REGEX_MATCH_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "text": { "type": "string" },
    "pattern": { "type": "string", "description": "Rust regex syntax" },
    "all": { "type": "boolean", "default": false }
  },
  "required": ["text", "pattern"]
}"#;

pub const TEXT_REGEX_MATCH_OUTPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "matches": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "full": { "type": "string" },
          "groups": { "type": "array", "items": { "type": ["string", "null"] } }
        }
      }
    }
  }
}"#;
