use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::base::{Tool, ToolExecError};
use super::context::{ContextAware, RequestContext};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ImageGenerationToolConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_aspect_ratio")]
    pub default_aspect_ratio: String,
    #[serde(default = "default_image_size")]
    pub default_image_size: String,
    #[serde(default = "default_max_images")]
    pub max_images_per_turn: u32,
    #[serde(default = "default_save_dir")]
    pub save_dir: String,
}

fn default_provider() -> String {
    "openrouter".into()
}
fn default_model() -> String {
    "openai/gpt-5.4-image-2".into()
}
fn default_aspect_ratio() -> String {
    "1:1".into()
}
fn default_image_size() -> String {
    "1K".into()
}
fn default_max_images() -> u32 {
    4
}
fn default_save_dir() -> String {
    "generated".into()
}

impl Default for ImageGenerationToolConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: default_provider(),
            model: default_model(),
            default_aspect_ratio: default_aspect_ratio(),
            default_image_size: default_image_size(),
            max_images_per_turn: default_max_images(),
            save_dir: default_save_dir(),
        }
    }
}

pub struct ImageGenerationProviderConfig {
    pub api_key: Option<String>,
    pub api_base: Option<String>,
    pub extra_headers: Option<HashMap<String, String>>,
    pub extra_body: Option<HashMap<String, Value>>,
}

pub struct ImageGenerationResponse {
    pub images: Vec<String>,
}

pub struct ImageGenerationError {
    pub message: String,
}

impl std::fmt::Display for ImageGenerationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::fmt::Debug for ImageGenerationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ImageGenerationError: {}", self.message)
    }
}

impl std::error::Error for ImageGenerationError {}

pub type ProviderConfig = ImageGenerationProviderConfig;

pub type GenerateFuture<'a> = Pin<Box<dyn Future<Output = Result<ImageGenerationResponse, ImageGenerationError>> + Send + 'a>>;

pub trait ImageGenerationProvider: Send + Sync {
    fn missing_key_message() -> Option<&'static str>
    where
        Self: Sized;

    fn generate<'a>(
        &'a self,
        prompt: String,
        model: String,
        reference_images: Vec<String>,
        aspect_ratio: String,
        image_size: String,
    ) -> GenerateFuture<'a>;
}

type ImageGenerationProviderCtor = fn(&ImageGenerationProviderConfig) -> Box<dyn ImageGenerationProvider>;

pub fn get_image_gen_provider(_provider: &str) -> Option<ImageGenerationProviderCtor> {
    todo!("TODO: implement provider registry")
}

pub fn store_generated_image_artifact(
    _image_data_url: String,
    _prompt: String,
    _model: String,
    _source_images: Vec<String>,
    _save_dir: String,
    _provider: String,
) -> Result<HashMap<String, Value>, ArtifactError> {
    todo!("TODO: implement artifact storage")
}

pub struct ArtifactError {
    pub message: String,
}

impl std::fmt::Display for ArtifactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::fmt::Debug for ArtifactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ArtifactError: {}", self.message)
    }
}

impl std::error::Error for ArtifactError {}

pub fn generated_image_tool_result(artifacts: &[HashMap<String, Value>]) -> String {
    json!(artifacts).to_string()
}

pub fn detect_image_mime(_bytes: &[u8]) -> Option<String> {
    todo!("TODO: implement image MIME detection")
}

pub fn get_media_dir() -> PathBuf {
    todo!("TODO: implement media dir resolution")
}

fn is_relative_to(path: &PathBuf, root: &PathBuf) -> bool {
    path.starts_with(root)
}

pub struct ImageGenerationTool {
    workspace: PathBuf,
    config: ImageGenerationToolConfig,
    provider_configs: HashMap<String, ProviderConfig>,
    request_ctx: Option<RequestContext>,
}

