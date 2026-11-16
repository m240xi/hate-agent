//! The provider catalog.
//!
//! Two wire formats cover the field — the Anthropic Messages API and chat
//! completions — so almost everything here is an `openai_compat` entry with a
//! base URL and a sensible default model. Base URLs omit the trailing slash and
//! stop at the version segment, because the adapter appends `/chat/completions`
//! and `/models`.
//!
//! Both the desktop app (Settings → Add a provider) and the CLI (`hate auth
//! add`) read this table, so a new provider is one row.

pub struct Preset {
    pub name: &'static str,
    pub label: &'static str,
    /// `first-party`, `router`, `hosted`, or `local`.
    pub group: &'static str,
    pub kind: &'static str,
    pub base_url: &'static str,
    pub model: &'static str,
    /// `api_key`, `oauth`, `agent`, or `none`.
    pub auth: &'static str,
    pub env: Option<&'static str>,
    pub keys: Option<&'static str>,
}

const fn api(
    name: &'static str,
    label: &'static str,
    group: &'static str,
    base_url: &'static str,
    model: &'static str,
    env: &'static str,
    keys: &'static str,
) -> Preset {
    Preset {
        name,
        label,
        group,
        kind: "openai_compat",
        base_url,
        model,
        auth: "api_key",
        env: Some(env),
        keys: Some(keys),
    }
}

const fn local(name: &'static str, label: &'static str, base_url: &'static str) -> Preset {
    Preset {
        name,
        label,
        group: "local",
        kind: "openai_compat",
        base_url,
        model: "local-model",
        auth: "none",
        env: None,
        keys: None,
    }
}

