//! Provider registry — single source of truth for LLM provider metadata.
//!
//! Port of `nanobot.providers.registry`. Order matters; it controls match
//! priority and fallback (gateways first so they win when model keywords
//! overlap).

use utils::to_snake;

/// Backend implementation to use for a [`ProviderSpec`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Backend {
    OpenAICompat,
    Anthropic,
    AzureOpenAI,
    OpenAICodex,
    GitHubCopilot,
}

impl Backend {
    pub fn as_str(&self) -> &'static str {
        match self {
            Backend::OpenAICompat => "openai_compat",
            Backend::Anthropic => "anthropic",
            Backend::AzureOpenAI => "azure_openai",
            Backend::OpenAICodex => "openai_codex",
            Backend::GitHubCopilot => "github_copilot",
        }
    }
}

/// One LLM provider's metadata. See [`PROVIDERS`] for the real table.
#[derive(Debug, Clone)]
pub struct ProviderSpec {
    /// Config field name, e.g. `"dashscope"`.
    pub name: &'static str,
    /// Model-name keywords for matching (lowercase).
    pub keywords: &'static [&'static str],
    /// Env var for API key, e.g. `"DASHSCOPE_API_KEY"`.
    pub env_key: &'static str,
    /// Shown in `nanobot status`.
    pub display_name: &'static str,

    pub backend: Backend,

    /// Extra env vars. Values may contain `{api_key}` / `{api_base}` placeholders.
    pub env_extras: &'static [(&'static str, &'static str)],

    /// Routes any model (OpenRouter, AiHubMix, ...).
    pub is_gateway: bool,
    /// Local deployment (vLLM, Ollama, ...).
    pub is_local: bool,
    /// Match `api_key` prefix, e.g. `"sk-or-"`.
    pub detect_by_key_prefix: &'static str,
    /// Match substring in `api_base` URL.
    pub detect_by_base_keyword: &'static str,
    /// OpenAI-compatible base URL for this provider.
    pub default_api_base: &'static str,

    /// Strip `"provider/"` prefix before sending to gateway.
    pub strip_model_prefix: bool,
    pub supports_max_completion_tokens: bool,

    /// Per-model param overrides, e.g. `(("kimi-k2.5", &[("temperature", "1.0")]), ...)`.
    pub model_overrides: &'static [(&'static str, &'static [(&'static str, &'static str)])],

    /// OAuth-based providers (e.g. OpenAI Codex) don't use API keys.
    pub is_oauth: bool,
    /// Direct providers skip API-key validation (user supplies everything).
    pub is_direct: bool,
    /// Provider supports `cache_control` on content blocks.
    pub supports_prompt_caching: bool,
}

impl ProviderSpec {
    pub fn label(&self) -> &'static str {
        if !self.display_name.is_empty() {
            self.display_name
        } else {
            self.name
        }
    }
}

