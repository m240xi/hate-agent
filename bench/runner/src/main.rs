use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use clap::Parser;
use hate_core::process_util::hide_std_cmd;
use serde::{Deserialize, Serialize};

#[derive(Parser, Debug)]
#[command(
    name = "hate-bench",
    about = "Benchmark harness for hate-agent fixture tasks"
)]
struct Cli {
    /// Directory containing task subdirectories (each with task.json and fixture/).
    #[arg(long, default_value = "bench/tasks")]
    tasks_dir: PathBuf,
    /// Output directory where JSON reports are written.
    #[arg(long, default_value = "bench/results")]
    out_dir: PathBuf,
    /// Optional provider name passed to `hate run`.
    #[arg(long)]
    provider: Option<String>,
    /// Optional model id passed to `hate run`.
    #[arg(long)]
    model: Option<String>,
    /// Optional explicit path to the `hate` CLI binary.
    #[arg(long)]
    hate_bin: Option<PathBuf>,
    /// Offline smoke mode: copies each fixture, verifies compilation & baseline checks without API calls.
    #[arg(long)]
    smoke: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct TaskSpec {
    id: String,
    name: String,
    prompt: String,
    verify_cmd: String,
    #[serde(default)]
    verify_args: Vec<String>,
    #[serde(default)]
    expect_baseline_fail: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct TaskResult {
    id: String,
    name: String,
    passed: bool,
    elapsed_ms: u64,
    input_tokens: u64,
    cached_tokens: u64,
    output_tokens: u64,
    cost_usd: f64,
    detail: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct BenchSummary {
    passed: usize,
    total: usize,
    tokens_per_completed_task: f64,
    cache_hit_ratio: f64,
    cost_usd: f64,
    elapsed_ms: u64,
}

#[derive(Debug, Serialize)]
struct BenchReport {
    mode: String,
    provider: Option<String>,
    model: Option<String>,
    timestamp_epoch: u64,
    summary: BenchSummary,
    tasks: Vec<TaskResult>,
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("mkdir {}: {e}", dst.display()))?;
    let entries = std::fs::read_dir(src).map_err(|e| format!("readdir {}: {e}", src.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("entry {}: {e}", src.display()))?;
        let name = entry.file_name();
        if name == "target" || name == ".git" {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)
                .map_err(|e| format!("copy {} -> {}: {e}", from.display(), to.display()))?;
        }
    }
    Ok(())
}

fn run_hidden(
    program: &str,
    args: &[String],
    cwd: &Path,
    stdin_text: Option<&str>,
) -> Result<(bool, String), String> {
    let mut cmd = Command::new(program);
    hide_std_cmd(&mut cmd);
    cmd.args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if stdin_text.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn `{program}` in {}: {e}", cwd.display()))?;
    if let Some(input) = stdin_text
        && let Some(mut pipe) = child.stdin.take()
    {
        let _ = pipe.write_all(input.as_bytes());
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("wait `{program}`: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = if stderr.is_empty() {
        stdout.into_owned()
    } else if stdout.is_empty() {
        stderr.into_owned()
    } else {
        format!("{stderr}\n{stdout}")
    };
    Ok((out.status.success(), combined))
}

#[derive(Debug, Default, PartialEq)]
struct ParsedTelemetry {
    input_tokens: u64,
    cached_tokens: u64,
    output_tokens: u64,
    cost_usd: f64,
}

fn parse_usage_line(line: &str) -> Option<(u64, u64, u64)> {
    let rest = line.trim().strip_prefix("[usage]")?;
    let mut input = 0u64;
    let mut cached = 0u64;
    let mut output = 0u64;
    for part in rest.split_whitespace() {
        if let Some(v) = part.strip_prefix("in=") {
            input = v.parse().ok()?;
        } else if let Some(v) = part.strip_prefix("cached=") {
            cached = v.parse().ok()?;
        } else if let Some(v) = part.strip_prefix("out=") {
            output = v.parse().ok()?;
        }
    }
    Some((input, cached, output))
}

fn parse_cli_telemetry(stderr: &str) -> ParsedTelemetry {
    let mut t = ParsedTelemetry::default();
    for line in stderr.lines() {
        if let Some((i, c, o)) = parse_usage_line(line) {
            t.input_tokens = t.input_tokens.saturating_add(i);
            t.cached_tokens = t.cached_tokens.saturating_add(c);
            t.output_tokens = t.output_tokens.saturating_add(o);
        } else if let Some(rest) = line.trim().strip_prefix("[cost]")
            && let Ok(cost) = rest.trim().trim_start_matches('$').parse::<f64>()
        {
            t.cost_usd = cost;
        }
    }
    t
}

fn resolve_hate_bin(explicit: Option<&Path>) -> PathBuf {
    if let Some(p) = explicit {
        return p.to_path_buf();
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join(if cfg!(windows) { "hate.exe" } else { "hate" });
        if sibling.is_file() {
            return sibling;
        }
    }
    PathBuf::from(if cfg!(windows) { "hate.exe" } else { "hate" })
}

fn run_smoke_task(spec: &TaskSpec, fixture_src: &Path) -> TaskResult {
    let started = Instant::now();
    let Ok(tmp) = tempfile::tempdir() else {
        return TaskResult {
            id: spec.id.clone(),
            name: spec.name.clone(),
            passed: false,
            elapsed_ms: 0,
            input_tokens: 0,
            cached_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
            detail: "failed to create temp workspace".to_string(),
        };
    };
    if let Err(err) = copy_dir_recursive(fixture_src, tmp.path()) {
        return TaskResult {
            id: spec.id.clone(),
            name: spec.name.clone(),
            passed: false,
            elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            input_tokens: 0,
            cached_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
            detail: err,
        };
    }

    // Step 1: verify fixture tests compile standalone.
    if spec.verify_cmd == "cargo" {
        let check_args = vec![
            "check".to_string(),
            "--tests".to_string(),
            "--quiet".to_string(),
        ];
        match run_hidden("cargo", &check_args, tmp.path(), None) {
            Ok((true, _)) => {}
            Ok((false, out)) => {
                return TaskResult {
                    id: spec.id.clone(),
                    name: spec.name.clone(),
                    passed: false,
                    elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    input_tokens: 0,
                    cached_tokens: 0,
                    output_tokens: 0,
                    cost_usd: 0.0,
                    detail: format!("fixture compile check failed: {out}"),
                };
            }
            Err(err) => {
                return TaskResult {
                    id: spec.id.clone(),
                    name: spec.name.clone(),
                    passed: false,
                    elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    input_tokens: 0,
                    cached_tokens: 0,
                    output_tokens: 0,
                    cost_usd: 0.0,
                    detail: err,
                };
            }
        }
    }

    // Step 2: run verify_cmd baseline and assert deterministic baseline expectation.
    match run_hidden(&spec.verify_cmd, &spec.verify_args, tmp.path(), None) {
        Ok((ok, out)) => {
            let passed = if spec.expect_baseline_fail { !ok } else { ok };
            let detail = if passed {
                if spec.expect_baseline_fail {
                    "compiled cleanly; baseline bug failed as expected".to_string()
                } else {
                    "compiled cleanly; baseline check passed".to_string()
                }
            } else {
                format!(
                    "unexpected baseline result (expected fail={}, got ok={ok}): {out}",
                    spec.expect_baseline_fail
                )
            };
            TaskResult {
                id: spec.id.clone(),
                name: spec.name.clone(),
                passed,
                elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                input_tokens: 0,
                cached_tokens: 0,
                output_tokens: 0,
                cost_usd: 0.0,
                detail,
            }
        }
        Err(err) => TaskResult {
            id: spec.id.clone(),
            name: spec.name.clone(),
            passed: false,
            elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            input_tokens: 0,
            cached_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
            detail: err,
        },
    }
}

fn run_live_task(spec: &TaskSpec, fixture_src: &Path, cli: &Cli, hate_bin: &Path) -> TaskResult {
    let started = Instant::now();
    let Ok(tmp) = tempfile::tempdir() else {
        return TaskResult {
            id: spec.id.clone(),
            name: spec.name.clone(),
            passed: false,
            elapsed_ms: 0,
            input_tokens: 0,
            cached_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
            detail: "failed to create temp workspace".to_string(),
        };
    };
    if let Err(err) = copy_dir_recursive(fixture_src, tmp.path()) {
        return TaskResult {
            id: spec.id.clone(),
            name: spec.name.clone(),
            passed: false,
            elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            input_tokens: 0,
            cached_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
            detail: err,
        };
    }

    let mut args = vec!["run".to_string(), "--trust".to_string()];
    if let Some(p) = &cli.provider {
        args.push("--provider".to_string());
        args.push(p.clone());
    }
    if let Some(m) = &cli.model {
        args.push("--model".to_string());
        args.push(m.clone());
    }
    args.push(spec.prompt.clone());

    let prog = hate_bin.display().to_string();
    let (agent_ok, agent_out) = match run_hidden(&prog, &args, tmp.path(), Some("s\n")) {
        Ok(res) => res,
        Err(err) => {
            return TaskResult {
                id: spec.id.clone(),
                name: spec.name.clone(),
                passed: false,
                elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                input_tokens: 0,
                cached_tokens: 0,
                output_tokens: 0,
                cost_usd: 0.0,
                detail: err,
            };
        }
    };
    let tel = parse_cli_telemetry(&agent_out);
    let (verify_ok, verify_out) =
        match run_hidden(&spec.verify_cmd, &spec.verify_args, tmp.path(), None) {
            Ok(res) => res,
            Err(err) => {
                return TaskResult {
                    id: spec.id.clone(),
                    name: spec.name.clone(),
                    passed: false,
                    elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    input_tokens: tel.input_tokens,
                    cached_tokens: tel.cached_tokens,
                    output_tokens: tel.output_tokens,
                    cost_usd: tel.cost_usd,
                    detail: err,
                };
            }
        };

    let passed = agent_ok && verify_ok;
    let detail = if passed {
        "verified".to_string()
    } else if !agent_ok {
        format!("hate CLI failed: {agent_out}")
    } else {
        format!("verification failed: {verify_out}")
    };

    TaskResult {
        id: spec.id.clone(),
        name: spec.name.clone(),
        passed,
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        input_tokens: tel.input_tokens,
        cached_tokens: tel.cached_tokens,
        output_tokens: tel.output_tokens,
        cost_usd: tel.cost_usd,
        detail,
    }
}

fn aggregate_metrics(tasks: &[TaskResult]) -> BenchSummary {
    let total = tasks.len();
    let passed = tasks.iter().filter(|t| t.passed).count();
    let completed_tokens: u64 = tasks
        .iter()
        .filter(|t| t.passed)
        .map(|t| t.input_tokens.saturating_add(t.output_tokens))
        .sum();
    let total_input: u64 = tasks.iter().map(|t| t.input_tokens).sum();
    let total_cached: u64 = tasks.iter().map(|t| t.cached_tokens).sum();
    let cost_usd: f64 = tasks.iter().map(|t| t.cost_usd).sum();
    let elapsed_ms: u64 = tasks.iter().map(|t| t.elapsed_ms).sum();

    let tokens_per_completed_task = if passed == 0 {
        0.0
    } else {
        completed_tokens as f64 / passed as f64
    };
    let cache_hit_ratio = if total_input == 0 {
        0.0
    } else {
        (total_cached as f64 / total_input as f64).clamp(0.0, 1.0)
    };

    BenchSummary {
        passed,
        total,
        tokens_per_completed_task,
        cache_hit_ratio,
        cost_usd,
        elapsed_ms,
    }
}

fn discover_tasks(tasks_dir: &Path) -> Result<Vec<(TaskSpec, PathBuf)>, String> {
    let entries = std::fs::read_dir(tasks_dir)
        .map_err(|e| format!("read tasks dir {}: {e}", tasks_dir.display()))?;
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    let mut out = Vec::new();
    for dir in dirs {
        let task_json = dir.join("task.json");
        let fixture = dir.join("fixture");
        if !task_json.is_file() || !fixture.is_dir() {
            continue;
        }
        let raw = std::fs::read_to_string(&task_json)
            .map_err(|e| format!("read {}: {e}", task_json.display()))?;
        let spec: TaskSpec = serde_json::from_str(&raw)
            .map_err(|e| format!("parse {}: {e}", task_json.display()))?;
        out.push((spec, fixture));
    }
    Ok(out)
}

fn run_cli(cli: &Cli) -> Result<PathBuf, String> {
    let tasks = discover_tasks(&cli.tasks_dir)?;
    if tasks.is_empty() {
        return Err(format!(
            "no benchmark tasks found under {}",
            cli.tasks_dir.display()
        ));
    }

    let hate_bin = resolve_hate_bin(cli.hate_bin.as_deref());
    let mut results = Vec::with_capacity(tasks.len());
    for (spec, fixture) in &tasks {
        let res = if cli.smoke {
            run_smoke_task(spec, fixture)
        } else {
            run_live_task(spec, fixture, cli, &hate_bin)
        };
        println!(
            "[{}] {} ({} ms) — {}",
            if res.passed { "PASS" } else { "FAIL" },
            res.id,
            res.elapsed_ms,
            res.detail.lines().next().unwrap_or("")
        );
        results.push(res);
    }

    let summary = aggregate_metrics(&results);
    let now_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let report = BenchReport {
        mode: if cli.smoke {
            "smoke".to_string()
        } else {
            "live".to_string()
        },
        provider: cli.provider.clone(),
        model: cli.model.clone(),
        timestamp_epoch: now_epoch,
        summary: summary.clone(),
        tasks: results,
    };

    std::fs::create_dir_all(&cli.out_dir)
        .map_err(|e| format!("mkdir {}: {e}", cli.out_dir.display()))?;
    let file_name = if cli.smoke {
        "smoke-latest.json".to_string()
    } else {
        format!("run-{now_epoch}.json")
    };
    let out_path = cli.out_dir.join(file_name);
    let body = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    std::fs::write(&out_path, body).map_err(|e| format!("write {}: {e}", out_path.display()))?;

    println!(
        "summary: {}/{} passed | tok/task={:.1} | cache={:.1}% | cost=${:.4} | elapsed={}ms | report={}",
        summary.passed,
        summary.total,
        summary.tokens_per_completed_task,
        summary.cache_hit_ratio * 100.0,
        summary.cost_usd,
        summary.elapsed_ms,
        out_path.display()
    );
    if summary.passed < summary.total {
        return Err(format!(
            "{}/{} benchmark tasks failed",
            summary.total - summary.passed,
            summary.total
        ));
    }
    Ok(out_path)
}

fn main() {
    let cli = Cli::parse();
    if let Err(err) = run_cli(&cli) {
        eprintln!("bench error: {err}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_task_spec_and_cli_telemetry() {
        let raw = r#"{
            "id": "01-bugfix",
            "name": "Fix bug",
            "prompt": "Fix it",
            "verify_cmd": "cargo",
            "verify_args": ["test", "--quiet"],
            "expect_baseline_fail": true
        }"#;
        let spec: TaskSpec = serde_json::from_str(raw).unwrap();
        assert_eq!(spec.id, "01-bugfix");
        assert!(spec.expect_baseline_fail);
        assert_eq!(spec.verify_args, ["test", "--quiet"]);

        let stderr = "[usage] in=1000 cached=700 out=250 thinking=40\n[cost] $0.0125\n";
        let tel = parse_cli_telemetry(stderr);
        assert_eq!(
            tel,
            ParsedTelemetry {
                input_tokens: 1000,
                cached_tokens: 700,
                output_tokens: 250,
                cost_usd: 0.0125,
            }
        );
    }

    #[test]
    fn aggregates_metrics_correctly() {
        let tasks = vec![
            TaskResult {
                id: "t1".into(),
                name: "T1".into(),
                passed: true,
                elapsed_ms: 100,
                input_tokens: 1000,
                cached_tokens: 800,
                output_tokens: 200,
                cost_usd: 0.01,
                detail: String::new(),
            },
            TaskResult {
                id: "t2".into(),
                name: "T2".into(),
                passed: true,
                elapsed_ms: 250,
                input_tokens: 2000,
                cached_tokens: 1000,
                output_tokens: 400,
                cost_usd: 0.02,
                detail: String::new(),
            },
            TaskResult {
                id: "t3".into(),
                name: "T3".into(),
                passed: false,
                elapsed_ms: 50,
                input_tokens: 1000,
                cached_tokens: 0,
                output_tokens: 100,
                cost_usd: 0.01,
                detail: String::new(),
            },
        ];
        let s = aggregate_metrics(&tasks);
        assert_eq!(s.passed, 2);
        assert_eq!(s.total, 3);
        assert!((s.tokens_per_completed_task - 1800.0).abs() < 1e-6);
        assert!((s.cache_hit_ratio - 0.45).abs() < 1e-6);
        assert!((s.cost_usd - 0.04).abs() < 1e-9);
        assert_eq!(s.elapsed_ms, 400);
    }
}
