use std::collections::VecDeque;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use specta::Type;
use tauri::{AppHandle, Emitter, State};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex as AsyncMutex, OnceCell};
use tokio_util::sync::CancellationToken;

pub const LOCAL_PROVIDER: &str = "local";
const RELEASES_API: &str = "https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=10";
const PREFERRED_PORT: u16 = 11435;
const DEFAULT_PORT_URL: &str = "http://127.0.0.1:11435/v1";

pub struct CatalogEntry {
    pub id: &'static str,
    pub name: &'static str,
    pub params: &'static str,
    pub quant: &'static str,
    pub size_bytes: u64,
    pub sha256: &'static str,
    pub url: &'static str,
    pub filename: &'static str,
    pub default_ctx: u32,
    pub tool_calling: bool,
    pub summary: &'static str,
}

/// Curated single-file GGUF catalog (`Q4_K_M` / `UD-Q4_K_M`) across diverse model families with verified Hugging Face LFS SHA-256 checksums.
pub const CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        id: "lfm3-1.7b",
        name: "LiquidAI LFM3 1.7B",
        params: "1.7B",
        quant: "Q4_K_M",
        size_bytes: 1_099_627_808,
        sha256: "3793e1e6e2266919d6e015287221b41100175a98eddaba92e111aed3b05c93c4",
        url: "https://huggingface.co/LiquidAI/LFM3-1.7B-GGUF/resolve/main/LFM3-1.7B-Q4_K_M.gguf",
        filename: "LFM3-1.7B-Q4_K_M.gguf",
        default_ctx: 16384,
        tool_calling: true,
        summary: "Nov 2026 LiquidAI ultra-light fallback for 4\u{2013}8 GB laptops and fast CPU runs.",
    },
    CatalogEntry {
        id: "qwen3.5-4b",
        name: "Qwen3.5 4B",
        params: "4B",
        quant: "Q4_K_M",
        size_bytes: 2_740_937_888,
        sha256: "00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4",
        url: "https://huggingface.co/unsloth/Qwen3.5-4B-GGUF/resolve/main/Qwen3.5-4B-Q4_K_M.gguf",
        filename: "Qwen3.5-4B-Q4_K_M.gguf",
        default_ctx: 16384,
        tool_calling: true,
        summary: "Snappy 2026 compact reasoning model for 8 GB RAM laptops or 4 GB GPUs.",
    },
    CatalogEntry {
        id: "llama-3.1-8b",
        name: "Meta Llama 3.1 8B",
        params: "8B",
        quant: "Q4_K_M",
        size_bytes: 4_920_739_200,
        sha256: "b3bdbf23b47d7e6bb791c99b206deb169cd5a96362a9e3399028df2faacdc506",
        url: "https://huggingface.co/unsloth/Llama-3.1-8B-Instruct-GGUF/resolve/main/Llama-3.1-8B-Instruct-Q4_K_M.gguf",
        filename: "Llama-3.1-8B-Instruct-Q4_K_M.gguf",
        default_ctx: 16384,
        tool_calling: true,
        summary: "Classic Meta 8B all-rounder \u{2014} fast and steady on 6\u{2013}8 GB VRAM or 16 GB RAM.",
    },
    CatalogEntry {
        id: "nemotron-3.5-bolt-e4b",
        name: "NVIDIA Nemotron 3.5 Bolt E4B",
        params: "8B (4B eff.)",
        quant: "UD-Q4_K_XL",
        size_bytes: 5_060_482_240,
        sha256: "84ff34f5834b929043f5ae74799e8d562d4056998f6a5923ceb44831e8ad84d6",
        url: "https://huggingface.co/unsloth/NVIDIA-Nemotron-3.5-Bolt-E4B-GGUF/resolve/main/NVIDIA-Nemotron-3.5-Bolt-E4B-UD-Q4_K_XL.gguf",
        filename: "NVIDIA-Nemotron-3.5-Bolt-E4B-UD-Q4_K_XL.gguf",
        default_ctx: 16384,
        tool_calling: true,
        summary: "Oct 2026 NVIDIA edge assistant tuned for fast local chats on 6\u{2013}8 GB GPUs.",
    },
    CatalogEntry {
        id: "lfm2.5-8b-a1b",
        name: "LiquidAI LFM2.5 8B-A1B",
        params: "8B (1B active)",
        quant: "Q4_K_M",
        size_bytes: 5_155_564_768,
        sha256: "4923ec14f06b968b74d663e5949867d2d9c3bf13a20b8be1a9f9af39989b2bb0",
        url: "https://huggingface.co/LiquidAI/LFM2.5-8B-A1B-GGUF/resolve/main/LFM2.5-8B-A1B-Q4_K_M.gguf",
        filename: "LFM2.5-8B-A1B-Q4_K_M.gguf",
        default_ctx: 16384,
        tool_calling: true,
        summary: "Ultra-fast 1B-active MoE \u{2014} responsive even on CPU RAM or entry GPUs.",
    },
    CatalogEntry {
        id: "ministral-3-8b",
        name: "Mistral Ministral 3 8B",
        params: "8B",
        quant: "Q4_K_M",
        size_bytes: 5_198_386_720,
        sha256: "5dbc3647eb563b9f8d3c70ec3d906cce84b86bb35c5e0b8a36e7df3937ab7174",
        url: "https://huggingface.co/unsloth/Ministral-3-8B-Instruct-2512-GGUF/resolve/main/Ministral-3-8B-Instruct-2512-Q4_K_M.gguf",
        filename: "Ministral-3-8B-Instruct-2512-Q4_K_M.gguf",
        default_ctx: 16384,
        tool_calling: true,
        summary: "Mistral 8B instruction & tool-calling model for 8 GB VRAM / 16 GB RAM.",
    },
    CatalogEntry {
        id: "qwen3.5-9b",
        name: "Qwen3.5 9B",
        params: "9B",
        quant: "Q4_K_M",
        size_bytes: 5_680_522_464,
        sha256: "03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8",
        url: "https://huggingface.co/unsloth/Qwen3.5-9B-GGUF/resolve/main/Qwen3.5-9B-Q4_K_M.gguf",
        filename: "Qwen3.5-9B-Q4_K_M.gguf",
        default_ctx: 16384,
        tool_calling: true,
        summary: "Strong 2026 coding & reasoning workhorse for 8 GB VRAM or 16 GB RAM.",
    },
    CatalogEntry {
        id: "gemma-4-12b",
        name: "Google Gemma 4 12B",
        params: "12B",
        quant: "Q4_K_M",
        size_bytes: 7_121_861_440,
        sha256: "0a270ec9fe6b34f4a0d33992b6135117b484ebc4766ab76b51d4ae8c457e4c42",
        url: "https://huggingface.co/unsloth/gemma-4-12b-it-GGUF/resolve/main/gemma-4-12b-it-Q4_K_M.gguf",
        filename: "gemma-4-12b-it-Q4_K_M.gguf",
        default_ctx: 16384,
        tool_calling: true,
        summary: "Google's crisp 2026 Apache-2.0 reasoning model for 8\u{2013}12 GB VRAM.",
    },
    CatalogEntry {
        id: "ministral-3-14b",
        name: "Mistral Ministral 3 14B",
        params: "14B",
        quant: "Q4_K_M",
        size_bytes: 8_239_067_840,
        sha256: "76ce697c065f2e40f1e8e958118b02cab38e2c10a6015f7d7908036a292dc8c8",
        url: "https://huggingface.co/unsloth/Ministral-3-14B-Instruct-2512-GGUF/resolve/main/Ministral-3-14B-Instruct-2512-Q4_K_M.gguf",
        filename: "Ministral-3-14B-Instruct-2512-Q4_K_M.gguf",
        default_ctx: 32768,
        tool_calling: true,
        summary: "Mistral 14B reasoning & agent model for 10\u{2013}12 GB VRAM or 16 GB+ Macs.",
    },
    CatalogEntry {
        id: "gpt-oss-20b",
        name: "OpenAI gpt-oss 20B",
        params: "21B (3.6B active)",
        quant: "Q4_K_M",
        size_bytes: 11_624_759_488,
        sha256: "c27536640e410032865dc68781d80a08b98f8db5e93575919af8ccc0568aeb4f",
        url: "https://huggingface.co/unsloth/gpt-oss-20b-GGUF/resolve/main/gpt-oss-20b-Q4_K_M.gguf",
        filename: "gpt-oss-20b-Q4_K_M.gguf",
        default_ctx: 32768,
        tool_calling: true,
        summary: "OpenAI's Apache-2.0 MoE (3.6B active/token) \u{2014} fast on 16 GB RAM or 12\u{2013}16 GB VRAM.",
    },
    CatalogEntry {
        id: "devstral-small-2-24b",
        name: "Mistral Devstral Small 2 24B",
        params: "24B",
        quant: "Q4_K_M",
        size_bytes: 14_334_446_752,
        sha256: "d14ba9edee1bb4c4996a726deb81e49ae81800a3216f0774634238c380aee496",
        url: "https://huggingface.co/unsloth/Devstral-Small-2-24B-Instruct-2512-GGUF/resolve/main/Devstral-Small-2-24B-Instruct-2512-Q4_K_M.gguf",
        filename: "Devstral-Small-2-24B-Instruct-2512-Q4_K_M.gguf",
        default_ctx: 32768,
        tool_calling: true,
        summary: "Mistral 24B coding-agent specialist for 16 GB VRAM or 24 GB+ Macs.",
    },
    CatalogEntry {
        id: "muse-glimmer-30b",
        name: "Meta Muse-Glimmer 30B",
        params: "30B",
        quant: "UD-Q4_K_XL",
        size_bytes: 15_878_222_368,
        sha256: "82bece304887a313ece08400bc030f6066c7bff5b906b0cd40308ec8a409fd38",
        url: "https://huggingface.co/unsloth/Muse-Glimmer-30B-GGUF/resolve/main/Muse-Glimmer-30B-UD-Q4_K_XL.gguf",
        filename: "Muse-Glimmer-30B-UD-Q4_K_XL.gguf",
        default_ctx: 32768,
        tool_calling: true,
        summary: "Meta 2026 Apache-2.0 30B flagship for 24 GB GPUs or 24\u{2013}32 GB Apple Silicon.",
    },
    CatalogEntry {
        id: "qwen3.8-27b",
        name: "Qwen3.8 27B",
        params: "27B",
        quant: "UD-Q4_K_M",
        size_bytes: 16_564_259_744,
        sha256: "1bce504dfecd84153666928a41d8a66706f0158d3a317bc5196b72819d7dc529",
        url: "https://huggingface.co/unsloth/Qwen3.8-27B-GGUF/resolve/main/Qwen3.8-27B-UD-Q4_K_M.gguf",
        filename: "Qwen3.8-27B-UD-Q4_K_M.gguf",
        default_ctx: 32768,
        tool_calling: true,
        summary: "Nov 2026 flagship dense 27B reasoning & coding powerhouse for 24 GB GPUs.",
    },
    CatalogEntry {
        id: "gemma-4-26b-a4b",
        name: "Google Gemma 4 26B-A4B",
        params: "26B (4B active)",
        quant: "UD-Q4_K_M",
        size_bytes: 16_947_541_728,
        sha256: "f2c28b3dc4776931ac6f879e11f203dec637ea0f14267a86ec8f6165f63f293f",
        url: "https://huggingface.co/unsloth/gemma-4-26B-A4B-it-GGUF/resolve/main/gemma-4-26B-A4B-it-UD-Q4_K_M.gguf",
        filename: "gemma-4-26B-A4B-it-UD-Q4_K_M.gguf",
        default_ctx: 32768,
        tool_calling: true,
        summary: "Google 2026 MoE flagship (3.8B active/token) \u{2014} fast 26B quality on 24 GB GPUs.",
    },
    CatalogEntry {
        id: "glm-4.7-flash",
        name: "Z.AI GLM-4.7 Flash",
        params: "30B (3B active)",
        quant: "Q4_K_M",
        size_bytes: 18_312_339_808,
        sha256: "29837ed2c0fc5f51981adf8ac8083fcf80743c598381f13e9f06cbad0498b174",
        url: "https://huggingface.co/unsloth/GLM-4.7-Flash-GGUF/resolve/main/GLM-4.7-Flash-Q4_K_M.gguf",
        filename: "GLM-4.7-Flash-Q4_K_M.gguf",
        default_ctx: 32768,
        tool_calling: true,
        summary: "Z.AI 30B-A3B MIT-licensed agentic MoE for 24 GB+ GPUs or 32 GB+ Macs.",
    },
    CatalogEntry {
        id: "qwen3.6-35b-a3b",
        name: "Qwen3.6 35B-A3B",
        params: "35B (3B active)",
        quant: "UD-Q4_K_M",
        size_bytes: 22_134_528_992,
        sha256: "ac0e2c1189e055faa36eff361580e79c5bd6f8e76bffb4ce547f167d53e31a61",
        url: "https://huggingface.co/unsloth/Qwen3.6-35B-A3B-GGUF/resolve/main/Qwen3.6-35B-A3B-UD-Q4_K_M.gguf",
        filename: "Qwen3.6-35B-A3B-UD-Q4_K_M.gguf",
        default_ctx: 32768,
        tool_calling: true,
        summary: "2026 MoE flagship (3B active/token) for 24 GB+ GPUs or 32 GB+ Apple Silicon.",
    },
];

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct HardwareSpecs {
    pub os: String,
    pub arch: String,
    pub cpu_name: String,
    pub cpu_cores: u32,
    pub ram_mb: u32,
    pub gpu_name: Option<String>,
    pub vram_mb: u32,
    pub unified_memory: bool,
    pub effective_gpu_mb: u32,
    pub recommended_backend: String,
    pub available_backends: Vec<String>,
    pub disk_free_mb: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct LocalModelStatus {
    pub id: String,
    pub name: String,
    pub params: String,
    pub quant: String,
    pub size_mb: u32,
    pub default_ctx: u32,
    pub recommended_ctx: u32,
    pub tool_calling: bool,
    pub summary: String,
    /// `gpu`, `hybrid`, `cpu`, or `too_heavy`
    pub fit: String,
    pub fit_note: String,
    pub recommended: bool,
    pub downloaded: bool,
    pub partial_mb: u32,
    pub active: bool,
    pub custom: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct LocalRuntimeStatus {
    pub specs: HardwareSpecs,
    pub runtime_installed: Option<String>,
    pub runtime_backend: Option<String>,
    pub preferred_backend: String,
    pub context_tokens: u32,
    pub models_dir: String,
    pub server_running: bool,
    pub server_port: Option<u32>,
    pub active_model: Option<String>,
    pub active_backend_used: Option<String>,
    pub downloading_model: Option<String>,
    pub downloading_runtime: bool,
    pub models: Vec<LocalModelStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InstalledRuntime {
    version: String,
    backend: String,
    exe: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LocalPrefs {
    #[serde(default = "default_backend_pref")]
    preferred_backend: String,
    #[serde(default)]
    context_tokens: u32,
}

impl Default for LocalPrefs {
    fn default() -> Self {
        Self {
            preferred_backend: default_backend_pref(),
            context_tokens: 0,
        }
    }
}

fn default_backend_pref() -> String {
    "auto".to_string()
}

struct RunningServer {
    model_id: String,
    port: u16,
    ctx: u32,
    configured_backend: String,
    backend_used: String,
    child: Child,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
}

pub struct LocalRuntimeState {
    specs: OnceCell<HardwareSpecs>,
    server: AsyncMutex<Option<RunningServer>>,
    op_lock: AsyncMutex<()>,
    dl_cancel: Mutex<Option<CancellationToken>>,
    downloading_model: Mutex<Option<String>>,
    downloading_runtime: Mutex<bool>,
}

impl Default for LocalRuntimeState {
    fn default() -> Self {
        Self {
            specs: OnceCell::new(),
            server: AsyncMutex::new(None),
            op_lock: AsyncMutex::new(()),
            dl_cancel: Mutex::new(None),
            downloading_model: Mutex::new(None),
            downloading_runtime: Mutex::new(false),
        }
    }
}

pub fn data_root() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("hate-agent")
        .join("local-runtime")
}

pub fn bin_dir() -> PathBuf {
    data_root().join("bin")
}

pub fn models_dir() -> PathBuf {
    data_root().join("models")
}

fn installed_manifest_path() -> PathBuf {
    bin_dir().join("installed.json")
}

fn prefs_path() -> PathBuf {
    data_root().join("settings.json")
}

fn load_prefs() -> LocalPrefs {
    std::fs::read_to_string(prefs_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_prefs(prefs: &LocalPrefs) -> Result<(), String> {
    let dir = data_root();
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let text = serde_json::to_string_pretty(prefs).map_err(|e| e.to_string())?;
    std::fs::write(prefs_path(), text).map_err(|e| format!("write prefs: {e}"))
}

fn load_installed() -> Option<InstalledRuntime> {
    let text = std::fs::read_to_string(installed_manifest_path()).ok()?;
    let inst: InstalledRuntime = serde_json::from_str(&text).ok()?;
    inst.exe.is_file().then_some(inst)
}

fn bytes_to_mb(bytes: u64) -> u32 {
    u32::try_from(bytes / (1024 * 1024)).unwrap_or(u32::MAX)
}

fn catalog_by_id(id: &str) -> Option<&'static CatalogEntry> {
    CATALOG.iter().find(|e| e.id == id)
}

fn resolve_model_path(model_id: &str) -> Option<(PathBuf, Option<&'static CatalogEntry>)> {
    if let Some(entry) = catalog_by_id(model_id) {
        let p = models_dir().join(entry.filename);
        if p.is_file() {
            return Some((p, Some(entry)));
        }
    }
    let dir = models_dir();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for ent in rd.flatten() {
            let p = ent.path();
            if p.extension().and_then(|e| e.to_str()) == Some("gguf")
                && p.file_stem().and_then(|s| s.to_str()) == Some(model_id)
            {
                return Some((p, None));
            }
        }
    }
    None
}

fn custom_ggufs() -> Vec<(String, PathBuf, u32)> {
    let known: std::collections::HashSet<&str> = CATALOG.iter().map(|e| e.filename).collect();
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(models_dir()) else {
        return out;
    };
    for ent in rd.flatten() {
        let p = ent.path();
        let Some(fname) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if p.extension().and_then(|e| e.to_str()) != Some("gguf") || known.contains(fname) {
            continue;
        }
        let stem = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("custom-gguf")
            .to_string();
        let size_mb = ent.metadata().map_or(0, |m| bytes_to_mb(m.len()));
        out.push((stem, p, size_mb));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

pub fn evaluate_fit(
    specs: &HardwareSpecs,
    size_mb: u32,
    default_ctx: u32,
) -> (String, String, u32) {
    let rec_ctx = if specs.ram_mb <= 8_500 && specs.effective_gpu_mb < 6_000 {
        default_ctx.min(8_192)
    } else if specs.effective_gpu_mb >= 12_000 || specs.ram_mb >= 24_000 {
        default_ctx
    } else {
        default_ctx.min(16_384)
    };
    let kv_overhead = (rec_ctx / 8_192).max(1) * 280;
    let runtime_overhead = 550;
    let total_needed = size_mb
        .saturating_add(kv_overhead)
        .saturating_add(runtime_overhead);

    if specs.effective_gpu_mb >= total_needed {
        let label = if specs.unified_memory {
            "Unified GPU \u{00b7} Fast"
        } else {
            "Full GPU \u{00b7} Fast"
        };
        return ("gpu".to_string(), label.to_string(), rec_ctx);
    }
    if specs.ram_mb >= total_needed.saturating_add(2_048) {
        if specs.effective_gpu_mb >= 2_048 {
            return (
                "hybrid".to_string(),
                "GPU + RAM Split \u{00b7} Steady".to_string(),
                rec_ctx,
            );
        }
        return (
            "cpu".to_string(),
            "CPU RAM \u{00b7} Slower".to_string(),
            rec_ctx,
        );
    }
    (
        "too_heavy".to_string(),
        "Exceeds available memory".to_string(),
        rec_ctx,
    )
}

fn pick_recommended_id(items: &[LocalModelStatus]) -> Option<String> {
    let cpu_friendly = |m: &&LocalModelStatus| {
        !m.custom
            && m.fit == "cpu"
            && (m.size_mb <= 5_800 || (m.params.contains("active") && m.size_mb <= 12_500))
    };
    items
        .iter()
        .filter(|m| !m.custom && m.fit == "gpu")
        .max_by_key(|m| m.size_mb)
        .or_else(|| {
            items
                .iter()
                .filter(|m| !m.custom && m.fit == "hybrid")
                .max_by_key(|m| m.size_mb)
        })
        .or_else(|| items.iter().filter(cpu_friendly).max_by_key(|m| m.size_mb))
        .or_else(|| {
            items
                .iter()
                .filter(|m| !m.custom && m.fit == "cpu")
                .max_by_key(|m| m.size_mb)
        })
        .or_else(|| items.iter().filter(|m| !m.custom).min_by_key(|m| m.size_mb))
        .map(|m| m.id.clone())
}

fn configure_hidden_command(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

fn configure_hidden_tokio_command(cmd: &mut Command) {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

#[cfg(any(windows, test))]
fn is_virtual_gpu(name: &str) -> bool {
    let low = name.to_ascii_lowercase();
    [
        "microsoft basic",
        "vmware",
        "virtualbox",
        "parallels",
        "qxl",
        "hyper-v",
        "remote display",
        "vnc",
    ]
    .iter()
    .any(|bad| low.contains(bad))
}

#[cfg(not(target_os = "macos"))]
fn query_nvidia_smi() -> Option<(String, u32)> {
    let mut cmd = std::process::Command::new("nvidia-smi");
    cmd.args([
        "--query-gpu=name,memory.total",
        "--format=csv,noheader,nounits",
    ]);
    configure_hidden_command(&mut cmd);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut best: Option<(String, u32)> = None;
    for line in text.lines() {
        let Some((name, mb_str)) = line.rsplit_once(',') else {
            continue;
        };
        let mb: u32 = mb_str.trim().parse().unwrap_or(0);
        let name = name.trim().to_string();
        if best.as_ref().is_none_or(|(_, b)| mb > *b) {
            best = Some((name, mb));
        }
    }
    best
}

fn disk_free_mb_now() -> u32 {
    let target = models_dir();
    let _ = std::fs::create_dir_all(&target);

    #[cfg(windows)]
    {
        let escaped = target.to_string_lossy().replace('\'', "''");
        let ps = format!(
            "$p = '{escaped}'; $root = [System.IO.Path]::GetPathRoot($p).TrimEnd('\\').TrimEnd(':'); (Get-PSDrive -Name $root).Free"
        );
        let mut cmd = std::process::Command::new("powershell");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", &ps]);
        configure_hidden_command(&mut cmd);
        if let Ok(out) = cmd.output()
            && out.status.success()
            && let Ok(bytes) = String::from_utf8_lossy(&out.stdout).trim().parse::<u64>()
        {
            return bytes_to_mb(bytes);
        }
        0
    }

    #[cfg(not(windows))]
    {
        let mut cmd = std::process::Command::new("df");
        cmd.arg("-k").arg(&target);
        if let Ok(out) = cmd.output()
            && out.status.success()
        {
            let txt = String::from_utf8_lossy(&out.stdout);
            if let Some(line) = txt.lines().nth(1) {
                let cols: Vec<&str> = line.split_whitespace().collect();
                if cols.len() >= 4
                    && let Ok(kb) = cols[3].parse::<u64>()
                {
                    return u32::try_from(kb / 1024).unwrap_or(u32::MAX);
                }
            }
        }
        0
    }
}

fn probe_hardware_blocking() -> HardwareSpecs {
    let os = std::env::consts::OS.to_string();
    let arch = std::env::consts::ARCH.to_string();
    let cpu_cores =
        u32::try_from(std::thread::available_parallelism().map_or(4, std::num::NonZero::get))
            .unwrap_or(4);
    let mut cpu_name = format!("{cpu_cores}-core {arch}");
    let mut ram_mb: u32 = 8_192;
    let mut gpu_name: Option<String> = None;
    let mut vram_mb: u32 = 0;
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut unified_memory = false;
    let mut vulkan_ok = false;
    let mut nvidia_detected = false;

    #[cfg(windows)]
    {
        let ps = r"$cs = Get-CimInstance Win32_ComputerSystem; $cpu = (Get-CimInstance Win32_Processor | Select-Object -First 1).Name; $gpus = @(Get-CimInstance Win32_VideoController | ForEach-Object { [pscustomobject]@{ Name=$_.Name; AdapterRAM=$_.AdapterRAM } }); $qw = @(Get-ItemProperty 'HKLM:\SYSTEM\ControlSet001\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}\0*' -ErrorAction SilentlyContinue | ForEach-Object { [pscustomobject]@{ Name=$_.DriverDesc; Bytes=$_.'HardwareInformation.qwMemorySize' } }); $vulkan = Test-Path (Join-Path $env:WINDIR 'System32\vulkan-1.dll'); [pscustomobject]@{ ram=[uint64]$cs.TotalPhysicalMemory; cpu=$cpu; gpus=$gpus; qw=$qw; vulkan=$vulkan } | ConvertTo-Json -Depth 4 -Compress";
        let mut cmd = std::process::Command::new("powershell");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", ps]);
        configure_hidden_command(&mut cmd);
        if let Ok(out) = cmd.output()
            && out.status.success()
            && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout)
        {
            if let Some(r) = v.get("ram").and_then(serde_json::Value::as_u64) {
                ram_mb = bytes_to_mb(r);
            }
            if let Some(c) = v.get("cpu").and_then(serde_json::Value::as_str) {
                let c = c.trim();
                if !c.is_empty() {
                    cpu_name = c.to_string();
                }
            }
            vulkan_ok = v
                .get("vulkan")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);

            let mut best: Option<(String, u32)> = None;
            if let Some(arr) = v.get("qw").and_then(serde_json::Value::as_array) {
                for item in arr {
                    let name = item
                        .get("Name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .trim();
                    let bytes = item
                        .get("Bytes")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0);
                    if !name.is_empty() && !is_virtual_gpu(name) && bytes > 0 {
                        let mb = bytes_to_mb(bytes);
                        if best.as_ref().is_none_or(|(_, b)| mb > *b) {
                            best = Some((name.to_string(), mb));
                        }
                    }
                }
            }
            if let Some(arr) = v.get("gpus").and_then(serde_json::Value::as_array) {
                for item in arr {
                    let name = item
                        .get("Name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .trim();
                    let bytes = item
                        .get("AdapterRAM")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0);
                    if !name.is_empty() && !is_virtual_gpu(name) {
                        let mb = bytes_to_mb(bytes);
                        if best.as_ref().is_none_or(|(_, b)| mb > *b) {
                            best = Some((name.to_string(), mb));
                        }
                    }
                }
            }
            if let Some((n, m)) = best {
                nvidia_detected = n.to_ascii_lowercase().contains("nvidia");
                gpu_name = Some(n);
                vram_mb = m;
            }
        }
        if let Some((nv_name, nv_mb)) = query_nvidia_smi() {
            nvidia_detected = true;
            if nv_mb >= vram_mb {
                gpu_name = Some(nv_name);
                vram_mb = nv_mb;
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            && out.status.success()
            && let Ok(bytes) = String::from_utf8_lossy(&out.stdout).trim().parse::<u64>()
        {
            ram_mb = bytes_to_mb(bytes);
        }
        if let Ok(out) = std::process::Command::new("sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
            && out.status.success()
        {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                cpu_name = s;
            }
        }
        if arch == "aarch64" {
            unified_memory = true;
            vram_mb = (ram_mb * 70) / 100;
            gpu_name = Some(format!("{cpu_name} GPU"));
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
            for line in meminfo.lines() {
                if let Some(rest) = line.strip_prefix("MemTotal:") {
                    if let Some(kb_str) = rest.split_whitespace().next()
                        && let Ok(kb) = kb_str.parse::<u64>()
                    {
                        ram_mb = u32::try_from(kb / 1024).unwrap_or(u32::MAX);
                    }
                    break;
                }
            }
        }
        if let Ok(cpuinfo) = std::fs::read_to_string("/proc/cpuinfo") {
            for line in cpuinfo.lines() {
                if let Some((_, val)) = line.split_once(':')
                    && line.starts_with("model name")
                {
                    let name = val.trim();
                    if !name.is_empty() {
                        cpu_name = name.to_string();
                        break;
                    }
                }
            }
        }
        vulkan_ok = [
            "/usr/lib/x86_64-linux-gnu/libvulkan.so.1",
            "/usr/lib/aarch64-linux-gnu/libvulkan.so.1",
            "/usr/lib/libvulkan.so.1",
            "/usr/lib64/libvulkan.so.1",
        ]
        .iter()
        .any(|p| Path::new(p).exists());

        if let Some((nv_name, nv_mb)) = query_nvidia_smi() {
            nvidia_detected = true;
            gpu_name = Some(nv_name);
            vram_mb = nv_mb;
        } else if let Ok(drm) = std::fs::read_dir("/sys/class/drm") {
            for ent in drm.flatten() {
                let vram_path = ent.path().join("device/mem_info_vram_total");
                if let Ok(txt) = std::fs::read_to_string(&vram_path)
                    && let Ok(bytes) = txt.trim().parse::<u64>()
                {
                    let mb = bytes_to_mb(bytes);
                    if mb > vram_mb {
                        vram_mb = mb;
                        gpu_name = Some("AMD / DRM GPU".to_string());
                    }
                }
            }
        }
    }

    let effective_gpu_mb = if unified_memory || vram_mb >= 2_048 {
        vram_mb
    } else {
        0
    };

    let mut available_backends = Vec::new();
    if os == "macos" && arch == "aarch64" {
        available_backends.push("metal".to_string());
    } else {
        if vulkan_ok && (gpu_name.is_some() || vram_mb > 0) {
            available_backends.push("vulkan".to_string());
        }
        if nvidia_detected {
            available_backends.push("cuda".to_string());
        }
    }
    available_backends.push("cpu".to_string());

    let recommended_backend = if os == "macos" && arch == "aarch64" {
        "metal".to_string()
    } else if vulkan_ok && effective_gpu_mb >= 2_048 {
        "vulkan".to_string()
    } else if nvidia_detected {
        "cuda".to_string()
    } else {
        "cpu".to_string()
    };

    HardwareSpecs {
        os,
        arch,
        cpu_name,
        cpu_cores,
        ram_mb,
        gpu_name,
        vram_mb,
        unified_memory,
        effective_gpu_mb,
        recommended_backend,
        available_backends,
        disk_free_mb: disk_free_mb_now(),
    }
}

async fn get_specs(state: &LocalRuntimeState) -> HardwareSpecs {
    let mut specs = state
        .specs
        .get_or_init(|| async {
            tokio::task::spawn_blocking(probe_hardware_blocking)
                .await
                .unwrap_or_else(|_| HardwareSpecs {
                    os: std::env::consts::OS.to_string(),
                    arch: std::env::consts::ARCH.to_string(),
                    cpu_name: "CPU".to_string(),
                    cpu_cores: 4,
                    ram_mb: 8_192,
                    gpu_name: None,
                    vram_mb: 0,
                    unified_memory: false,
                    effective_gpu_mb: 0,
                    recommended_backend: "cpu".to_string(),
                    available_backends: vec!["cpu".to_string()],
                    disk_free_mb: 0,
                })
        })
        .await
        .clone();
    specs.disk_free_mb = tokio::task::spawn_blocking(disk_free_mb_now)
        .await
        .unwrap_or(specs.disk_free_mb);
    specs
}

pub fn downloaded_models() -> Vec<(String, String, u32)> {
    let mut out = Vec::new();
    for e in CATALOG {
        if models_dir().join(e.filename).is_file() {
            out.push((e.id.to_string(), e.name.to_string(), e.default_ctx));
        }
    }
    for (stem, _, _) in custom_ggufs() {
        out.push((stem.clone(), stem, 16_384));
    }
    out
}

pub async fn build_status(state: &LocalRuntimeState) -> LocalRuntimeStatus {
    let specs = get_specs(state).await;
    let prefs = load_prefs();
    let inst = load_installed();

    let (server_running, server_port, active_model, active_backend_used) = {
        let mut guard = state.server.lock().await;
        if let Some(srv) = guard.as_mut() {
            if let Ok(None) = srv.child.try_wait() {
                (
                    true,
                    Some(u32::from(srv.port)),
                    Some(srv.model_id.clone()),
                    Some(srv.backend_used.clone()),
                )
            } else {
                *guard = None;
                (false, None, None, None)
            }
        } else {
            (false, None, None, None)
        }
    };

    let downloading_model = state.downloading_model.lock().ok().and_then(|g| g.clone());
    let downloading_runtime = state.downloading_runtime.lock().is_ok_and(|g| *g);

    let mut models: Vec<LocalModelStatus> = Vec::new();
    for e in CATALOG {
        let size_mb = bytes_to_mb(e.size_bytes);
        let (fit, fit_note, rec_ctx) = evaluate_fit(&specs, size_mb, e.default_ctx);
        let full_path = models_dir().join(e.filename);
        let part_path = models_dir().join(format!("{}.part", e.filename));
        let downloaded = full_path.is_file();
        let partial_mb = if downloaded {
            0
        } else {
            std::fs::metadata(&part_path).map_or(0, |m| bytes_to_mb(m.len()))
        };
        models.push(LocalModelStatus {
            id: e.id.to_string(),
            name: e.name.to_string(),
            params: e.params.to_string(),
            quant: e.quant.to_string(),
            size_mb,
            default_ctx: e.default_ctx,
            recommended_ctx: rec_ctx,
            tool_calling: e.tool_calling,
            summary: e.summary.to_string(),
            fit,
            fit_note,
            recommended: false,
            downloaded,
            partial_mb,
            active: active_model.as_deref() == Some(e.id),
            custom: false,
        });
    }

    for (stem, _, size_mb) in custom_ggufs() {
        let (fit, fit_note, rec_ctx) = evaluate_fit(&specs, size_mb, 16_384);
        models.push(LocalModelStatus {
            id: stem.clone(),
            name: stem.clone(),
            params: "Custom".to_string(),
            quant: "GGUF".to_string(),
            size_mb,
            default_ctx: 16_384,
            recommended_ctx: rec_ctx,
            tool_calling: true,
            summary: "Custom GGUF file discovered in your local models directory.".to_string(),
            fit,
            fit_note,
            recommended: false,
            downloaded: true,
            partial_mb: 0,
            active: active_model.as_deref() == Some(stem.as_str()),
            custom: true,
        });
    }

    if let Some(best_id) = pick_recommended_id(&models) {
        for m in &mut models {
            if m.id == best_id {
                m.recommended = true;
            }
        }
    }

    LocalRuntimeStatus {
        specs,
        runtime_installed: inst.as_ref().map(|i| i.version.clone()),
        runtime_backend: inst.as_ref().map(|i| i.backend.clone()),
        preferred_backend: prefs.preferred_backend,
        context_tokens: prefs.context_tokens,
        models_dir: models_dir().display().to_string(),
        server_running,
        server_port,
        active_model,
        active_backend_used,
        downloading_model,
        downloading_runtime,
        models,
    }
}

fn find_cudart_companion(main_name: &str, asset_names: &[&str]) -> Option<String> {
    let bin_idx = main_name.find("-bin-")?;
    let bin_tail = &main_name[bin_idx..];
    asset_names
        .iter()
        .find(|n| n.starts_with("cudart-llama") && n.ends_with(bin_tail))
        .map(|s| (*s).to_string())
}

pub fn select_llama_assets(
    os: &str,
    arch: &str,
    backend: &str,
    asset_names: &[&str],
) -> Option<(String, Option<String>)> {
    if backend == "cuda" {
        let (prefix, ext_arch) = match (os, arch) {
            ("windows", "x86_64") => ("-bin-win-cuda-", "-x64.zip"),
            ("windows", "aarch64") => ("-bin-win-cuda-", "-arm64.zip"),
            ("linux", "x86_64") => ("-bin-ubuntu-cuda-", "-x64.tar.gz"),
            ("linux", "aarch64") => ("-bin-ubuntu-cuda-", "-arm64.tar.gz"),
            _ => return None,
        };
        let main = asset_names
            .iter()
            .find(|n| {
                n.starts_with("llama-")
                    && n.contains(&format!("{prefix}12."))
                    && n.ends_with(ext_arch)
            })
            .or_else(|| {
                asset_names.iter().find(|n| {
                    n.starts_with("llama-") && n.contains(prefix) && n.ends_with(ext_arch)
                })
            })?
            .to_string();
        let extra = find_cudart_companion(&main, asset_names);
        return Some((main, extra));
    }

    let suffix: &str = match (os, arch, backend) {
        ("windows", "x86_64", "vulkan") => "-bin-win-vulkan-x64.zip",
        ("windows", "x86_64", _) => "-bin-win-cpu-x64.zip",
        ("windows", "aarch64", "vulkan") => "-bin-win-vulkan-arm64.zip",
        ("windows", "aarch64", _) => "-bin-win-cpu-arm64.zip",
        ("macos", "aarch64", _) => "-bin-macos-arm64.tar.gz",
        ("macos", "x86_64", _) => "-bin-macos-x64.tar.gz",
        ("linux", "x86_64", "vulkan") => "-bin-ubuntu-vulkan-x64.tar.gz",
        ("linux", "x86_64", _) => "-bin-ubuntu-x64.tar.gz",
        ("linux", "aarch64", "vulkan") => "-bin-ubuntu-vulkan-arm64.tar.gz",
        ("linux", "aarch64", _) => "-bin-ubuntu-arm64.tar.gz",
        _ => return None,
    };

    let main = asset_names
        .iter()
        .find(|n| n.starts_with("llama-") && n.ends_with(suffix))?
        .to_string();
    Some((main, None))
}

fn verify_sha256_opt(bytes: &[u8], digest: Option<&str>) -> Result<(), String> {
    let Some(digest) = digest else {
        return Ok(());
    };
    let expected = digest.strip_prefix("sha256:").unwrap_or(digest).trim();
    if expected.len() != 64 {
        return Ok(());
    }
    let mut got = String::with_capacity(64);
    for b in Sha256::digest(bytes) {
        use std::fmt::Write as _;
        let _ = write!(got, "{b:02x}");
    }
    if got.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(format!(
            "checksum mismatch (expected {expected}, got {got})"
        ))
    }
}

fn extract_archive(staging: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    if Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
    {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
            .map_err(|e| format!("open zip: {e}"))?;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| format!("zip entry: {e}"))?;
            let Some(rel) = entry.enclosed_name() else {
                return Err("archive contains unsafe path".to_string());
            };
            let out = staging.join(rel);
            if entry.is_dir() {
                std::fs::create_dir_all(&out).map_err(|e| format!("mkdir: {e}"))?;
                continue;
            }
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
            }
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|e| format!("extract: {e}"))?;
            std::fs::write(&out, &buf).map_err(|e| format!("write {}: {e}", out.display()))?;
        }
        return Ok(());
    }

    if name.ends_with(".tar.gz") {
        let gz = flate2::read::GzDecoder::new(std::io::Cursor::new(bytes));
        let mut tar = tar::Archive::new(gz);
        tar.unpack(staging)
            .map_err(|e| format!("extract tar.gz: {e}"))?;
        return Ok(());
    }

    Err(format!("unsupported archive format: {name}"))
}

fn find_llama_server_exe(root: &Path) -> Option<PathBuf> {
    let target = if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    };
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().and_then(|n| n.to_str()) == Some(target) {
                return Some(p);
            }
        }
    }
    None
}

