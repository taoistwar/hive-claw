//! Image generation provider helpers.
//!
//! Port of `nanobot.providers.image_generation`.

use std::collections::HashMap;
use std::path::Path;

use base64::Engine;
use log::error;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::registry::find_by_name;
use utils::detect_image_mime;

const _OPENROUTER_ATTRIBUTION_HEADERS: &[(&str, &str)] = &[
    ("HTTP-Referer", "https://github.com/HKUDS/nanobot"),
    ("X-OpenRouter-Title", "nanobot"),
    ("X-OpenRouter-Categories", "cli-agent,personal-agent"),
];
const _DEFAULT_TIMEOUT_S: f64 = 120.0;
const _AIHUBMIX_TIMEOUT_S: f64 = 300.0;
const _GEMINI_DEFAULT_TIMEOUT_S: f64 = 120.0;
const _MINIMAX_TIMEOUT_S: f64 = 300.0;
const _STEPFUN_TIMEOUT_S: f64 = 120.0;

#[derive(Error, Debug, Clone)]
pub enum ImageGenerationError {
    #[error("image generation error: {0}")]
    Generation(String),
    #[error("unsupported reference image: {0}")]
    UnsupportedImage(String),
    #[error("generated image payload was not valid base64")]
    InvalidBase64,
    #[error("generated image payload was not a supported image")]
    UnsupportedPayload,
    #[error("generated image URL did not return a supported image")]
    UnsupportedUrl,
    #[error("failed to download generated image: {0}")]
    DownloadFailed(String),
    #[error("HTTP error: {status} - {detail}")]
    HttpError { status: u16, detail: String },
    #[error("request error: {0}")]
    RequestError(String),
    #[error("timeout")]
    Timeout,
}

impl ImageGenerationError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self::Generation(msg.into())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedImageResponse {
    pub images: Vec<String>,
    pub content: String,
    pub raw: Value,
}

fn _read_image_b64(path: &Path) -> Result<(String, String), ImageGenerationError> {
    let raw = std::fs::read(path).map_err(|e| ImageGenerationError::Generation(e.to_string()))?;
    let mime = detect_image_mime(&raw)
        .ok_or_else(|| ImageGenerationError::UnsupportedImage(path.display().to_string()))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&raw);
    Ok((mime.to_string(), encoded))
}

pub fn image_path_to_data_url(path: &Path) -> Result<String, ImageGenerationError> {
    let (mime, encoded) = _read_image_b64(path)?;
    Ok(format!("data:{};base64,{}", mime, encoded))
}

pub fn image_path_to_inline_data(path: &Path) -> Result<HashMap<String, String>, ImageGenerationError> {
    let (mime, encoded) = _read_image_b64(path)?;
    let mut map = HashMap::new();
    map.insert("mimeType".to_string(), mime);
    map.insert("data".to_string(), encoded);
    Ok(map)
}

fn _b64_image_data_url(value: &str) -> Result<String, ImageGenerationError> {
    let encoded: String = value.split_whitespace().collect();
    let raw = base64::engine::general_purpose::STANDARD
        .decode(&encoded)
        .map_err(|_| ImageGenerationError::InvalidBase64)?;
    let mime = detect_image_mime(&raw)
        .ok_or_else(|| ImageGenerationError::UnsupportedPayload)?;
    Ok(format!("data:{};base64,{}", mime, encoded))
}

fn _aihubmix_size(aspect_ratio: Option<&str>, image_size: Option<&str>) -> String {
    if let Some(size) = image_size {
        if size.to_lowercase().contains('x') {
            return size.to_string();
        }
    }
    if let Some(ar) = aspect_ratio {
        for &(k, v) in _AIHUBMIX_ASPECT_RATIO_SIZES {
            if k == ar {
                return v.to_string();
            }
        }
    }
    "auto".to_string()
}

static _AIHUBMIX_ASPECT_RATIO_SIZES: &[(&str, &str)] = &[
    ("1:1", "1024x1024"),
    ("3:4", "1024x1536"),
    ("9:16", "1024x1536"),
    ("4:3", "1536x1024"),
    ("16:9", "1536x1024"),
];

fn _aihubmix_model_path(model: &str) -> String {
    if model.contains('/') {
        return model.to_string();
    }
    if model.starts_with("gpt-image-") || model.starts_with("dall-e-") {
        return format!("openai/{}", model);
    }
    model.to_string()
}