/// Canonical provider table. Order = priority.
pub const PROVIDERS: &[ProviderSpec] = &[
    ProviderSpec {
        name: "custom",
        keywords: &[],
        env_key: "",
        display_name: "Custom",
        backend: Backend::OpenAICompat,
        env_extras: &[],
        is_gateway: false,
        is_local: false,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        default_api_base: "",
        strip_model_prefix: false,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: false,
        is_direct: true,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "azure_openai",
        keywords: &["azure", "azure-openai"],
        env_key: "",
        display_name: "Azure OpenAI",
        backend: Backend::AzureOpenAI,
        env_extras: &[],
        is_gateway: false,
        is_local: false,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        default_api_base: "",
        strip_model_prefix: false,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: false,
        is_direct: true,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "openrouter",
        keywords: &["openrouter"],
        env_key: "OPENROUTER_API_KEY",
        display_name: "OpenRouter",
        backend: Backend::OpenAICompat,
        env_extras: &[],
        is_gateway: true,
        is_local: false,
        detect_by_key_prefix: "sk-or-",
        detect_by_base_keyword: "openrouter",
        default_api_base: "https://openrouter.ai/api/v1",
        strip_model_prefix: false,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: false,
        is_direct: false,
        supports_prompt_caching: true,
    },
    ProviderSpec {
        name: "aihubmix",
        keywords: &["aihubmix"],
        env_key: "OPENAI_API_KEY",
        display_name: "AiHubMix",
        backend: Backend::OpenAICompat,
        env_extras: &[],
        is_gateway: true,
        is_local: false,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "aihubmix",
        default_api_base: "https://aihubmix.com/v1",
        strip_model_prefix: true,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: false,
        is_direct: false,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "siliconflow",
        keywords: &["siliconflow"],
        env_key: "OPENAI_API_KEY",
        display_name: "SiliconFlow",
        backend: Backend::OpenAICompat,
        env_extras: &[],
        is_gateway: true,
        is_local: false,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "siliconflow",
        default_api_base: "https://api.siliconflow.cn/v1",
        strip_model_prefix: false,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: false,
        is_direct: false,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "volcengine",
        keywords: &["volcengine", "volces", "ark"],
        env_key: "OPENAI_API_KEY",
        display_name: "VolcEngine",
        backend: Backend::OpenAICompat,
        env_extras: &[],
        is_gateway: true,
        is_local: false,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "volces",
        default_api_base: "https://ark.cn-beijing.volces.com/api/v3",
        strip_model_prefix: false,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: false,
        is_direct: false,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "volcengine_coding_plan",
        keywords: &["volcengine-plan"],
        env_key: "OPENAI_API_KEY",
        display_name: "VolcEngine Coding Plan",
        backend: Backend::OpenAICompat,
        env_extras: &[],
        is_gateway: true,
        is_local: false,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        default_api_base: "https://ark.cn-beijing.volces.com/api/coding/v3",
        strip_model_prefix: true,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: false,
        is_direct: false,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "byteplus",
        keywords: &["byteplus"],
        env_key: "OPENAI_API_KEY",
        display_name: "BytePlus",
        backend: Backend::OpenAICompat,
        env_extras: &[],
        is_gateway: true,
        is_local: false,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "bytepluses",
        default_api_base: "https://ark.ap-southeast.bytepluses.com/api/v3",
        strip_model_prefix: true,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: false,
        is_direct: false,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "byteplus_coding_plan",
        keywords: &["byteplus-plan"],
        env_key: "OPENAI_API_KEY",
        display_name: "BytePlus Coding Plan",
        backend: Backend::OpenAICompat,
        env_extras: &[],
        is_gateway: true,
        is_local: false,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        default_api_base: "https://ark.ap-southeast.bytepluses.com/api/coding/v3",
        strip_model_prefix: true,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: false,
        is_direct: false,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "anthropic",
        keywords: &["anthropic", "claude"],
        env_key: "ANTHROPIC_API_KEY",
        display_name: "Anthropic",
        backend: Backend::Anthropic,
        env_extras: &[],
        is_gateway: false,
        is_local: false,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        default_api_base: "",
        strip_model_prefix: false,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: false,
        is_direct: false,
        supports_prompt_caching: true,
    },
    ProviderSpec {
        name: "openai",
        keywords: &["openai", "gpt"],
        env_key: "OPENAI_API_KEY",
        display_name: "OpenAI",
        backend: Backend::OpenAICompat,
        env_extras: &[],
        is_gateway: false,
        is_local: false,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        default_api_base: "",
        strip_model_prefix: false,
        supports_max_completion_tokens: true,
        model_overrides: &[],
        is_oauth: false,
        is_direct: false,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "openai_codex",
        keywords: &["openai-codex"],
        env_key: "",
        display_name: "OpenAI Codex",
        backend: Backend::OpenAICodex,
        env_extras: &[],
        is_gateway: false,
        is_local: false,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "codex",
        default_api_base: "https://chatgpt.com/backend-api",
        strip_model_prefix: false,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: true,
        is_direct: false,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "github_copilot",
        keywords: &["github_copilot", "copilot"],
        env_key: "",
        display_name: "Github Copilot",
        backend: Backend::GitHubCopilot,
        env_extras: &[],
        is_gateway: false,
        is_local: false,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        default_api_base: "https://api.githubcopilot.com",
        strip_model_prefix: true,
        supports_max_completion_tokens: true,
        model_overrides: &[],
        is_oauth: true,
        is_direct: false,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "deepseek",
        keywords: &["deepseek"],
        env_key: "DEEPSEEK_API_KEY",
        display_name: "DeepSeek",
        backend: Backend::OpenAICompat,
        env_extras: &[],
        is_gateway: false,
        is_local: false,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        default_api_base: "https://api.deepseek.com",
        strip_model_prefix: false,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: false,
        is_direct: false,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "gemini",
        keywords: &["gemini"],
        env_key: "GEMINI_API_KEY",
        display_name: "Gemini",
        backend: Backend::OpenAICompat,
        env_extras: &[],
        is_gateway: false,
        is_local: false,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        default_api_base: "https://generativelanguage.googleapis.com/v1beta/openai/",
        strip_model_prefix: false,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: false,
        is_direct: false,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "zhipu",
        keywords: &["zhipu", "glm", "zai"],
        env_key: "ZAI_API_KEY",
        display_name: "Zhipu AI",
        backend: Backend::OpenAICompat,
        env_extras: &[("ZHIPUAI_API_KEY", "{api_key}")],
        is_gateway: false,
        is_local: false,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        default_api_base: "https://open.bigmodel.cn/api/paas/v4",
        strip_model_prefix: false,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: false,
        is_direct: false,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "dashscope",
        keywords: &["qwen", "dashscope"],
        env_key: "DASHSCOPE_API_KEY",
        display_name: "DashScope",
        backend: Backend::OpenAICompat,
        env_extras: &[],
        is_gateway: false,
        is_local: false,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        default_api_base: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        strip_model_prefix: false,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: false,
        is_direct: false,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "moonshot",
        keywords: &["moonshot", "kimi"],
        env_key: "MOONSHOT_API_KEY",
        display_name: "Moonshot",
        backend: Backend::OpenAICompat,
        env_extras: &[],
        is_gateway: false,
        is_local: false,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        default_api_base: "https://api.moonshot.ai/v1",
        strip_model_prefix: false,
        supports_max_completion_tokens: false,
        model_overrides: &[
            ("kimi-k2.5", &[("temperature", "1.0")]),
            ("kimi-k2.6", &[("temperature", "1.0")]),
        ],
        is_oauth: false,
        is_direct: false,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "minimax",
        keywords: &["minimax"],
        env_key: "MINIMAX_API_KEY",
        display_name: "MiniMax",
        backend: Backend::OpenAICompat,
        env_extras: &[],
        is_gateway: false,
        is_local: false,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        default_api_base: "https://api.minimax.io/v1",
        strip_model_prefix: false,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: false,
        is_direct: false,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "minimax_anthropic",
        keywords: &["minimax_anthropic"],
        env_key: "MINIMAX_API_KEY",
        display_name: "MiniMax (Anthropic)",
        backend: Backend::Anthropic,
        env_extras: &[],
        is_gateway: false,
        is_local: false,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        default_api_base: "https://api.minimax.io/anthropic",
        strip_model_prefix: false,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: false,
        is_direct: false,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "mistral",
        keywords: &["mistral"],
        env_key: "MISTRAL_API_KEY",
        display_name: "Mistral",
        backend: Backend::OpenAICompat,
        env_extras: &[],
        is_gateway: false,
        is_local: false,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        default_api_base: "https://api.mistral.ai/v1",
        strip_model_prefix: false,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: false,
        is_direct: false,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "stepfun",
        keywords: &["stepfun", "step"],
        env_key: "STEPFUN_API_KEY",
        display_name: "Step Fun",
        backend: Backend::OpenAICompat,
        env_extras: &[],
        is_gateway: false,
        is_local: false,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        default_api_base: "https://api.stepfun.com/v1",
        strip_model_prefix: false,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: false,
        is_direct: false,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "xiaomi_mimo",
        keywords: &["xiaomi_mimo", "mimo"],
        env_key: "XIAOMIMIMO_API_KEY",
        display_name: "Xiaomi MIMO",
        backend: Backend::OpenAICompat,
        env_extras: &[],
        is_gateway: false,
        is_local: false,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        default_api_base: "https://api.xiaomimimo.com/v1",
        strip_model_prefix: false,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: false,
        is_direct: false,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "vllm",
        keywords: &["vllm"],
        env_key: "HOSTED_VLLM_API_KEY",
        display_name: "vLLM/Local",
        backend: Backend::OpenAICompat,
        env_extras: &[],
        is_gateway: false,
        is_local: true,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        default_api_base: "",
        strip_model_prefix: false,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: false,
        is_direct: false,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "ollama",
        keywords: &["ollama", "nemotron"],
        env_key: "OLLAMA_API_KEY",
        display_name: "Ollama",
        backend: Backend::OpenAICompat,
        env_extras: &[],
        is_gateway: false,
        is_local: true,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "11434",
        default_api_base: "http://localhost:11434/v1",
        strip_model_prefix: false,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: false,
        is_direct: false,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "lm_studio",
        keywords: &["lm-studio", "lmstudio", "lm_studio"],
        env_key: "LM_STUDIO_API_KEY",
        display_name: "LM Studio",
        backend: Backend::OpenAICompat,
        env_extras: &[],
        is_gateway: false,
        is_local: true,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "1234",
        default_api_base: "http://localhost:1234/v1",
        strip_model_prefix: false,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: false,
        is_direct: false,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "ovms",
        keywords: &["openvino", "ovms"],
        env_key: "",
        display_name: "OpenVINO Model Server",
        backend: Backend::OpenAICompat,
        env_extras: &[],
        is_gateway: false,
        is_local: true,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        default_api_base: "http://localhost:8000/v3",
        strip_model_prefix: false,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: false,
        is_direct: true,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "groq",
        keywords: &["groq"],
        env_key: "GROQ_API_KEY",
        display_name: "Groq",
        backend: Backend::OpenAICompat,
        env_extras: &[],
        is_gateway: false,
        is_local: false,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        default_api_base: "https://api.groq.com/openai/v1",
        strip_model_prefix: false,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: false,
        is_direct: false,
        supports_prompt_caching: false,
    },
    ProviderSpec {
        name: "qianfan",
        keywords: &["qianfan", "ernie"],
        env_key: "QIANFAN_API_KEY",
        display_name: "Qianfan",
        backend: Backend::OpenAICompat,
        env_extras: &[],
        is_gateway: false,
        is_local: false,
        detect_by_key_prefix: "",
        detect_by_base_keyword: "",
        default_api_base: "https://qianfan.baidubce.com/v2",
        strip_model_prefix: false,
        supports_max_completion_tokens: false,
        model_overrides: &[],
        is_oauth: false,
        is_direct: false,
        supports_prompt_caching: false,
    },
];