async fn download_bytes_with_progress(
    client: &reqwest::Client,
    url: &str,
    mut on_chunk: impl FnMut(u64, Option<u64>) + Send,
) -> Result<Vec<u8>, String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("download failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("download failed: {e}"))?;
    let total = resp.content_length();
    let mut stream = resp.bytes_stream();
    let mut out = Vec::with_capacity(usize::try_from(total.unwrap_or(0)).unwrap_or(0));
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("download failed: {e}"))?;
        out.extend_from_slice(&chunk);
        on_chunk(u64::try_from(out.len()).unwrap_or(u64::MAX), total);
    }
    Ok(out)
}

struct ChosenAssets {
    tag: String,
    main_name: String,
    main_url: String,
    main_sha: Option<String>,
    extra: Option<(String, String, Option<String>)>,
}

async fn install_runtime_inner(
    app: &AppHandle,
    state: &LocalRuntimeState,
    backend_override: Option<&str>,
) -> Result<InstalledRuntime, String> {
    let specs = get_specs(state).await;
    let prefs = load_prefs();
    let chosen_backend = match backend_override.unwrap_or(&prefs.preferred_backend) {
        "auto" => specs.recommended_backend.clone(),
        other => other.to_string(),
    };

    if let Some(cur) = load_installed()
        && cur.backend == chosen_backend
    {
        return Ok(cur);
    }

    if let Ok(mut flag) = state.downloading_runtime.lock() {
        *flag = true;
    }
    let _reset = scopeguard_reset(&state.downloading_runtime);

    let client = reqwest::Client::builder()
        .user_agent("hate-agent")
        .connect_timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| format!("http client: {e}"))?;

    let releases: Vec<serde_json::Value> = client
        .get(RELEASES_API)
        .send()
        .await
        .map_err(|e| format!("fetch llama.cpp releases: {e}"))?
        .error_for_status()
        .map_err(|e| format!("fetch llama.cpp releases: {e}"))?
        .json()
        .await
        .map_err(|e| format!("decode llama.cpp releases: {e}"))?;

    let mut chosen: Option<ChosenAssets> = None;

    for rel in &releases {
        let Some(tag) = rel.get("tag_name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(assets) = rel.get("assets").and_then(serde_json::Value::as_array) else {
            continue;
        };
        let names: Vec<&str> = assets
            .iter()
            .filter_map(|a| a.get("name").and_then(serde_json::Value::as_str))
            .collect();
        let Some((main_name, extra_name)) =
            select_llama_assets(&specs.os, &specs.arch, &chosen_backend, &names)
        else {
            continue;
        };
        let find_asset = |name: &str| {
            assets
                .iter()
                .find(|a| a.get("name").and_then(serde_json::Value::as_str) == Some(name))
        };
        let Some(main_asset) = find_asset(&main_name) else {
            continue;
        };
        let Some(main_url) = main_asset
            .get("browser_download_url")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let main_sha = main_asset
            .get("digest")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);

        let extra = extra_name.and_then(|ename| {
            let ea = find_asset(&ename)?;
            let url = ea
                .get("browser_download_url")
                .and_then(serde_json::Value::as_str)?
                .to_string();
            let sha = ea
                .get("digest")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            Some((ename, url, sha))
        });

        chosen = Some(ChosenAssets {
            tag: tag.to_string(),
            main_name,
            main_url: main_url.to_string(),
            main_sha,
            extra,
        });
        break;
    }

    let ChosenAssets {
        tag,
        main_name,
        main_url,
        main_sha,
        extra,
    } = chosen.ok_or_else(|| {
        format!(
            "no llama.cpp runtime build found for {}/{} ({chosen_backend})",
            specs.os, specs.arch
        )
    })?;

    let app2 = app.clone();
    let mut last_pct = u64::MAX;
    let main_bytes = download_bytes_with_progress(&client, &main_url, |done, total| {
        let pct = total.map_or(done / (1024 * 1024), |t| done * 100 / t.max(1));
        if pct != last_pct {
            last_pct = pct;
            let _ = app2.emit(
                "local-runtime-event",
                serde_json::json!({
                    "kind": "runtime_download",
                    "backend": chosen_backend,
                    "downloadedMb": bytes_to_mb(done),
                    "totalMb": total.map(bytes_to_mb),
                }),
            );
        }
    })
    .await?;

    verify_sha256_opt(&main_bytes, main_sha.as_deref())?;

    let extra_data = if let Some((ename, eurl, esha)) = extra {
        let mut last_epct = u64::MAX;
        let app3 = app.clone();
        let ebytes = download_bytes_with_progress(&client, &eurl, |done, total| {
            let pct = total.map_or(done / (1024 * 1024), |t| done * 100 / t.max(1));
            if pct != last_epct {
                last_epct = pct;
                let _ = app3.emit(
                    "local-runtime-event",
                    serde_json::json!({
                        "kind": "runtime_download",
                        "backend": format!("{chosen_backend} runtime libs"),
                        "downloadedMb": bytes_to_mb(done),
                        "totalMb": total.map(bytes_to_mb),
                    }),
                );
            }
        })
        .await?;
        verify_sha256_opt(&ebytes, esha.as_deref())?;
        Some((ename, ebytes))
    } else {
        None
    };

    let backend_clone = chosen_backend.clone();
    let tag_clone = tag.clone();
    tokio::task::spawn_blocking(move || {
        let root = bin_dir();
        let dest = root.join(format!("{backend_clone}-{tag_clone}"));
        let staging = root.join(format!(".staging-{backend_clone}-{tag_clone}"));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).map_err(|e| format!("mkdir: {e}"))?;

        extract_archive(&staging, &main_name, &main_bytes)?;
        if let Some((ename, ebytes)) = extra_data {
            extract_archive(&staging, &ename, &ebytes)?;
        }

        let exe_in_staging = find_llama_server_exe(&staging)
            .ok_or("downloaded archive is missing llama-server executable")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ =
                std::fs::set_permissions(&exe_in_staging, std::fs::Permissions::from_mode(0o755));
        }

        let rel_exe = exe_in_staging
            .strip_prefix(&staging)
            .map_err(|e| e.to_string())?
            .to_path_buf();

        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&staging, &dest).map_err(|e| format!("finalize runtime: {e}"))?;

        let inst = InstalledRuntime {
            version: tag_clone,
            backend: backend_clone,
            exe: dest.join(rel_exe),
        };
        let json = serde_json::to_string_pretty(&inst).map_err(|e| e.to_string())?;
        std::fs::write(installed_manifest_path(), json)
            .map_err(|e| format!("write manifest: {e}"))?;

        if let Ok(rd) = std::fs::read_dir(&root) {
            for ent in rd.flatten() {
                let p = ent.path();
                if p.is_dir() && p != dest {
                    let _ = std::fs::remove_dir_all(p);
                }
            }
        }
        Ok(inst)
    })
    .await
    .map_err(|e| format!("unpack task: {e}"))?
}