pub const PRESETS: &[Preset] = &[
    // -- first-party APIs ---------------------------------------------------
    Preset {
        name: "claude",
        label: "Claude",
        group: "first-party",
        kind: "anthropic",
        base_url: "https://api.anthropic.com",
        model: "claude-sonnet-4-5",
        auth: "oauth",
        env: Some("ANTHROPIC_API_KEY"),
        keys: Some("https://console.anthropic.com/settings/keys"),
    },
    api(
        "openai",
        "OpenAI",
        "first-party",
        "https://api.openai.com/v1",
        "gpt-5",
        "OPENAI_API_KEY",
        "https://platform.openai.com/api-keys",
    ),
    api(
        "gemini",
        "Google Gemini",
        "first-party",
        "https://generativelanguage.googleapis.com/v1beta/openai",
        "gemini-2.5-pro",
        "GEMINI_API_KEY",
        "https://aistudio.google.com/apikey",
    ),
    Preset {
        name: "antigravity",
        label: "Antigravity (Google)",
        group: "first-party",
        kind: "acp",
        base_url: "Google's official Antigravity agent",
        model: "",
        auth: "agent",
        env: None,
        keys: Some("https://antigravity.google/pricing"),
    },
    api(
        "xai",
        "xAI",
        "first-party",
        "https://api.x.ai/v1",
        "grok-4",
        "XAI_API_KEY",
        "https://console.x.ai",
    ),
    api(
        "deepseek",
        "DeepSeek",
        "first-party",
        "https://api.deepseek.com/v1",
        "deepseek-chat",
        "DEEPSEEK_API_KEY",
        "https://platform.deepseek.com/api_keys",
    ),
    api(
        "mistral",
        "Mistral",
        "first-party",
        "https://api.mistral.ai/v1",
        "mistral-large-latest",
        "MISTRAL_API_KEY",
        "https://console.mistral.ai/api-keys",
    ),
    api(
        "moonshot",
        "Moonshot (Kimi)",
        "first-party",
        "https://api.moonshot.ai/v1",
        "kimi-latest",
        "MOONSHOT_API_KEY",
        "https://platform.moonshot.ai/console/api-keys",
    ),
    api(
        "zai",
        "Z.AI (GLM)",
        "first-party",
        "https://api.z.ai/api/paas/v4",
        "glm-4.6",
        "ZAI_API_KEY",
        "https://z.ai/manage-apikey/apikey-list",
    ),
    api(
        "qwen",
        "Alibaba Qwen",
        "first-party",
        "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
        "qwen-plus",
        "DASHSCOPE_API_KEY",
        "https://bailian.console.alibabacloud.com",
    ),
    api(
        "minimax",
        "MiniMax",
        "first-party",
        "https://api.minimax.io/v1",
        "MiniMax-M2",
        "MINIMAX_API_KEY",
        "https://platform.minimax.io/user-center/basic-information/interface-key",
    ),
    api(
        "cohere",
        "Cohere",
        "first-party",
        "https://api.cohere.ai/compatibility/v1",
        "command-a-03-2025",
        "COHERE_API_KEY",
        "https://dashboard.cohere.com/api-keys",
    ),
    api(
        "upstage",
        "Upstage (Solar)",
        "first-party",
        "https://api.upstage.ai/v1",
        "solar-pro4",
        "UPSTAGE_API_KEY",
        "https://console.upstage.ai/api-keys",
    ),
    api(
        "perplexity",
        "Perplexity",
        "first-party",
        "https://api.perplexity.ai/v1",
        "sonar",
        "PERPLEXITY_API_KEY",
        "https://www.perplexity.ai/settings/api",
    ),
    // -- routers and aggregators --------------------------------------------
    Preset {
        name: "nous",
        label: "Nous Portal",
        group: "router",
        kind: "openai_compat",
        base_url: crate::nous::DEFAULT_INFERENCE_URL,
        model: "moonshotai/kimi-k3",
        auth: "oauth",
        env: Some("NOUS_API_KEY"),
        keys: Some("https://portal.nousresearch.com"),
    },
    api(
        "openrouter",
        "OpenRouter",
        "router",
        "https://openrouter.ai/api/v1",
        "anthropic/claude-sonnet-4.5",
        "OPENROUTER_API_KEY",
        "https://openrouter.ai/keys",
    ),
    api(
        "requesty",
        "Requesty",
        "router",
        "https://router.requesty.ai/v1",
        "openai/gpt-4o",
        "REQUESTY_API_KEY",
        "https://app.requesty.ai/api-keys",
    ),
    api(
        "github",
        "GitHub Models",
        "router",
        "https://models.github.ai/inference",
        "openai/gpt-4o",
        "GITHUB_TOKEN",
        "https://github.com/settings/tokens",
    ),
    // -- hosted open-model inference ----------------------------------------
    api(
        "groq",
        "Groq",
        "hosted",
        "https://api.groq.com/openai/v1",
        "openai/gpt-oss-120b",
        "GROQ_API_KEY",
        "https://console.groq.com/keys",
    ),
    api(
        "cerebras",
        "Cerebras",
        "hosted",
        "https://api.cerebras.ai/v1",
        "gpt-oss-120b",
        "CEREBRAS_API_KEY",
        "https://cloud.cerebras.ai",
    ),
    api(
        "sambanova",
        "SambaNova",
        "hosted",
        "https://api.sambanova.ai/v1",
        "DeepSeek-V3-0324",
        "SAMBANOVA_API_KEY",
        "https://cloud.sambanova.ai/apis",
    ),
    api(
        "together",
        "Together AI",
        "hosted",
        "https://api.together.ai/v1",
        "meta-llama/Llama-3.3-70B-Instruct-Turbo",
        "TOGETHER_API_KEY",
        "https://api.together.ai/settings/api-keys",
    ),
    api(
        "fireworks",
        "Fireworks",
        "hosted",
        "https://api.fireworks.ai/inference/v1",
        "accounts/fireworks/models/llama-v3p3-70b-instruct",
        "FIREWORKS_API_KEY",
        "https://fireworks.ai/account/api-keys",
    ),
    api(
        "deepinfra",
        "DeepInfra",
        "hosted",
        "https://api.deepinfra.com/v1/openai",
        "deepseek-ai/DeepSeek-V3",
        "DEEPINFRA_API_KEY",
        "https://deepinfra.com/dash/api_keys",
    ),
    api(
        "nebius",
        "Nebius",
        "hosted",
        "https://api.studio.nebius.com/v1",
        "deepseek-ai/DeepSeek-V3-0324",
        "NEBIUS_API_KEY",
        "https://studio.nebius.com/settings/api-keys",
    ),
    api(
        "novita",
        "Novita AI",
        "hosted",
        "https://api.novita.ai/openai/v1",
        "meta-llama/llama-3.3-70b-instruct",
        "NOVITA_API_KEY",
        "https://novita.ai/settings/key-management",
    ),
    api(
        "hyperbolic",
        "Hyperbolic",
        "hosted",
        "https://api.hyperbolic.ai/inference/v1",
        "deepseek-ai/DeepSeek-V3",
        "HYPERBOLIC_API_KEY",
        "https://app.hyperbolic.ai/settings",
    ),
    // -- local servers -------------------------------------------------------
    Preset {
        name: "local",
        label: "Built-in Local",
        group: "local",
        kind: "openai_compat",
        base_url: "http://127.0.0.1:11435/v1",
        model: "qwen3.5-9b",
        auth: "none",
        env: None,
        keys: None,
    },
    local("ollama", "Ollama", "http://localhost:11434/v1"),
    local("lmstudio", "LM Studio", "http://localhost:1234/v1"),
    local("llamacpp", "llama.cpp", "http://localhost:8080/v1"),
    local("vllm", "vLLM", "http://localhost:8000/v1"),
];