impl ImageGenerationTool {
    pub fn new(
        workspace: PathBuf,
        config: ImageGenerationToolConfig,
        provider_configs: HashMap<String, ProviderConfig>,
    ) -> Self {
        Self {
            workspace,
            config,
            provider_configs,
            request_ctx: None,
        }
    }

    pub fn config_cls() -> impl Fn() -> ImageGenerationToolConfig {
        || ImageGenerationToolConfig::default()
    }

    pub fn enabled(_config: &ImageGenerationToolConfig) -> bool {
        _config.enabled
    }

    pub fn create(
        ctx: &super::context::ToolContext,
    ) -> Self {
        Self {
            workspace: PathBuf::from(&ctx.workspace),
            config: ImageGenerationToolConfig::default(),
            provider_configs: HashMap::new(),
            request_ctx: None,
        }
    }

    fn provider_config(&self) -> Option<&ProviderConfig> {
        self.provider_configs.get(&self.config.provider)
    }

    fn provider_client(&self) -> Option<Box<dyn ImageGenerationProvider>> {
        let provider = self.provider_config();
        let ctor = get_image_gen_provider(&self.config.provider)?;
        let cfg = ImageGenerationProviderConfig {
            api_key: provider.map(|p| p.api_key.clone()).flatten(),
            api_base: provider.map(|p| p.api_base.clone()).flatten(),
            extra_headers: provider.and_then(|p| p.extra_headers.clone()),
            extra_body: provider.and_then(|p| p.extra_body.clone()),
        };
        Some(ctor(&cfg))
    }

    fn missing_api_key_error(&self) -> String {
        if let Some(ctor) = get_image_gen_provider(&self.config.provider) {
            todo!("TODO: implement missing key message retrieval")
        }
        format!(
            "Error: {} API key is not configured.",
            self.config.provider
        )
    }

    fn resolve_reference_image(&self, value: &str) -> Result<String, ImageGenerationError> {
        let raw_path = PathBuf::from(value);
        let path = if raw_path.is_absolute() {
            raw_path
        } else {
            self.workspace.join(&raw_path)
        };

        let resolved = path.canonicalize().map_err(|e| {
            ImageGenerationError {
                message: format!("reference image not found: {}: {}", value, e),
            }
        })?;

        let workspace_resolved = self.workspace.canonicalize().unwrap_or_else(|_| self.workspace.clone());
        let media_dir = get_media_dir();
        let media_resolved = media_dir.canonicalize().unwrap_or(media_dir);

        let is_relative_to_workspace = is_relative_to(&resolved, &workspace_resolved);
        let is_relative_to_media = is_relative_to(&resolved, &media_resolved);

        if !is_relative_to_workspace && !is_relative_to_media {
            return Err(ImageGenerationError {
                message: "reference_images must be inside the workspace or nanobot media directory".into(),
            });
        }

        if !resolved.is_file() {
            return Err(ImageGenerationError {
                message: format!("reference image is not a file: {}", value),
            });
        }

        let bytes = std::fs::read(&resolved).map_err(|e| {
            ImageGenerationError {
                message: format!("failed to read reference image: {}", e),
            }
        })?;

        if detect_image_mime(&bytes).is_none() {
            return Err(ImageGenerationError {
                message: format!("unsupported reference image: {}", value),
            });
        }

        Ok(resolved.to_string_lossy().to_string())
    }

    fn resolve_reference_images(&self, values: Option<&Vec<String>>) -> Vec<String> {
        match values {
            None => vec![],
            Some(vals) => vals
                .iter()
                .filter(|v| !v.is_empty())
                .filter_map(|v| self.resolve_reference_image(v).ok())
                .collect(),
        }
    }
}

impl ContextAware for ImageGenerationTool {
    fn set_context(&mut self, ctx: &RequestContext) {
        self.request_ctx = Some(ctx.clone());
    }
}

#[async_trait]
impl Tool for ImageGenerationTool {
    fn name(&self) -> &str {
        "generate_image"
    }

    fn description(&self) -> String {
        "Generate or edit images and store them as persistent artifacts. Returns artifact ids and local paths. For edits, pass prior generated image paths or user image paths as reference_images.".into()
    }