struct FlagGuard<'a>(&'a Mutex<bool>);
impl Drop for FlagGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut g) = self.0.lock() {
            *g = false;
        }
    }
}
fn scopeguard_reset(m: &Mutex<bool>) -> FlagGuard<'_> {
    FlagGuard(m)
}

fn upsert_local_provider_config(
    port: u16,
    model_id: &str,
    make_default: bool,
) -> Result<(), String> {
    let mut ctx = hate_auth::AuthContext::load().map_err(|e| e.to_string())?;
    ctx.config.providers.insert(
        LOCAL_PROVIDER.to_string(),
        hate_auth::ProviderConfig {
            kind: "openai_compat".to_string(),
            base_url: format!("http://127.0.0.1:{port}/v1"),
            model: model_id.to_string(),
        },
    );
    if make_default || ctx.config.default_provider.is_none() {
        ctx.config.default_provider = Some(LOCAL_PROVIDER.to_string());
    }
    ctx.config.save().map_err(|e| e.to_string())
}

pub async fn live_base_url(state: &LocalRuntimeState) -> Option<String> {
    let mut guard = state.server.lock().await;
    if let Some(srv) = guard.as_mut() {
        if matches!(srv.child.try_wait(), Ok(None)) {
            return Some(format!("http://127.0.0.1:{}/v1", srv.port));
        }
        *guard = None;
    }
    None
}

