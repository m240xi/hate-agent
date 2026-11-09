//! `hate` — the headless CLI. Same `hate-core` session the desktop app
//! drives; this binary is what the benchmark harness calls (spec §5).

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use hate_auth::{AuthContext, ClaudeAuth, PortalAuth};
use hate_core::{
    Command, Event, Limits, PermissionHub, SessionConfig, WorkspaceTools, spawn_session,
};
use hate_ledger::Ledger;
use hate_providers::{ProviderKind, ProviderSpec, build, warn_if_unknown};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(name = "hate", version, about = "Token-efficient coding agent")]
struct Cli {
    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage credentials (keychain + Nous Portal login).
    Auth {
        #[command(subcommand)]
        cmd: AuthCommands,
    },
    /// Send one prompt and stream the reply.
    Run {
        /// Prompt text. Use `-` to read from stdin.
        prompt: String,
        /// Provider name from config (default: configured default).
        #[arg(short, long)]
        provider: Option<String>,
        /// Override the configured model.
        #[arg(short, long)]
        model: Option<String>,
        /// Reasoning effort: off, low, medium, high.
        #[arg(short, long)]
        thinking: Option<String>,
    },
    /// List models the configured provider exposes.
    Models {
        /// Provider name from config.
        #[arg(short, long)]
        provider: Option<String>,
    },
}

#[derive(Subcommand)]
enum AuthCommands {
    /// Log in. `nous` runs the Portal device-code flow, `claude` (or
    /// `anthropic`) runs the subscription OAuth flow; other providers
    /// take `--api-key`.
    Login {
        /// Provider to log in to (default: `nous`).
        #[arg(default_value = "nous")]
        provider: String,
        /// API key for non-OAuth providers (prompted interactively if
        /// omitted and stdin is a terminal — M1 requires the flag or
        /// `set-key`).
        #[arg(long)]
        api_key: Option<String>,
    },
    /// Remove stored credentials for a provider.
    Logout {
        /// Provider name.
        provider: String,
    },
    /// Store an API key for a provider.
    SetKey {
        /// Provider name.
        provider: String,
        /// The key.
        key: String,
    },
    /// Show credential state (never prints secrets).
    Status,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let code = match cli.cmd {
        Commands::Auth { cmd } => auth(cmd).await,
        Commands::Run {
            prompt,
            provider,
            model,
            thinking,
        } => run(prompt, provider, model, thinking).await,
        Commands::Models { provider } => models(provider).await,
    };
    std::process::exit(code);
}

async fn auth(cmd: AuthCommands) -> i32 {
    let ctx = match AuthContext::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    match cmd {
        AuthCommands::Login { provider, api_key } => {
            if provider == "nous" && api_key.is_none() {
                login_nous(&ctx).await
            } else if matches!(provider.as_str(), "claude" | "anthropic") && api_key.is_none() {
                login_claude(&ctx, &provider).await
            } else {
                let Some(key) = api_key else {
                    eprintln!(
                        "error: --api-key required for non-OAuth login (or use `hate auth set-key`)"
                    );
                    return 2;
                };
                match ctx.set_api_key(&provider, &key) {
                    Ok(()) => {
                        println!("stored API key for {provider}");
                        0
                    }
                    Err(e) => {
                        eprintln!("error: {e}");
                        2
                    }
                }
            }
        }
        AuthCommands::Logout { provider } => match ctx.logout(&provider) {
            Ok(()) => {
                println!("removed credentials for {provider}");
                0
            }
            Err(e) => {
                eprintln!("error: {e}");
                2
            }
        },
        AuthCommands::SetKey { provider, key } => match ctx.set_api_key(&provider, &key) {
            Ok(()) => {
                println!("stored API key for {provider}");
                0
            }
            Err(e) => {
                eprintln!("error: {e}");
                2
            }
        },
        AuthCommands::Status => {
            let mut config = ctx.config.clone();
            config.ensure_default_provider();
            let mut any = false;
            for name in config.providers.keys() {
                match ctx.status(name) {
                    Ok(s) => {
                        any = true;
                        println!(
                            "{name}: {}{}",
                            s.kind,
                            s.expires_at
                                .map(|e| format!(" (token expires {e})"))
                                .unwrap_or_default()
                        );
                    }
                    Err(e) => {
                        eprintln!("{name}: error: {e}");
                        any = true;
                    }
                }
            }
            // Also report providers with credentials but no config entry.
            if !any {
                println!("no providers configured — run `hate auth login`");
            }
            0
        }
    }
}

