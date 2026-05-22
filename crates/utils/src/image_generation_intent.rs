/// Helpers for WebUI image-generation intent metadata.

use std::collections::HashMap;

use serde_json::Value;

const IMAGE_GENERATION_METADATA_KEY: &str = "image_generation";

/// Decorate a user prompt when WebUI image mode is enabled.
pub fn image_generation_prompt(content: &str, metadata: Option<&HashMap<String, Value>>) -> String {
    let raw = match metadata.and_then(|m| m.get(IMAGE_GENERATION_METADATA_KEY)) {
        Some(Value::Object(map)) => map,
        _ => return content.to_string(),
    };

    if raw.get("enabled").and_then(|v| v.as_bool()) != Some(true) {
        return content.to_string();
    }

    let instruction = match raw.get("aspect_ratio").and_then(|v| v.as_str()) {
        Some(ar) if !ar.trim().is_empty() => {
            format!(
                "The user selected WebUI image generation mode. Use the generate_image tool. \
                 When calling generate_image, pass aspect_ratio={ar:?}."
            )
        }
        _ => {
            "The user selected WebUI image generation mode. Use the generate_image tool. \
             Choose the most suitable aspect_ratio yourself from the prompt and intended use."
                .to_string()
        }
    };

    format!("{content}\n\n[WebUI image generation instruction: {instruction}]")
}