async fn _download_image_data_url(
    client: &Client,
    url: &str,
) -> Result<String, ImageGenerationError> {
    let response = client.get(url).send().await.map_err(|e| ImageGenerationError::RequestError(e.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        let detail = response.text().await.unwrap_or_default();
        let preview: String = detail.chars().take(500).collect();
        return Err(ImageGenerationError::DownloadFailed(preview));
    }
    let raw = response.bytes().await.map_err(|e| ImageGenerationError::RequestError(e.to_string()))?;
    let mime = detect_image_mime(&raw)
        .ok_or_else(|| ImageGenerationError::UnsupportedUrl)?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&raw);
    Ok(format!("data:{};base64,{}", mime, encoded))
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

type ImageGenProviderCtor = fn(
    api_key: Option<String>,
    api_base: Option<String>,
    extra_headers: Option<HashMap<String, String>>,
    extra_body: Option<Value>,
    timeout: Option<f64>,
    client: Option<Client>,
) -> Box<dyn ImageGenerationProvider>;

static _IMAGE_GEN_PROVIDERS: std::sync::OnceLock<std::sync::Mutex<HashMap<String, ImageGenProviderCtor>>> = std::sync::OnceLock::new();

fn _get_registry() -> &'static std::sync::Mutex<HashMap<String, ImageGenProviderCtor>> {
    _IMAGE_GEN_PROVIDERS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

pub fn register_image_gen_provider(name: &str, ctor: ImageGenProviderCtor) {
    _get_registry().lock().unwrap().insert(name.to_string(), ctor);
}

pub fn get_image_gen_provider(name: &str) -> Option<ImageGenProviderCtor> {
    _get_registry().lock().unwrap().get(name).copied()
}

pub fn image_gen_provider_names() -> Vec<String> {
    _get_registry().lock().unwrap().keys().cloned().collect()
}

// ---------------------------------------------------------------------------
// Base trait
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
pub trait ImageGenerationProvider: Send + Sync {
    fn provider_name(&self) -> &str;
    fn missing_key_message(&self) -> &str;
    fn default_timeout(&self) -> f64 {
        _DEFAULT_TIMEOUT_S
    }

    fn resolve_base_url(&self, api_base: Option<&str>) -> String {
        if let Some(base) = api_base {
            return base.trim_end_matches('/').to_string();
        }
        let spec = find_by_name(self.provider_name());
        if let Some(s) = spec {
            if !s.default_api_base.is_empty() {
                return s.default_api_base.trim_end_matches('/').to_string();
            }
        }
        self.default_base_url()
    }

    fn default_base_url(&self) -> String {
        String::new()
    }

    async fn generate(
        &self,
        prompt: &str,
        model: &str,
        reference_images: Option<&[String]>,
        aspect_ratio: Option<&str>,
        image_size: Option<&str>,
    ) -> Result<GeneratedImageResponse, ImageGenerationError>;

    fn require_images(&self, images: &[String], data: &Value) -> Result<(), ImageGenerationError> {
        if !images.is_empty() {
            return Ok(());
        }
        let provider_error = data.get("error");
        let label = self.provider_name();
        if let Some(err) = provider_error {
            return Err(ImageGenerationError::new(format!(
                "{} returned no images: {}", label, err
            )));
        }
        Err(ImageGenerationError::new(format!(
            "{} returned no images for this request", label
        )))
    }

    async fn http_post(
        &self,
        client: &Client,
        url: &str,
        headers: HashMap<String, String>,
        body: &Value,
    ) -> Result<reqwest::Response, ImageGenerationError> {
        let mut req = client.post(url);
        for (k, v) in &headers {
            req = req.header(k, v);
        }
        req = req.json(body);
        req.send().await.map_err(|e| ImageGenerationError::RequestError(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// OpenRouter
// ---------------------------------------------------------------------------

pub struct OpenRouterImageGenerationClient {
    api_key: Option<String>,
    api_base: String,
    extra_headers: HashMap<String, String>,
    extra_body: Value,
    timeout: f64,
    client: Option<Client>,
}

impl OpenRouterImageGenerationClient {
    pub fn new(
        api_key: Option<String>,
        api_base: Option<String>,
        extra_headers: Option<HashMap<String, String>>,
        extra_body: Option<Value>,
        timeout: Option<f64>,
        client: Option<Client>,
    ) -> Self {
        let base = Self::default_base_url_static();
        Self {
            api_key,
            api_base: api_base
                .unwrap_or_else(|| base.clone())
                .trim_end_matches('/')
                .to_string(),
            extra_headers: extra_headers.unwrap_or_default(),
            extra_body: extra_body.unwrap_or(Value::Null),
            timeout: timeout.unwrap_or(_DEFAULT_TIMEOUT_S),
            client,
        }
    }

    fn default_base_url_static() -> String {
        "https://openrouter.ai/api/v1".to_string()
    }

    fn build_headers(&self) -> HashMap<String, String> {
        let mut headers = HashMap::new();
        if let Some(ref key) = self.api_key {
            headers.insert("Authorization".to_string(), format!("Bearer {}", key));
        }
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        for &(k, v) in _OPENROUTER_ATTRIBUTION_HEADERS {
            headers.insert(k.to_string(), v.to_string());
        }
        for (k, v) in &self.extra_headers {
            headers.insert(k.clone(), v.clone());
        }
        headers
    }
}

#[async_trait::async_trait]
impl ImageGenerationProvider for OpenRouterImageGenerationClient {
    fn provider_name(&self) -> &str {
        "openrouter"
    }

    fn missing_key_message(&self) -> &str {
        "OpenRouter API key is not configured. Set providers.openrouter.apiKey."
    }

    fn default_base_url(&self) -> String {
        Self::default_base_url_static()
    }

    async fn generate(
        &self,
        prompt: &str,
        model: &str,
        reference_images: Option<&[String]>,
        aspect_ratio: Option<&str>,
        image_size: Option<&str>,
    ) -> Result<GeneratedImageResponse, ImageGenerationError> {
        if self.api_key.is_none() {
            return Err(ImageGenerationError::new(self.missing_key_message()));
        }

        let content: Value = if let Some(refs) = reference_images {
            if refs.is_empty() {
                Value::String(prompt.to_string())
            } else {
                let mut blocks: Vec<Value> = vec![
                    serde_json::json!({"type": "text", "text": prompt}),
                ];
                for path in refs {
                    let data_url = image_path_to_data_url(Path::new(path))?;
                    blocks.push(serde_json::json!({
                        "type": "image_url",
                        "image_url": {"url": data_url},
                    }));
                }
                Value::Array(blocks)
            }
        } else {
            Value::String(prompt.to_string())
        };

        let mut body = serde_json::json!({
            "model": model,
            "messages": [{"role": "user", "content": content}],
            "modalities": ["image", "text"],
            "stream": false,
        });

        let mut image_config = serde_json::Map::new();
        if let Some(ar) = aspect_ratio {
            image_config.insert("aspect_ratio".to_string(), Value::String(ar.to_string()));
        }
        if let Some(size) = image_size {
            image_config.insert("image_size".to_string(), Value::String(size.to_string()));
        }
        if !image_config.is_empty() {
            body["image_config"] = Value::Object(image_config);
        }

        if self.extra_body.is_object() {
            if let (Some(body_obj), Some(extra_obj)) = (body.as_object_mut(), self.extra_body.as_object()) {
                for (k, v) in extra_obj {
                    body_obj.insert(k.clone(), v.clone());
                }
            }
        }

        let headers = self.build_headers();
        let url = format!("{}/chat/completions", self.api_base);

        let client = self.client.as_ref().cloned().unwrap_or_else(|| {
            Client::builder()
                .timeout(std::time::Duration::from_secs_f64(self.timeout))
                .build()
                .unwrap()
        });

        let response = self.http_post(&client, &url, headers, &body).await?;

        let status = response.status();
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            let preview: String = detail.chars().take(500).collect();
            return Err(ImageGenerationError::new(format!(
                "OpenRouter image generation failed: {}", preview
            )));
        }

        let data: Value = response.json().await.map_err(|e| ImageGenerationError::RequestError(e.to_string()))?;

        let mut images: Vec<String> = Vec::new();
        let mut text_parts: Vec<String> = Vec::new();

        if let Some(choices) = data.get("choices").and_then(|v| v.as_array()) {
            for choice in choices {
                if !choice.is_object() {
                    continue;
                }
                if let Some(message) = choice.get("message").and_then(|v| v.as_object()) {
                    if let Some(content_str) = message.get("content").and_then(|v| v.as_str()) {
                        text_parts.push(content_str.to_string());
                    }
                    if let Some(imgs) = message.get("images").and_then(|v| v.as_array()) {
                        for image in imgs {
                            if !image.is_object() {
                                continue;
                            }
                            let image_url = image.get("image_url")
                                .or_else(|| image.get("imageUrl"));
                            if let Some(obj) = image_url.and_then(|v| v.as_object()) {
                                if let Some(url_value) = obj.get("url").and_then(|v| v.as_str()) {
                                    if url_value.starts_with("data:image/") {
                                        images.push(url_value.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        self.require_images(&images, &data)?;

        let content_text = text_parts
            .iter()
            .filter(|p| !p.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string();

        Ok(GeneratedImageResponse {
            images,
            content: content_text,
            raw: data,
        })
    }
}

// ---------------------------------------------------------------------------
// AIHubMix
// ---------------------------------------------------------------------------

pub struct AIHubMixImageGenerationClient {
    api_key: Option<String>,
    api_base: String,
    extra_headers: HashMap<String, String>,
    extra_body: Value,
    timeout: f64,
    client: Option<Client>,
}

impl AIHubMixImageGenerationClient {
    pub fn new(
        api_key: Option<String>,
        api_base: Option<String>,
        extra_headers: Option<HashMap<String, String>>,
        extra_body: Option<Value>,
        timeout: Option<f64>,
        client: Option<Client>,
    ) -> Self {
        let base = Self::default_base_url_static();
        Self {
            api_key,
            api_base: api_base
                .unwrap_or_else(|| base.clone())
                .trim_end_matches('/')
                .to_string(),
            extra_headers: extra_headers.unwrap_or_default(),
            extra_body: extra_body.unwrap_or(Value::Null),
            timeout: timeout.unwrap_or(_AIHUBMIX_TIMEOUT_S),
            client,
        }
    }

    fn default_base_url_static() -> String {
        "https://aihubmix.com/v1".to_string()
    }
}

#[async_trait::async_trait]
impl ImageGenerationProvider for AIHubMixImageGenerationClient {
    fn provider_name(&self) -> &str {
        "aihubmix"
    }

    fn missing_key_message(&self) -> &str {
        "AIHubMix API key is not configured. Set providers.aihubmix.apiKey."
    }

    fn default_timeout(&self) -> f64 {
        _AIHUBMIX_TIMEOUT_S
    }

    fn default_base_url(&self) -> String {
        Self::default_base_url_static()
    }

    async fn generate(
        &self,
        prompt: &str,
        model: &str,
        reference_images: Option<&[String]>,
        aspect_ratio: Option<&str>,
        image_size: Option<&str>,
    ) -> Result<GeneratedImageResponse, ImageGenerationError> {
        if self.api_key.is_none() {
            return Err(ImageGenerationError::new(self.missing_key_message()));
        }

        let refs: Vec<String> = reference_images.unwrap_or(&[]).to_vec();
        let size = _aihubmix_size(aspect_ratio, image_size);

        let mut headers = HashMap::new();
        if let Some(ref key) = self.api_key {
            headers.insert("Authorization".to_string(), format!("Bearer {}", key));
        }
        for (k, v) in &self.extra_headers {
            headers.insert(k.clone(), v.clone());
        }

        let client = self.client.as_ref().cloned().unwrap_or_else(|| {
            Client::builder()
                .timeout(std::time::Duration::from_secs_f64(self.timeout))
                .build()
                .unwrap()
        });

        self._generate_with_client(
            &client,
            prompt,
            model,
            &refs,
            &size,
            &headers,
        )
        .await
    }
}

impl AIHubMixImageGenerationClient {
    async fn _generate_with_client(
        &self,
        client: &Client,
        prompt: &str,
        model: &str,
        reference_images: &[String],
        size: &str,
        headers: &HashMap<String, String>,
    ) -> Result<GeneratedImageResponse, ImageGenerationError> {
        let image_input: Option<Value> = if reference_images.is_empty() {
            None
        } else {
            let image_refs: Result<Vec<String>, ImageGenerationError> = reference_images
                .iter()
                .map(|p| image_path_to_data_url(Path::new(p)))
                .collect();
            let image_refs = image_refs?;
            if image_refs.len() == 1 {
                Some(Value::String(image_refs[0].clone()))
            } else {
                Some(Value::Array(image_refs.into_iter().map(Value::String).collect()))
            }
        };

        let mut input_body = serde_json::json!({
            "prompt": prompt,
            "n": 1,
            "size": size,
        });

        if let Some(img) = image_input {
            input_body["image"] = img;
        }

        if self.extra_body.is_object() {
            if let (Some(input_obj), Some(extra_obj)) = (input_body.as_object_mut(), self.extra_body.as_object()) {
                for (k, v) in extra_obj {
                    input_obj.insert(k.clone(), v.clone());
                }
            }
        }

        let body = serde_json::json!({"input": input_body});
        let model_path = _aihubmix_model_path(model);
        let url = format!("{}/models/{}/predictions", self.api_base, model_path);

        let mut req_headers = headers.clone();
        req_headers.insert("Content-Type".to_string(), "application/json".to_string());

        let response = self.http_post(client, &url, req_headers, &body).await?;

        let status = response.status();
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            let preview: String = detail.chars().take(500).collect();
            return Err(ImageGenerationError::new(format!(
                "AIHubMix image generation failed: {}", preview
            )));
        }

        let payload: Value = response.json().await.map_err(|e| ImageGenerationError::RequestError(e.to_string()))?;

        let images = _aihubmix_images_from_payload(client, &payload).await?;

        self.require_images(&images, &payload)?;

        Ok(GeneratedImageResponse {
            images,
            content: String::new(),
            raw: payload,
        })
    }
}

async fn _http_error_detail(mut response: reqwest::Response) -> String {
    let status = response.status();
    let body_bytes = response.bytes().await.unwrap_or_default();
    if let Ok(data) = serde_json::from_slice::<Value>(&body_bytes) {
        if let Some(obj) = data.as_object() {
            if let Some(err) = obj.get("error") {
                if let Some(err_obj) = err.as_object() {
                    if let Some(msg) = err_obj.get("message").and_then(|v| v.as_str()) {
                        return format!("HTTP {} - {}", status, msg);
                    }
                }
                return format!("HTTP {} - {}", status, err);
            }
        }
    }
    let text = String::from_utf8_lossy(&body_bytes);
    let preview: String = text.chars().take(500).collect();
    if preview.is_empty() {
        format!("HTTP {} - <empty response body>", status)
    } else {
        format!("HTTP {} - {}", status, preview)
    }
}

async fn _aihubmix_images_from_payload(
    client: &Client,
    payload: &Value,
) -> Result<Vec<String>, ImageGenerationError> {
    let mut images: Vec<String> = Vec::new();

    let candidates: Vec<&Value> = {
        let mut c = Vec::new();
        if payload.get("data").is_some() {
            c.push(payload.get("data").unwrap());
        }
        if payload.get("output").is_some() {
            c.push(payload.get("output").unwrap());
        }
        c
    };

    for candidate in candidates {
        _aihubmix_collect(candidate, client, &mut images).await?;
    }

    Ok(images)
}

async fn _aihubmix_collect(
    value: &Value,
    client: &Client,
    images: &mut Vec<String>,
) -> Result<(), ImageGenerationError> {
    if let Some(arr) = value.as_array() {
        for item in arr {
            Box::pin(_aihubmix_collect(item, client, images)).await?;
        }
        return Ok(());
    }

    if let Some(s) = value.as_str() {
        if s.starts_with("data:image/") {
            images.push(s.to_string());
        } else if s.starts_with("http://") || s.starts_with("https://") {
            let data_url = _download_image_data_url(client, s).await?;
            images.push(data_url);
        }
        return Ok(());
    }

    if !value.is_object() {
        return Ok(());
    }

    let obj = value.as_object().unwrap();

    if let Some(b64_json) = obj.get("b64_json").and_then(|v| v.as_str()) {
        if !b64_json.is_empty() {
            images.push(_b64_image_data_url(b64_json)?);
        }
    } else if let Some(b64_json) = obj.get("b64_json") {
        Box::pin(_aihubmix_collect(b64_json, client, images)).await?;
    }

    for key in &["bytesBase64", "bytes_base64", "base64"] {
        if let Some(bytes_base64) = obj.get(*key).and_then(|v| v.as_str()) {
            if !bytes_base64.is_empty() {
                images.push(_b64_image_data_url(bytes_base64)?);
            }
        }
    }

    if let Some(image_url) = obj.get("image_url").or_else(|| obj.get("imageUrl")) {
        if let Some(url_obj) = image_url.as_object() {
            if let Some(url) = url_obj.get("url") {
                Box::pin(_aihubmix_collect(url, client, images)).await?;
            }
        } else {
            Box::pin(_aihubmix_collect(image_url, client, images)).await?;
        }
    }

    if let Some(url_value) = obj.get("url") {
        Box::pin(_aihubmix_collect(url_value, client, images)).await?;
    }

    for key in &["images", "image", "output"] {
        if let Some(nested) = obj.get(*key) {
            Box::pin(_aihubmix_collect(nested, client, images)).await?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Gemini
// ---------------------------------------------------------------------------

static _GEMINI_IMAGEN_ASPECT_RATIOS: &[&str] = &["1:1", "9:16", "16:9", "3:4", "4:3"];

pub struct GeminiImageGenerationClient {
    api_key: Option<String>,
    api_base: String,
    extra_headers: HashMap<String, String>,
    extra_body: Value,
    timeout: f64,
    client: Option<Client>,
}

impl GeminiImageGenerationClient {
    pub fn new(
        api_key: Option<String>,
        api_base: Option<String>,
        extra_headers: Option<HashMap<String, String>>,
        extra_body: Option<Value>,
        timeout: Option<f64>,
        client: Option<Client>,
    ) -> Self {
        let base = Self::default_base_url_static();
        Self {
            api_key,
            api_base: api_base
                .unwrap_or_else(|| base.clone())
                .trim_end_matches('/')
                .to_string(),
            extra_headers: extra_headers.unwrap_or_default(),
            extra_body: extra_body.unwrap_or(Value::Null),
            timeout: timeout.unwrap_or(_GEMINI_DEFAULT_TIMEOUT_S),
            client,
        }
    }

    fn default_base_url_static() -> String {
        "https://generativelanguage.googleapis.com/v1beta".to_string()
    }
}

#[async_trait::async_trait]
impl ImageGenerationProvider for GeminiImageGenerationClient {
    fn provider_name(&self) -> &str {
        "gemini"
    }

    fn missing_key_message(&self) -> &str {
        "Gemini API key is not configured. Set providers.gemini.apiKey."
    }

    fn default_timeout(&self) -> f64 {
        _GEMINI_DEFAULT_TIMEOUT_S
    }

    fn default_base_url(&self) -> String {
        Self::default_base_url_static()
    }

    fn resolve_base_url(&self, api_base: Option<&str>) -> String {
        if let Some(base) = api_base {
            return base.trim_end_matches('/').to_string();
        }
        self.default_base_url()
    }

    async fn generate(
        &self,
        prompt: &str,
        model: &str,
        reference_images: Option<&[String]>,
        aspect_ratio: Option<&str>,
        image_size: Option<&str>,
    ) -> Result<GeneratedImageResponse, ImageGenerationError> {
        if self.api_key.is_none() {
            return Err(ImageGenerationError::new(self.missing_key_message()));
        }

        if model.to_lowercase().contains("imagen") {
            if let Some(refs) = reference_images {
                if !refs.is_empty() {
                    error!(
                        "Imagen models do not support reference images; ignoring {} reference image(s) for {}",
                        refs.len(),
                        model,
                    );
                }
            }
            return self._generate_imagen(prompt, model, aspect_ratio).await;
        }

        let refs: Vec<String> = reference_images.unwrap_or(&[]).to_vec();
        self._generate_gemini_flash(prompt, model, &refs).await
    }
}

impl GeminiImageGenerationClient {
    async fn _generate_imagen(
        &self,
        prompt: &str,
        model: &str,
        aspect_ratio: Option<&str>,
    ) -> Result<GeneratedImageResponse, ImageGenerationError> {
        let mut parameters = serde_json::json!({"sampleCount": 1});
        if let Some(ar) = aspect_ratio {
            if _GEMINI_IMAGEN_ASPECT_RATIOS.contains(&ar) {
                parameters["aspectRatio"] = Value::String(ar.to_string());
            }
        }

        let mut body = serde_json::json!({
            "instances": [{"prompt": prompt}],
            "parameters": parameters,
        });

        if self.extra_body.is_object() {
            if let (Some(body_obj), Some(extra_obj)) = (body.as_object_mut(), self.extra_body.as_object()) {
                for (k, v) in extra_obj {
                    body_obj.insert(k.clone(), v.clone());
                }
            }
        }

        let url = format!("{}/models/{}:predict", self.api_base, model);
        let mut headers = HashMap::new();
        headers.insert(
            "x-goog-api-key".to_string(),
            self.api_key.clone().unwrap_or_default(),
        );
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        for (k, v) in &self.extra_headers {
            headers.insert(k.clone(), v.clone());
        }

        let client = self.client.as_ref().cloned().unwrap_or_else(|| {
            Client::builder()
                .timeout(std::time::Duration::from_secs_f64(self.timeout))
                .build()
                .unwrap()
        });

        let response = self.http_post(&client, &url, headers, &body).await?;

        let status = response.status();
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            let preview: String = detail.chars().take(500).collect();
            error!(
                "Gemini Imagen generation failed (HTTP {}): {}", status, preview
            );
            return Err(ImageGenerationError::new(format!(
                "Gemini Imagen generation failed (HTTP {}): {}", status, preview
            )));
        }

        let data: Value = response.json().await.map_err(|e| ImageGenerationError::RequestError(e.to_string()))?;

        let mut images: Vec<String> = Vec::new();

        if let Some(predictions) = data.get("predictions").and_then(|v| v.as_array()) {
            for prediction in predictions {
                if !prediction.is_object() {
                    continue;
                }
                if let Some(b64) = prediction.get("bytesBase64Encoded").and_then(|v| v.as_str()) {
                    let mime = prediction
                        .get("mimeType")
                        .and_then(|v| v.as_str())
                        .unwrap_or("image/png");
                    images.push(format!("data:{};base64,{}", mime, b64));
                }
            }
        }

        self.require_images(&images, &data)?;

        Ok(GeneratedImageResponse {
            images,
            content: String::new(),
            raw: data,
        })
    }

    async fn _generate_gemini_flash(
        &self,
        prompt: &str,
        model: &str,
        reference_images: &[String],
    ) -> Result<GeneratedImageResponse, ImageGenerationError> {
        let mut parts: Vec<Value> = Vec::new();
        for path in reference_images {
            let inline = image_path_to_inline_data(Path::new(path))?;
            let mut inline_map = serde_json::Map::new();
            for (k, v) in inline {
                inline_map.insert(k, Value::String(v));
            }
            let mut part = serde_json::Map::new();
            part.insert("inlineData".to_string(), Value::Object(inline_map));
            parts.push(Value::Object(part));
        }
        parts.push(serde_json::json!({"text": prompt}));

        let mut body = serde_json::json!({
            "contents": [{"role": "user", "parts": parts}],
            "generationConfig": {"responseModalities": ["TEXT", "IMAGE"]},
        });

        if self.extra_body.is_object() {
            if let (Some(body_obj), Some(extra_obj)) = (body.as_object_mut(), self.extra_body.as_object()) {
                for (k, v) in extra_obj {
                    body_obj.insert(k.clone(), v.clone());
                }
            }
        }

        let url = format!("{}/models/{}:generateContent", self.api_base, model);
        let mut headers = HashMap::new();
        headers.insert(
            "x-goog-api-key".to_string(),
            self.api_key.clone().unwrap_or_default(),
        );
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        for (k, v) in &self.extra_headers {
            headers.insert(k.clone(), v.clone());
        }

        let client = self.client.as_ref().cloned().unwrap_or_else(|| {
            Client::builder()
                .timeout(std::time::Duration::from_secs_f64(self.timeout))
                .build()
                .unwrap()
        });

        let response = self.http_post(&client, &url, headers, &body).await?;

        let status = response.status();
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            let preview: String = detail.chars().take(500).collect();
            error!(
                "Gemini image generation failed (HTTP {}): {}", status, preview
            );
            return Err(ImageGenerationError::new(format!(
                "Gemini image generation failed (HTTP {}): {}", status, preview
            )));
        }

        let data: Value = response.json().await.map_err(|e| ImageGenerationError::RequestError(e.to_string()))?;

        let mut images: Vec<String> = Vec::new();
        let mut text_parts: Vec<String> = Vec::new();

        if let Some(candidates) = data.get("candidates").and_then(|v| v.as_array()) {
            for candidate in candidates {
                if !candidate.is_object() {
                    continue;
                }
                if let Some(content_obj) = candidate.get("content").and_then(|v| v.as_object()) {
                    if let Some(parts_arr) = content_obj.get("parts").and_then(|v| v.as_array()) {
                        for part in parts_arr {
                            if !part.is_object() {
                                continue;
                            }
                            if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                                text_parts.push(text.to_string());
                            }
                            if let Some(inline) = part.get("inlineData").and_then(|v| v.as_object()) {
                                let mime = inline
                                    .get("mimeType")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("image/png");
                                if let Some(b64) = inline.get("data").and_then(|v| v.as_str()) {
                                    if !b64.is_empty() {
                                        images.push(format!("data:{};base64,{}", mime, b64));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        self.require_images(&images, &data)?;

        let content_text = text_parts
            .iter()
            .filter(|t| !t.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string();

        Ok(GeneratedImageResponse {
            images,
            content: content_text,
            raw: data,
        })
    }
}

// ---------------------------------------------------------------------------
// MiniMax
// ---------------------------------------------------------------------------

static _MINIMAX_ASPECT_RATIO_SIZES: &[(&str, &str)] = &[
    ("1:1", "1:1"),
    ("16:9", "16:9"),
    ("4:3", "4:3"),
    ("3:2", "3:2"),
    ("2:3", "2:3"),
    ("3:4", "3:4"),
    ("9:16", "9:16"),
    ("21:9", "21:9"),
];

pub struct MiniMaxImageGenerationClient {
    api_key: Option<String>,
    api_base: String,
    extra_headers: HashMap<String, String>,
    extra_body: Value,
    timeout: f64,
    client: Option<Client>,
}

impl MiniMaxImageGenerationClient {
    pub fn new(
        api_key: Option<String>,
        api_base: Option<String>,
        extra_headers: Option<HashMap<String, String>>,
        extra_body: Option<Value>,
        timeout: Option<f64>,
        client: Option<Client>,
    ) -> Self {
        let base = Self::default_base_url_static();
        Self {
            api_key,
            api_base: api_base
                .unwrap_or_else(|| base.clone())
                .trim_end_matches('/')
                .to_string(),
            extra_headers: extra_headers.unwrap_or_default(),
            extra_body: extra_body.unwrap_or(Value::Null),
            timeout: timeout.unwrap_or(_MINIMAX_TIMEOUT_S),
            client,
        }
    }

    fn default_base_url_static() -> String {
        "https://api.minimaxi.com/v1".to_string()
    }

    fn resolve_aspect_ratio(&self, aspect_ratio: Option<&str>) -> String {
        if let Some(ar) = aspect_ratio {
            for &(k, v) in _MINIMAX_ASPECT_RATIO_SIZES {
                if k == ar {
                    return v.to_string();
                }
            }
        }
        "1:1".to_string()
    }
}

#[async_trait::async_trait]
impl ImageGenerationProvider for MiniMaxImageGenerationClient {
    fn provider_name(&self) -> &str {
        "minimax"
    }

    fn missing_key_message(&self) -> &str {
        "MiniMax API key is not configured. Set providers.minimax.apiKey."
    }

    fn default_timeout(&self) -> f64 {
        _MINIMAX_TIMEOUT_S
    }

    fn default_base_url(&self) -> String {
        Self::default_base_url_static()
    }

    async fn generate(
        &self,
        prompt: &str,
        model: &str,
        reference_images: Option<&[String]>,
        aspect_ratio: Option<&str>,
        image_size: Option<&str>,
    ) -> Result<GeneratedImageResponse, ImageGenerationError> {
        if self.api_key.is_none() {
            return Err(ImageGenerationError::new(self.missing_key_message()));
        }

        let mut headers = HashMap::new();
        if let Some(ref key) = self.api_key {
            headers.insert("Authorization".to_string(), format!("Bearer {}", key));
        }
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        for (k, v) in &self.extra_headers {
            headers.insert(k.clone(), v.clone());
        }

        let resolved_ratio = self.resolve_aspect_ratio(aspect_ratio);

        let mut body = serde_json::json!({
            "model": model,
            "prompt": prompt,
            "response_format": "base64",
            "aspect_ratio": resolved_ratio,
        });

        if let Some(refs) = reference_images {
            if !refs.is_empty() {
                let image_refs: Result<Vec<String>, ImageGenerationError> = refs
                    .iter()
                    .map(|p| image_path_to_data_url(Path::new(p)))
                    .collect();
                let image_refs = image_refs?;
                let subject_reference: Vec<Value> = image_refs
                    .into_iter()
                    .map(|ref_data| {
                        serde_json::json!({
                            "type": "character",
                            "image_file": ref_data,
                        })
                    })
                    .collect();
                body["subject_reference"] = Value::Array(subject_reference);
            }
        }

        if self.extra_body.is_object() {
            if let (Some(body_obj), Some(extra_obj)) = (body.as_object_mut(), self.extra_body.as_object()) {
                for (k, v) in extra_obj {
                    body_obj.insert(k.clone(), v.clone());
                }
            }
        }

        let client = self.client.as_ref().cloned().unwrap_or_else(|| {
            Client::builder()
                .timeout(std::time::Duration::from_secs_f64(self.timeout))
                .build()
                .unwrap()
        });

        self._generate_with_client(&client, &body, &headers).await
    }
}

impl MiniMaxImageGenerationClient {
    async fn _generate_with_client(
        &self,
        client: &Client,
        body: &Value,
        headers: &HashMap<String, String>,
    ) -> Result<GeneratedImageResponse, ImageGenerationError> {
        let url = format!("{}/image_generation", self.api_base);

        let response = self.http_post(client, &url, headers.clone(), body).await?;

        let status = response.status();
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            let preview: String = detail.chars().take(500).collect();
            return Err(ImageGenerationError::new(format!(
                "MiniMax image generation failed: {}", preview
            )));
        }

        let payload: Value = response.json().await.map_err(|e| ImageGenerationError::RequestError(e.to_string()))?;

        let images = _minimax_images_from_payload(&payload);

        self.require_images(&images, &payload)?;

        Ok(GeneratedImageResponse {
            images,
            content: String::new(),
            raw: payload,
        })
    }
}

fn _minimax_images_from_payload(payload: &Value) -> Vec<String> {
    let mut images: Vec<String> = Vec::new();
    let data = payload.get("data");
    if !data.map(|v| v.is_object()).unwrap_or(false) {
        return images;
    }
    if let Some(image_base64) = data.and_then(|v| v.get("image_base64")).and_then(|v| v.as_array()) {
        for b64 in image_base64 {
            if let Some(b64_str) = b64.as_str() {
                if !b64_str.is_empty() {
                    if let Ok(url) = _b64_image_data_url(b64_str) {
                        images.push(url);
                    }
                }
            }
        }
    }
    images
}

// ---------------------------------------------------------------------------
// StepFun
// ---------------------------------------------------------------------------

static _STEPFUN_ASPECT_RATIO_SIZES: &[(&str, &str)] = &[
    ("1:1", "1024x1024"),
    ("16:9", "1280x800"),
    ("9:16", "800x1280"),
    ("3:4", "768x1360"),
    ("4:3", "1360x768"),
];

fn _stepfun_size(aspect_ratio: Option<&str>, image_size: Option<&str>) -> String {
    if let Some(size) = image_size {
        if size.to_lowercase().contains('x') {
            return size.to_string();
        }
    }
    if let Some(ar) = aspect_ratio {
        for &(k, v) in _STEPFUN_ASPECT_RATIO_SIZES {
            if k == ar {
                return v.to_string();
            }
        }
    }
    "1024x1024".to_string()
}

fn _stepfun_images_from_payload(payload: &Value) -> Vec<String> {
    let mut images: Vec<String> = Vec::new();
    if let Some(data_arr) = payload.get("data").and_then(|v| v.as_array()) {
        for item in data_arr {
            if !item.is_object() {
                continue;
            }
            if let Some(b64) = item.get("b64_json").and_then(|v| v.as_str()) {
                if !b64.is_empty() {
                    if let Ok(url) = _b64_image_data_url(b64) {
                        images.push(url);
                    }
                }
            }
        }
    }
    images
}

pub struct StepFunImageGenerationClient {
    api_key: Option<String>,
    api_base: String,
    extra_headers: HashMap<String, String>,
    extra_body: Value,
    timeout: f64,
    client: Option<Client>,
}

impl StepFunImageGenerationClient {
    pub fn new(
        api_key: Option<String>,
        api_base: Option<String>,
        extra_headers: Option<HashMap<String, String>>,
        extra_body: Option<Value>,
        timeout: Option<f64>,
        client: Option<Client>,
    ) -> Self {
        let base = Self::default_base_url_static();
        Self {
            api_key,
            api_base: api_base
                .unwrap_or_else(|| base.clone())
                .trim_end_matches('/')
                .to_string(),
            extra_headers: extra_headers.unwrap_or_default(),
            extra_body: extra_body.unwrap_or(Value::Null),
            timeout: timeout.unwrap_or(_STEPFUN_TIMEOUT_S),
            client,
        }
    }

    fn default_base_url_static() -> String {
        "https://api.stepfun.com/v1".to_string()
    }
}

#[async_trait::async_trait]
impl ImageGenerationProvider for StepFunImageGenerationClient {
    fn provider_name(&self) -> &str {
        "stepfun"
    }

    fn missing_key_message(&self) -> &str {
        "StepFun API key is not configured. Set providers.stepfun.apiKey."
    }

    fn default_timeout(&self) -> f64 {
        _STEPFUN_TIMEOUT_S
    }

    fn default_base_url(&self) -> String {
        Self::default_base_url_static()
    }

    async fn generate(
        &self,
        prompt: &str,
        model: &str,
        reference_images: Option<&[String]>,
        aspect_ratio: Option<&str>,
        image_size: Option<&str>,
    ) -> Result<GeneratedImageResponse, ImageGenerationError> {
        if self.api_key.is_none() {
            return Err(ImageGenerationError::new(self.missing_key_message()));
        }

        let mut headers = HashMap::new();
        if let Some(ref key) = self.api_key {
            headers.insert("Authorization".to_string(), format!("Bearer {}", key));
        }
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        for (k, v) in &self.extra_headers {
            headers.insert(k.clone(), v.clone());
        }

        let mut body = serde_json::json!({
            "model": model,
            "prompt": prompt,
            "response_format": "b64_json",
            "n": 1,
        });

        let size = _stepfun_size(aspect_ratio, image_size);
        if !size.is_empty() {
            body["size"] = Value::String(size);
        }

        if let Some(refs) = reference_images {
            if !refs.is_empty() && model.contains("1x") {
                let data_url = image_path_to_data_url(Path::new(&refs[0]))?;
                body["style_reference"] = serde_json::json!({
                    "source_url": data_url,
                });
            }
        }

        if self.extra_body.is_object() {
            if let (Some(body_obj), Some(extra_obj)) = (body.as_object_mut(), self.extra_body.as_object()) {
                for (k, v) in extra_obj {
                    body_obj.insert(k.clone(), v.clone());
                }
            }
        }

        let client = self.client.as_ref().cloned().unwrap_or_else(|| {
            Client::builder()
                .timeout(std::time::Duration::from_secs_f64(self.timeout))
                .build()
                .unwrap()
        });

        let url = format!("{}/images/generations", self.api_base);
        let response = self.http_post(&client, &url, headers, &body).await?;

        let status = response.status();
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            let preview: String = detail.chars().take(500).collect();
            return Err(ImageGenerationError::new(format!(
                "StepFun image generation failed: {}", preview
            )));
        }

        let payload: Value = response.json().await.map_err(|e| ImageGenerationError::RequestError(e.to_string()))?;

        let images = _stepfun_images_from_payload(&payload);

        self.require_images(&images, &payload)?;

        Ok(GeneratedImageResponse {
            images,
            content: String::new(),
            raw: payload,
        })
    }
}

// ---------------------------------------------------------------------------
// Provider registration (auto-run)
// ---------------------------------------------------------------------------

pub fn register_all_image_gen_providers() {
    register_image_gen_provider("openrouter", |api_key, api_base, extra_headers, extra_body, timeout, client| {
        Box::new(OpenRouterImageGenerationClient::new(
            api_key, api_base, extra_headers, extra_body, timeout, client,
        ))
    });
    register_image_gen_provider("aihubmix", |api_key, api_base, extra_headers, extra_body, timeout, client| {
        Box::new(AIHubMixImageGenerationClient::new(
            api_key, api_base, extra_headers, extra_body, timeout, client,
        ))
    });
    register_image_gen_provider("gemini", |api_key, api_base, extra_headers, extra_body, timeout, client| {
        Box::new(GeminiImageGenerationClient::new(
            api_key, api_base, extra_headers, extra_body, timeout, client,
        ))
    });
    register_image_gen_provider("minimax", |api_key, api_base, extra_headers, extra_body, timeout, client| {
        Box::new(MiniMaxImageGenerationClient::new(
            api_key, api_base, extra_headers, extra_body, timeout, client,
        ))
    });
    register_image_gen_provider("stepfun", |api_key, api_base, extra_headers, extra_body, timeout, client| {
        Box::new(StepFunImageGenerationClient::new(
            api_key, api_base, extra_headers, extra_body, timeout, client,
        ))
    });
}