async fn login_nous(ctx: &AuthContext) -> i32 {
    let auth = PortalAuth::new(Arc::clone(&ctx.store));
    let cancel = CancellationToken::new();
    let cancel2 = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            cancel2.cancel();
        }
    });
    match auth
        .login(&cancel, |info| {
            println!("Open: {}", info.verification_url);
            if let Some(full) = &info.verification_url_complete {
                println!("  (or: {full})");
            }
            println!("Enter code: {}", info.user_code);
            println!("Waiting for approval… (Ctrl-C to cancel)");
            let _ = std::io::stdout().flush();
        })
        .await
    {
        Ok(_) => {
            println!("logged in to Nous Portal");
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            2
        }
    }
}

/// Claude subscription login: PKCE authorize URL → browser → paste the
/// code the callback page shows. Also registers a provider entry in
/// config.toml when missing so the credential is reachable.
async fn login_claude(ctx: &AuthContext, provider: &str) -> i32 {
    let auth = ClaudeAuth::for_provider(Arc::clone(&ctx.store), provider);
    let login = auth.begin_login();
    println!("Open: {}", login.authorize_url);
    let _ = webbrowser::open(&login.authorize_url);
    print!("After approving, paste the code shown: ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        eprintln!("error: couldn't read code from stdin");
        return 2;
    }
    match auth.complete_login(&login.pending, &line).await {
        Ok(_) => {
            // Self-heal the provider entry so `hate run`/`hate models`
            // can actually resolve the new credential.
            match hate_auth::Config::load() {
                Ok(mut config) => {
                    config.providers.entry(provider.to_string()).or_insert(
                        hate_auth::ProviderConfig {
                            kind: "anthropic".to_string(),
                            base_url: "https://api.anthropic.com".to_string(),
                            model: "claude-sonnet-4-5".to_string(),
                        },
                    );
                    if let Err(e) = config.save() {
                        eprintln!("warning: logged in but config save failed: {e}");
                    }
                }
                Err(e) => eprintln!("warning: logged in but config load failed: {e}"),
            }
            println!("logged in to Claude ({provider})");
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            2
        }
    }
}

/// Shared setup for `run`/`models`: resolve provider + credential + model.
fn resolve_target(
    ctx: &AuthContext,
    provider: Option<String>,
    model: Option<String>,
) -> Result<(Arc<dyn hate_core::provider::Provider>, String, String), String> {
    let mut config = ctx.config.clone();
    config.ensure_default_provider();
    let (name, pconf) = config
        .resolve_provider(provider.as_deref())
        .ok_or_else(|| "no provider configured — run `hate auth login`".to_string())?;
    let name = name.to_string();

    warn_if_unknown(&pconf.kind);
    let kind = ProviderKind::parse(&pconf.kind);
    let credential = ctx
        .credential(&name)
        .map_err(|e| format!("credentials for {name}: {e}"))?;
    let base_url = ctx
        .base_url_override(&name)
        .map_err(|e| format!("base url for {name}: {e}"))?
        .unwrap_or_else(|| pconf.base_url.clone());
    let provider = build(ProviderSpec {
        kind,
        base_url,
        credential,
    });
    let model = model.unwrap_or_else(|| pconf.model.clone());
    Ok((provider, model, name))
}

fn ledger() -> Option<Arc<Ledger>> {
    let dir = dirs_data_dir();
    std::fs::create_dir_all(&dir).ok()?;
    Ledger::open(&dir.join("ledger.db")).ok().map(Arc::new)
}

fn dirs_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("hate-agent")
}