async fn stop_server_locked(slot: &mut Option<RunningServer>) {
    if let Some(mut srv) = slot.take() {
        let _ = srv.child.kill().await;
        let _ = srv.child.wait().await;
    }
}

fn bind_local_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", PREFERRED_PORT))
        .or_else(|_| std::net::TcpListener::bind("127.0.0.1:0"))
        .map_err(|e| format!("bind port: {e}"))?;
    listener
        .local_addr()
        .map(|a| a.port())
        .map_err(|e| format!("local addr: {e}"))
}

#[allow(clippy::too_many_arguments)]
async fn spawn_and_wait(
    exe: &Path,
    model_path: &Path,
    model_id: &str,
    ctx: u32,
    ngl: &str,
    threads: u32,
    configured_backend: &str,
    backend_label: &str,
) -> Result<RunningServer, String> {
    let port = bind_local_port()?;

    let mut cmd = Command::new(exe);
    if let Some(parent) = exe.parent() {
        cmd.current_dir(parent);
    }
    cmd.args([
        "--host",
        "127.0.0.1",
        "--port",
        &port.to_string(),
        "--model",
        &model_path.to_string_lossy(),
        "--alias",
        model_id,
        "--ctx-size",
        &ctx.to_string(),
        "--threads",
        &threads.to_string(),
        "--jinja",
        "-ngl",
        ngl,
    ]);
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::piped());
    cmd.kill_on_drop(true);
    configure_hidden_tokio_command(&mut cmd);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to launch llama-server: {e}"))?;

    let tail = Arc::new(Mutex::new(VecDeque::with_capacity(30)));
    if let Some(stderr) = child.stderr.take() {
        let tail2 = Arc::clone(&tail);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(mut q) = tail2.lock() {
                    if q.len() >= 30 {
                        q.pop_front();
                    }
                    q.push_back(line);
                }
            }
        });
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(|e| e.to_string())?;
    let health_url = format!("http://127.0.0.1:{port}/health");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            let msg = tail
                .lock()
                .ok()
                .map(|q| q.iter().cloned().collect::<Vec<_>>().join(" | "))
                .unwrap_or_default();
            return Err(format!(
                "llama-server exited ({status}) while loading model: {msg}"
            ));
        }
        if let Ok(resp) = client.get(&health_url).send().await
            && resp.status().is_success()
        {
            return Ok(RunningServer {
                model_id: model_id.to_string(),
                port,
                ctx,
                configured_backend: configured_backend.to_string(),
                backend_used: backend_label.to_string(),
                child,
                stderr_tail: tail,
            });
        }
        if tokio::time::Instant::now() >= deadline {
            let _ = child.kill().await;
            return Err("timed out waiting for local model server to load".to_string());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

pub async fn ensure_model_running(
    app: &AppHandle,
    state: &LocalRuntimeState,
    model_id: &str,
    make_default: bool,
) -> Result<(u16, String), String> {
    let _op = state.op_lock.lock().await;
    let specs = get_specs(state).await;
    let prefs = load_prefs();
    let desired_backend = if prefs.preferred_backend == "auto" {
        specs.recommended_backend.clone()
    } else {
        prefs.preferred_backend.clone()
    };

    let (model_path, entry) = resolve_model_path(model_id).ok_or_else(|| {
        format!(
            "local model {model_id:?} is not downloaded yet \u{2014} download it in Settings \u{2192} Providers"
        )
    })?;
    let size_mb = std::fs::metadata(&model_path).map_or(0, |m| bytes_to_mb(m.len()));
    let default_ctx = entry.map_or(16_384, |e| e.default_ctx);
    let (fit, _, rec_ctx) = evaluate_fit(&specs, size_mb, default_ctx);
    let ctx = if prefs.context_tokens > 0 {
        prefs.context_tokens
    } else {
        rec_ctx
    };

    {
        let mut srv_guard = state.server.lock().await;
        if let Some(srv) = srv_guard.as_mut()
            && srv.model_id == model_id
            && srv.ctx == ctx
            && srv.configured_backend == desired_backend
            && matches!(srv.child.try_wait(), Ok(None))
        {
            let _ = upsert_local_provider_config(srv.port, model_id, make_default);
            return Ok((srv.port, srv.backend_used.clone()));
        }
    }

    let inst = install_runtime_inner(app, state, Some(&desired_backend)).await?;
    {
        let mut srv_guard = state.server.lock().await;
        stop_server_locked(&mut srv_guard).await;
    }

    let threads = specs.cpu_cores.clamp(2, 16);
    let want_gpu = inst.backend != "cpu" && fit != "cpu";
    let primary_ngl = if want_gpu { "auto" } else { "0" };

    let launched = match spawn_and_wait(
        &inst.exe,
        &model_path,
        model_id,
        ctx,
        primary_ngl,
        threads,
        &desired_backend,
        &inst.backend,
    )
    .await
    {
        Ok(srv) => srv,
        Err(gpu_err) if want_gpu => {
            tracing::warn!(error = %gpu_err, "GPU launch failed, retrying local model on CPU");
            let _ = app.emit(
                "local-runtime-event",
                serde_json::json!({
                    "kind": "fallback",
                    "message": "GPU offload failed on this driver \u{2014} automatically fell back to CPU mode."
                }),
            );
            match spawn_and_wait(
                &inst.exe,
                &model_path,
                model_id,
                ctx.min(8_192),
                "0",
                threads,
                &desired_backend,
                "cpu (fallback)",
            )
            .await
            {
                Ok(srv) => srv,
                Err(cpu_ngl_err) if prefs.preferred_backend == "auto" && inst.backend != "cpu" => {
                    tracing::warn!(error = %cpu_ngl_err, "Retrying with pure CPU llama.cpp build");
                    let cpu_inst = install_runtime_inner(app, state, Some("cpu")).await?;
                    spawn_and_wait(
                        &cpu_inst.exe,
                        &model_path,
                        model_id,
                        ctx.min(8_192),
                        "0",
                        threads,
                        &desired_backend,
                        "cpu (safe mode)",
                    )
                    .await
                    .map_err(|pure_err| {
                        format!("{gpu_err}\nCPU fallback failed: {cpu_ngl_err}\nSafe CPU runtime failed: {pure_err}")
                    })?
                }
                Err(cpu_err) => {
                    return Err(format!("{gpu_err}\nCPU fallback also failed: {cpu_err}"));
                }
            }
        }
        Err(err) if prefs.preferred_backend == "auto" && inst.backend != "cpu" => {
            tracing::warn!(error = %err, "GPU binary failed to start, switching to pure CPU build");
            let _ = app.emit(
                "local-runtime-event",
                serde_json::json!({
                    "kind": "fallback",
                    "message": "GPU runtime could not start on this PC \u{2014} switched to CPU mode."
                }),
            );
            let cpu_inst = install_runtime_inner(app, state, Some("cpu")).await?;
            spawn_and_wait(
                &cpu_inst.exe,
                &model_path,
                model_id,
                ctx.min(8_192),
                "0",
                threads,
                &desired_backend,
                "cpu (safe mode)",
            )
            .await
            .map_err(|pure_err| format!("{err}\nSafe CPU runtime also failed: {pure_err}"))?
        }
        Err(err) => return Err(err),
    };

    let port = launched.port;
    let backend_used = launched.backend_used.clone();
    let _ = &launched.stderr_tail;
    {
        let mut srv_guard = state.server.lock().await;
        *srv_guard = Some(launched);
    }
    upsert_local_provider_config(port, model_id, make_default)?;
    let _ = app.emit(
        "local-runtime-event",
        serde_json::json!({
            "kind": "server_ready",
            "modelId": model_id,
            "port": port,
            "backend": backend_used,
        }),
    );
    Ok((port, backend_used))
}

pub fn shutdown_sync(state: &LocalRuntimeState) {
    if let Ok(mut guard) = state.server.try_lock()
        && let Some(mut srv) = guard.take()
    {
        let _ = srv.child.start_kill();
    }
}

#[tauri::command]
#[specta::specta]
pub async fn local_runtime_status(
    state: State<'_, LocalRuntimeState>,
) -> Result<LocalRuntimeStatus, String> {
    Ok(build_status(&state).await)
}

#[tauri::command]
#[specta::specta]
pub async fn local_runtime_set_prefs(
    state: State<'_, LocalRuntimeState>,
    preferred_backend: Option<String>,
    context_tokens: Option<u32>,
) -> Result<LocalRuntimeStatus, String> {
    let mut prefs = load_prefs();
    if let Some(b) = preferred_backend {
        let b = b.trim().to_ascii_lowercase();
        if !matches!(b.as_str(), "auto" | "vulkan" | "cuda" | "metal" | "cpu") {
            return Err(format!("unknown backend {b:?}"));
        }
        prefs.preferred_backend = b;
    }
    if let Some(ctx) = context_tokens {
        prefs.context_tokens = ctx;
    }
    save_prefs(&prefs)?;
    Ok(build_status(&state).await)
}

#[tauri::command]
#[specta::specta]
pub async fn local_runtime_install(
    app: AppHandle,
    state: State<'_, LocalRuntimeState>,
    backend: Option<String>,
) -> Result<LocalRuntimeStatus, String> {
    let _op = state.op_lock.lock().await;
    install_runtime_inner(&app, &state, backend.as_deref()).await?;
    Ok(build_status(&state).await)
}

#[tauri::command]
#[specta::specta]
pub async fn local_model_download(
    app: AppHandle,
    state: State<'_, LocalRuntimeState>,
    model_id: String,
) -> Result<LocalRuntimeStatus, String> {
    let _op = state.op_lock.lock().await;
    let entry =
        catalog_by_id(&model_id).ok_or_else(|| format!("unknown local model {model_id:?}"))?;

    let dir = models_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir: {e}"))?;
    let final_path = dir.join(entry.filename);
    if final_path.is_file() {
        return Ok(build_status(&state).await);
    }

    let part_path = dir.join(format!("{}.part", entry.filename));
    let existing = std::fs::metadata(&part_path).map_or(0, |m| m.len());
    let needed_mb = bytes_to_mb(entry.size_bytes.saturating_sub(existing)).saturating_add(512);
    let free_mb = tokio::task::spawn_blocking(disk_free_mb_now)
        .await
        .unwrap_or(0);
    if free_mb > 0 && free_mb < needed_mb {
        return Err(format!(
            "Not enough free disk space: need ~{needed_mb} MB, only {free_mb} MB free."
        ));
    }

    let cancel = CancellationToken::new();
    if let Ok(mut slot) = state.dl_cancel.lock() {
        *slot = Some(cancel.clone());
    }
    if let Ok(mut dm) = state.downloading_model.lock() {
        *dm = Some(model_id.clone());
    }

    let res = async {
        let client = reqwest::Client::builder()
            .user_agent("hate-agent")
            .connect_timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| e.to_string())?;

        let mut req = client.get(entry.url);
        if existing > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={existing}-"));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| format!("download failed: {e}"))?
            .error_for_status()
            .map_err(|e| format!("download failed: {e}"))?;

        let resuming = resp.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        let mut downloaded = if resuming { existing } else { 0 };
        let total = Some(entry.size_bytes);

        let mut file = if resuming {
            tokio::fs::OpenOptions::new()
                .append(true)
                .open(&part_path)
                .await
                .map_err(|e| format!("open part file: {e}"))?
        } else {
            tokio::fs::File::create(&part_path)
                .await
                .map_err(|e| format!("create part file: {e}"))?
        };

        let mut stream = resp.bytes_stream();
        let mut last_pct = u64::MAX;

        loop {
            tokio::select! {
                () = cancel.cancelled() => {
                    return Err("download cancelled".to_string());
                }
                chunk = stream.next() => match chunk {
                    None => break,
                    Some(Err(e)) => return Err(format!("download stream: {e}")),
                    Some(Ok(bytes)) => {
                        file.write_all(&bytes)
                            .await
                            .map_err(|e| format!("write model file: {e}"))?;
                        downloaded = downloaded
                            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
                        let pct = total.map_or(downloaded / (1024 * 1024), |t| downloaded * 100 / t.max(1));
                        if pct != last_pct {
                            last_pct = pct;
                            let _ = app.emit(
                                "local-runtime-event",
                                serde_json::json!({
                                    "kind": "model_download",
                                    "modelId": model_id,
                                    "downloadedMb": bytes_to_mb(downloaded),
                                    "totalMb": total.map(bytes_to_mb),
                                    "pct": pct.min(100),
                                }),
                            );
                        }
                    }
                }
            }
        }
        file.flush().await.map_err(|e| format!("flush: {e}"))?;
        drop(file);

        if entry.sha256.len() == 64 {
            let pp = part_path.clone();
            let expected = entry.sha256.to_string();
            tokio::task::spawn_blocking(move || {
                use std::io::BufRead as _;
                let f = std::fs::File::open(&pp).map_err(|e| e.to_string())?;
                let mut reader = std::io::BufReader::with_capacity(1024 * 1024, f);
                let mut hasher = Sha256::new();
                loop {
                    let buf = reader.fill_buf().map_err(|e| e.to_string())?;
                    if buf.is_empty() {
                        break;
                    }
                    let n = buf.len();
                    hasher.update(buf);
                    reader.consume(n);
                }
                let mut got = String::with_capacity(64);
                for b in hasher.finalize() {
                    use std::fmt::Write as _;
                    let _ = write!(got, "{b:02x}");
                }
                if got.eq_ignore_ascii_case(&expected) {
                    Ok(())
                } else {
                    Err(format!("model checksum mismatch (got {got})"))
                }
            })
            .await
            .map_err(|e| e.to_string())??;
        }

        tokio::fs::rename(&part_path, &final_path)
            .await
            .map_err(|e| format!("finalize model file: {e}"))?;
        Ok(())
    }
    .await;

    if let Ok(mut slot) = state.dl_cancel.lock() {
        slot.take();
    }
    if let Ok(mut dm) = state.downloading_model.lock() {
        *dm = None;
    }

    res?;

    // Ensure `local` provider exists in config once the user has at least one local model.
    if let Ok(ctx) = hate_auth::AuthContext::load()
        && !ctx.config.providers.contains_key(LOCAL_PROVIDER)
    {
        let _ = upsert_local_provider_config(PREFERRED_PORT, &model_id, false);
    }

    Ok(build_status(&state).await)
}