pub fn find(name: &str) -> Option<&'static Preset> {
    let name = name.trim().to_ascii_lowercase();
    PRESETS.iter().find(|p| p.name == name)
}

/// Candidate environment variables for a provider name, in priority order.
///
/// Aliases are accepted so a config written by hand or by an older build keeps
/// working.
pub fn env_vars_for(name: &str) -> &'static [&'static str] {
    match name.trim().to_ascii_lowercase().as_str() {
        "anthropic" => &["ANTHROPIC_API_KEY"],
        "google" => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        "grok" => &["XAI_API_KEY"],
        "kimi" => &["MOONSHOT_API_KEY"],
        "zhipu" => &["ZAI_API_KEY", "ZHIPUAI_API_KEY"],
        "dashscope" => &["DASHSCOPE_API_KEY", "QWEN_API_KEY"],
        other => match find(other) {
            Some(preset) => env_vars(preset),
            None => &[],
        },
    }
}

/// Environment variable candidates for a catalog entry.
pub fn env_vars(preset: &'static Preset) -> &'static [&'static str] {
    match preset.name {
        "openai" => &["OPENAI_API_KEY"],
        "claude" => &["ANTHROPIC_API_KEY"],
        "gemini" => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        "antigravity" => &[],
        "xai" => &["XAI_API_KEY"],
        "deepseek" => &["DEEPSEEK_API_KEY"],
        "mistral" => &["MISTRAL_API_KEY"],
        "moonshot" => &["MOONSHOT_API_KEY"],
        "zai" => &["ZAI_API_KEY", "ZHIPUAI_API_KEY"],
        "qwen" => &["DASHSCOPE_API_KEY", "QWEN_API_KEY"],
        "minimax" => &["MINIMAX_API_KEY"],
        "cohere" => &["COHERE_API_KEY"],
        "upstage" => &["UPSTAGE_API_KEY"],
        "perplexity" => &["PERPLEXITY_API_KEY", "PPLX_API_KEY"],
        "nous" => &["NOUS_API_KEY"],
        "openrouter" => &["OPENROUTER_API_KEY"],
        "requesty" => &["REQUESTY_API_KEY"],
        "github" => &["GITHUB_TOKEN", "GH_TOKEN"],
        "groq" => &["GROQ_API_KEY"],
        "cerebras" => &["CEREBRAS_API_KEY"],
        "sambanova" => &["SAMBANOVA_API_KEY"],
        "together" => &["TOGETHER_API_KEY"],
        "fireworks" => &["FIREWORKS_API_KEY"],
        "deepinfra" => &["DEEPINFRA_API_KEY"],
        "nebius" => &["NEBIUS_API_KEY"],
        "novita" => &["NOVITA_API_KEY"],
        "hyperbolic" => &["HYPERBOLIC_API_KEY"],
        _ => &[],
    }
}