async fn run(
    prompt: String,
    provider: Option<String>,
    model: Option<String>,
    thinking: Option<String>,
) -> i32 {
    let prompt = if prompt == "-" {
        let mut buf = String::new();
        if std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf).is_err() {
            eprintln!("error: failed to read stdin");
            return 2;
        }
        buf
    } else {
        prompt
    };

    let ctx = match AuthContext::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let (provider, model, provider_name) = match resolve_target(&ctx, provider, model) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };

    let thinking = match thinking
        .as_deref()
        .map(hate_core::ThinkingLevel::parse)
        .transpose()
    {
        Ok(t) => t.unwrap_or_default(),
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };

    // Best-effort context window for compaction thresholds — one
    // catalog call; failure just disables compaction.
    let context_window = provider
        .list_models()
        .await
        .ok()
        .and_then(|ms| ms.into_iter().find(|m| m.id == model))
        .and_then(|m| m.meta)
        .and_then(|meta| meta.context_tokens)
        .map_or(0, u64::from);

    // MCP servers from the shared `mcp.json` — same file the desktop
    // app manages, so `hate run` sees identical tools. Missing file →
    // empty hub, zero cost.
    let mcp_path = dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("hate-agent")
        .join("mcp.json");
    let mcp = std::sync::Arc::new(hate_core::McpHub::from_config_file(&mcp_path).await);
    let skills = hate_core::SkillStore::default_dir()
        .map(|d| std::sync::Arc::new(hate_core::SkillStore::new(d)));
    let auto_skills = hate_auth::Config::load().map_or(true, |c| c.auto_skills);

    let session = spawn_session(SessionConfig {
        provider,
        model: model.clone(),
        provider_name: provider_name.clone(),
        thinking,
        limits: Limits::default(),
        ledger: ledger(),
        persist: None, // one-shot CLI runs stay ephemeral
        // `hate run` operates on the current directory — read tools on.
        tools: std::env::current_dir()
            .ok()
            .and_then(|p| WorkspaceTools::new(p).ok())
            .map(std::sync::Arc::new),
        permissions: std::sync::Arc::new(PermissionHub::default()),
        mcp: Some(mcp),
        skills,
        auto_skills,
        context_window,
    });

    // Ctrl-C cancels the in-flight turn (Esc is the desktop equivalent).
    let handle = session.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = handle.command(Command::Cancel);
        }
    });

    let mut rx = session.subscribe();
    let _ = session.command(Command::UserMessage(prompt));

    let mut saw_done = false;
    loop {
        match rx.recv().await {
            Ok(Event::Delta { text }) => {
                print!("{text}");
                let _ = std::io::stdout().flush();
            }
            Ok(Event::ThinkingDelta { text }) => {
                eprint!("\x1b[2m{text}\x1b[0m");
                let _ = std::io::stderr().flush();
            }
            Ok(Event::TokensCounted { usage, .. }) => {
                eprintln!(
                    "\n[usage] in={} cached={} out={} thinking={}",
                    usage.input, usage.cached_input, usage.output, usage.thinking
                );
            }
            Ok(Event::ToolStarted { call }) => {
                let args = serde_json::to_string(&call.args).unwrap_or_default();
                eprintln!("\n[tool] {} {args}", call.name);
            }
            Ok(Event::ToolFinished {
                ok,
                full_tokens,
                sent_tokens,
                ..
            }) => {
                eprintln!(
                    "[tool] {} — {} tok → {} tok{}",
                    if ok { "ok" } else { "failed" },
                    full_tokens,
                    sent_tokens,
                    if ok && full_tokens > sent_tokens {
                        format!(" (saved {} tok)", full_tokens - sent_tokens)
                    } else {
                        String::new()
                    }
                );
            }
            Ok(Event::PermissionRequested { request }) => {
                let detail = request.detail.clone().unwrap_or_default();
                eprint!(
                    "\n[permission] {} needs approval — {}\n  ",
                    request.call.name, request.reason
                );
                if !detail.is_empty() {
                    eprintln!("{detail}");
                    eprint!("  ");
                }
                eprint!("[y]es / [s]ession / [n]o? ");
                let _ = std::io::stderr().flush();
                let handle = session.clone();
                let id = request.id.clone();
                tokio::task::spawn_blocking(move || {
                    let mut line = String::new();
                    let _ = std::io::stdin().read_line(&mut line);
                    let d = match line.trim() {
                        "y" | "yes" => "allow",
                        "s" | "session" => "allow_session",
                        _ => "deny",
                    };
                    let _ = handle.command(Command::RespondPermission {
                        request_id: id,
                        decision: d.to_string(),
                    });
                });
            }
            Ok(Event::TurnComplete { outcome }) => {
                saw_done = true;
                eprintln!("\n[done] {:?} in {}ms", outcome.outcome, outcome.elapsed_ms);
                if let Some(cost) = outcome.cost_usd {
                    eprintln!("[cost] ${cost:.4}");
                }
                break;
            }
            Ok(Event::Failed { reason }) => {
                eprintln!("\n[failed] {reason:?}");
                return 1;
            }
            Ok(Event::Error { error }) => {
                eprintln!("\n[error] {error}");
            }
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
    let _ = saw_done;
    0
}

async fn models(provider: Option<String>) -> i32 {
    let ctx = match AuthContext::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let (provider, _model, _name) = match resolve_target(&ctx, provider, None) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    match provider.list_models().await {
        Ok(models) => {
            for m in models {
                match m.name {
                    Some(n) => println!("{}\t{}", m.id, n),
                    None => println!("{}", m.id),
                }
            }
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}