    fn parameters(&self) -> Value {
        let mut props = serde_json::Map::new();

        props.insert(
            "prompt".into(),
            json!({
                "type": "string",
                "minLength": 1,
                "description": "Detailed image generation or edit prompt. Include style, subject, composition, colors, and constraints."
            }),
        );

        props.insert(
            "reference_images".into(),
            json!({
                "type": "array",
                "items": {
                    "type": "string",
                    "description": "Local path of an existing image artifact or user-provided image to use as an edit reference."
                },
                "description": "Optional local image paths. Use generated artifact paths for iterative edits."
            }),
        );

        props.insert(
            "aspect_ratio".into(),
            json!({
                "type": "string",
                "description": "Optional output aspect ratio, e.g. 1:1, 16:9, 9:16, 4:3."
            }),
        );

        props.insert(
            "image_size".into(),
            json!({
                "type": "string",
                "description": "Optional output size hint supported by the configured provider, e.g. 1K, 2K, 4K, or 1024x1024."
            }),
        );

        props.insert(
            "count".into(),
            json!({
                "type": "integer",
                "description": "Number of images to generate in this turn.",
                "minimum": 1,
                "maximum": 8
            }),
        );

        json!({
            "type": "object",
            "properties": props,
            "required": ["prompt"]
        })
    }

    fn config_key(&self) -> &str {
        "image_generation"
    }

    async fn execute(&self, params: Value) -> Result<Value, ToolExecError> {
        let prompt = params
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolExecError::InvalidParams("missing required parameter: prompt".into()))?
            .to_string();

        let reference_images = params
            .get("reference_images")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect());

        let aspect_ratio = params
            .get("aspect_ratio")
            .and_then(|v| v.as_str())
            .map(String::from);

        let image_size = params
            .get("image_size")
            .and_then(|v| v.as_str())
            .map(String::from);

        let count = params
            .get("count")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32);

        let client = self.provider_client();
        let client = match client {
            Some(c) => c,
            None => {
                return Ok(Value::String(format!(
                    "Error: unsupported image generation provider '{}'",
                    self.config.provider
                )));
            }
        };

        let provider = self.provider_config();
        if provider.is_none() || provider.map(|p| p.api_key.is_none()).unwrap_or(true) {
            return Ok(Value::String(self.missing_api_key_error()));
        }

        let requested = count.unwrap_or(1);
        if requested > self.config.max_images_per_turn {
            return Ok(Value::String(format!(
                "Error: count exceeds tools.imageGeneration.maxImagesPerTurn ({})",
                self.config.max_images_per_turn
            )));
        }

        let refs = self.resolve_reference_images(reference_images.as_ref());

        let mut artifacts: Vec<HashMap<String, Value>> = Vec::new();
        let model = self.config.model.clone();
        let ar = aspect_ratio.unwrap_or_else(|| self.config.default_aspect_ratio.clone());
        let is = image_size.unwrap_or_else(|| self.config.default_image_size.clone());
        let save_dir = self.config.save_dir.clone();
        let prov = self.config.provider.clone();

        while artifacts.len() < requested as usize {
            let response = client
                .generate(
                    prompt.clone(),
                    model.clone(),
                    refs.clone(),
                    ar.clone(),
                    is.clone(),
                )
                .await
                .map_err(|e| ToolExecError::Other(e.to_string()))?;

            for image_data_url in response.images {
                let artifact = store_generated_image_artifact(
                    image_data_url,
                    prompt.clone(),
                    model.clone(),
                    refs.clone(),
                    save_dir.clone(),
                    prov.clone(),
                )
                .map_err(|e| ToolExecError::Other(e.to_string()))?;
                artifacts.push(artifact);
                if artifacts.len() >= requested as usize {
                    break;
                }
            }
        }

        Ok(Value::String(generated_image_tool_result(&artifacts)))
    }
}