/// Find a provider spec by config field name, e.g. `"dashscope"`.
///
/// Accepts hyphenated or CamelCase forms — they're normalised to
/// `snake_case` before comparison, mirroring Python's
/// `pydantic.alias_generators.to_snake`.
pub fn find_by_name(name: &str) -> Option<&'static ProviderSpec> {
    let normalized = to_snake(&name.replace('-', "_"));
    PROVIDERS.iter().find(|p| p.name == normalized)
}

/// Find the first provider whose `keywords` match *model_name*
/// (case-insensitive substring).
pub fn find_by_model(model_name: &str) -> Option<&'static ProviderSpec> {
    let name = model_name.to_ascii_lowercase();
    PROVIDERS.iter().find(|p| {
        p.keywords
            .iter()
            .any(|kw| !kw.is_empty() && name.contains(kw))
    })
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_by_name_exact() {
        let p = find_by_name("dashscope").unwrap();
        assert_eq!(p.name, "dashscope");
        assert_eq!(p.backend, Backend::OpenAICompat);
    }

    #[test]
    fn finds_by_name_normalised() {
        assert_eq!(find_by_name("azure-openai").unwrap().name, "azure_openai");
        assert_eq!(find_by_name("LM-Studio").unwrap().name, "lm_studio");
    }

    #[test]
    fn finds_by_model_matches_keywords() {
        assert_eq!(find_by_model("gpt-5").unwrap().name, "openai");
        assert_eq!(find_by_model("claude-opus-4").unwrap().name, "anthropic");
        assert_eq!(find_by_model("qwen-max").unwrap().name, "dashscope");
        assert!(find_by_model("unknown-model").is_none());
    }

    #[test]
    fn gateway_entries_have_default_base_or_detect() {
        for p in PROVIDERS.iter().filter(|p| p.is_gateway) {
            assert!(
                !p.default_api_base.is_empty() || !p.detect_by_key_prefix.is_empty(),
                "gateway {} must have default_api_base or detect_by_key_prefix",
                p.name
            );
        }
    }

    #[test]
    fn moonshot_overrides_are_wired() {
        let p = find_by_name("moonshot").unwrap();
        assert_eq!(p.model_overrides.len(), 2);
        assert_eq!(p.model_overrides[0].0, "kimi-k2.5");
        assert_eq!(p.model_overrides[0].1[0], ("temperature", "1.0"));
    }
}