#[tauri::command]
#[specta::specta]
pub fn local_model_download_cancel(state: State<'_, LocalRuntimeState>) {
    if let Ok(mut slot) = state.dl_cancel.lock()
        && let Some(c) = slot.take()
    {
        c.cancel();
    }
}

#[tauri::command]
#[specta::specta]
pub async fn local_model_delete(
    state: State<'_, LocalRuntimeState>,
    model_id: String,
) -> Result<LocalRuntimeStatus, String> {
    let _op = state.op_lock.lock().await;
    {
        let mut srv_guard = state.server.lock().await;
        if srv_guard.as_ref().is_some_and(|s| s.model_id == model_id) {
            stop_server_locked(&mut srv_guard).await;
        }
    }
    if let Some(entry) = catalog_by_id(&model_id) {
        let p = models_dir().join(entry.filename);
        let part = models_dir().join(format!("{}.part", entry.filename));
        let _ = std::fs::remove_file(p);
        let _ = std::fs::remove_file(part);
    } else if let Some((p, _)) = resolve_model_path(&model_id) {
        let _ = std::fs::remove_file(p);
    }

    if let Ok(mut ctx) = hate_auth::AuthContext::load()
        && let Some(local_conf) = ctx.config.providers.get_mut(LOCAL_PROVIDER)
        && local_conf.model == model_id
    {
        let next = downloaded_models().into_iter().next().map(|(id, _, _)| id);
        if let Some(next_id) = next {
            local_conf.model = next_id;
            let _ = ctx.config.save();
        }
    }

    Ok(build_status(&state).await)
}

#[tauri::command]
#[specta::specta]
pub async fn local_model_start(
    app: AppHandle,
    state: State<'_, LocalRuntimeState>,
    model_id: String,
    make_default: bool,
) -> Result<LocalRuntimeStatus, String> {
    ensure_model_running(&app, &state, &model_id, make_default).await?;
    Ok(build_status(&state).await)
}

#[tauri::command]
#[specta::specta]
pub async fn local_runtime_stop(
    state: State<'_, LocalRuntimeState>,
) -> Result<LocalRuntimeStatus, String> {
    let _op = state.op_lock.lock().await;
    let mut srv_guard = state.server.lock().await;
    stop_server_locked(&mut srv_guard).await;
    Ok(build_status(&state).await)
}

#[allow(dead_code)]
fn _default_url() -> &'static str {
    DEFAULT_PORT_URL
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_specs(ram_mb: u32, effective_gpu_mb: u32, unified: bool) -> HardwareSpecs {
        HardwareSpecs {
            os: "windows".to_string(),
            arch: "x86_64".to_string(),
            cpu_name: "Test CPU".to_string(),
            cpu_cores: 8,
            ram_mb,
            gpu_name: (effective_gpu_mb > 0).then(|| "Test GPU".to_string()),
            vram_mb: effective_gpu_mb,
            unified_memory: unified,
            effective_gpu_mb,
            recommended_backend: if effective_gpu_mb > 0 {
                "vulkan".to_string()
            } else {
                "cpu".to_string()
            },
            available_backends: vec!["cpu".to_string()],
            disk_free_mb: 100_000,
        }
    }

    #[test]
    fn catalog_entries_are_well_formed() {
        let mut ids = std::collections::HashSet::new();
        for e in CATALOG {
            assert!(ids.insert(e.id), "duplicate id {}", e.id);
            assert!(e.url.starts_with("https://"));
            assert!(
                Path::new(e.filename)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("gguf"))
            );
            assert_eq!(e.sha256.len(), 64, "missing full sha256 for {}", e.id);
            assert!(e.size_bytes > 500_000_000);
        }
    }

    #[test]
    fn fit_scoring_adapts_across_hardware_tiers() {
        let rtx_16g = fake_specs(64_000, 16_384, false);
        assert_eq!(evaluate_fit(&rtx_16g, 8_585, 32_768).0, "gpu");
        assert_eq!(evaluate_fit(&rtx_16g, 18_847, 32_768).0, "hybrid");

        let laptop_8g_cpu = fake_specs(8_192, 0, false);
        assert_eq!(evaluate_fit(&laptop_8g_cpu, 2_382, 16_384).0, "cpu");
        assert_eq!(evaluate_fit(&laptop_8g_cpu, 8_585, 32_768).0, "too_heavy");

        let mac_36g = fake_specs(36_864, 25_804, true);
        assert_eq!(evaluate_fit(&mac_36g, 18_847, 32_768).0, "gpu");
    }

    #[test]
    fn recommended_model_picks_snappy_tiers() {
        let map_catalog = |specs: &HardwareSpecs| -> Vec<LocalModelStatus> {
            CATALOG
                .iter()
                .map(|e| {
                    let size_mb = bytes_to_mb(e.size_bytes);
                    let (fit, fit_note, rec_ctx) = evaluate_fit(specs, size_mb, e.default_ctx);
                    LocalModelStatus {
                        id: e.id.to_string(),
                        name: e.name.to_string(),
                        params: e.params.to_string(),
                        quant: e.quant.to_string(),
                        size_mb,
                        default_ctx: e.default_ctx,
                        recommended_ctx: rec_ctx,
                        tool_calling: e.tool_calling,
                        summary: e.summary.to_string(),
                        fit,
                        fit_note,
                        recommended: false,
                        downloaded: false,
                        partial_mb: 0,
                        active: false,
                        custom: false,
                    }
                })
                .collect()
        };

        let cpu_32g = fake_specs(32_768, 0, false);
        assert_eq!(
            pick_recommended_id(&map_catalog(&cpu_32g)).as_deref(),
            Some("gpt-oss-20b")
        );

        let gpu_8g = fake_specs(16_384, 8_192, false);
        assert_eq!(
            pick_recommended_id(&map_catalog(&gpu_8g)).as_deref(),
            Some("gemma-4-12b")
        );

        let gpu_16g = fake_specs(32_768, 16_384, false);
        assert_eq!(
            pick_recommended_id(&map_catalog(&gpu_16g)).as_deref(),
            Some("devstral-small-2-24b")
        );
    }

    #[test]
    fn selects_expected_llama_archives_per_platform() {
        let win_assets = [
            "llama-b12445-bin-win-vulkan-x64.zip",
            "llama-b12445-bin-win-cpu-x64.zip",
            "llama-b12445-bin-win-cuda-12.4-x64.zip",
            "llama-b12445-bin-win-cuda-13.5-x64.zip",
            "cudart-llama-bin-win-cuda-12.4-x64.zip",
            "cudart-llama-bin-win-cuda-13.5-x64.zip",
        ];
        assert_eq!(
            select_llama_assets("windows", "x86_64", "vulkan", &win_assets),
            Some(("llama-b12445-bin-win-vulkan-x64.zip".to_string(), None))
        );
        assert_eq!(
            select_llama_assets("windows", "x86_64", "cuda", &win_assets),
            Some((
                "llama-b12445-bin-win-cuda-12.4-x64.zip".to_string(),
                Some("cudart-llama-bin-win-cuda-12.4-x64.zip".to_string())
            ))
        );

        let linux_assets = [
            "llama-b12445-bin-ubuntu-vulkan-x64.tar.gz",
            "llama-b12445-bin-ubuntu-x64.tar.gz",
            "llama-b12445-bin-ubuntu-cuda-12.8-x64.tar.gz",
            "cudart-llama-b12445-bin-ubuntu-cuda-12.8-x64.tar.gz",
        ];
        assert_eq!(
            select_llama_assets("linux", "x86_64", "cuda", &linux_assets),
            Some((
                "llama-b12445-bin-ubuntu-cuda-12.8-x64.tar.gz".to_string(),
                Some("cudart-llama-b12445-bin-ubuntu-cuda-12.8-x64.tar.gz".to_string())
            ))
        );

        let mac_assets = ["llama-b12445-bin-macos-arm64.tar.gz"];
        assert_eq!(
            select_llama_assets("macos", "aarch64", "metal", &mac_assets),
            Some(("llama-b12445-bin-macos-arm64.tar.gz".to_string(), None))
        );
    }

    #[test]
    fn virtual_gpu_filter_ignores_software_adapters() {
        assert!(is_virtual_gpu("Microsoft Basic Render Driver"));
        assert!(is_virtual_gpu("VMware SVGA 3D"));
        assert!(!is_virtual_gpu("NVIDIA GeForce RTX 4060 Ti"));
        assert!(!is_virtual_gpu("AMD Radeon RX 7800 XT"));
    }

    #[test]
    fn hardware_probe_returns_sane_host_specs() {
        let specs = probe_hardware_blocking();
        assert!(specs.cpu_cores >= 1);
        assert!(specs.ram_mb >= 1024);
        assert!(specs.available_backends.contains(&"cpu".to_string()));
        assert_ne!(specs.recommended_backend, "");
    }
}
