use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use tokio::process::Command as TokioCommand;
use tokio_util::sync::CancellationToken;

use crate::process_util::{CmdRun, run_tokio_cmd_cancellable};

const VERIFY_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_FAILURE_LINES: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyCheck {
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyFailure {
    pub label: String,
    pub signatures: Vec<String>,
    pub snippet: String,
}

#[derive(Debug, Clone, Default)]
pub struct VerifyBaseline {
    pub checks: Vec<VerifyCheck>,
    pub existing_failures: BTreeSet<String>,
}

/// Discover deterministic check commands for a repository in priority order:
/// 1. `AGENTS.md` check lines / code blocks
/// 2. `package.json` (`typecheck`, `check`, `lint`, `test`)
/// 3. `Cargo.toml` (`cargo check --quiet`)
/// 4. `Makefile` (`check` / `test`)
pub fn discover_checks(root: &Path) -> Vec<VerifyCheck> {
    if let Some(checks) = discover_agents_md(root)
        && !checks.is_empty()
    {
        return checks;
    }
    if let Some(checks) = discover_package_json(root)
        && !checks.is_empty()
    {
        return checks;
    }
    if root.join("Cargo.toml").is_file() {
        return vec![VerifyCheck {
            label: "cargo check".to_string(),
            program: "cargo".to_string(),
            args: vec!["check".to_string(), "--quiet".to_string()],
        }];
    }
    if let Some(checks) = discover_makefile(root)
        && !checks.is_empty()
    {
        return checks;
    }
    Vec::new()
}

fn discover_agents_md(root: &Path) -> Option<Vec<VerifyCheck>> {
    let text = std::fs::read_to_string(root.join("AGENTS.md")).ok()?;
    let mut checks = Vec::new();
    let mut in_verify_section = false;
    let mut in_code_fence = false;

    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('#') {
            let low = line.to_ascii_lowercase();
            in_verify_section = low.contains("verify")
                || low.contains("verification")
                || low.contains("checks")
                || low.contains("test");
            in_code_fence = false;
            continue;
        }
        if line.starts_with("```") {
            in_code_fence = !in_code_fence;
            continue;
        }
        let candidate = if in_verify_section && in_code_fence {
            Some(line)
        } else {
            line.strip_prefix("Check:")
                .or_else(|| line.strip_prefix("Verify:"))
                .or_else(|| line.strip_prefix("- Check:"))
                .or_else(|| line.strip_prefix("- Verify:"))
                .map(|rest| rest.trim().trim_matches('`'))
        };

        if let Some(cmd) = candidate
            && let Some(parsed) = parse_simple_check_cmd(cmd)
            && !checks.contains(&parsed)
        {
            checks.push(parsed);
            if checks.len() >= 3 {
                break;
            }
        }
    }
    Some(checks)
}

fn parse_simple_check_cmd(cmd: &str) -> Option<VerifyCheck> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let parts: Vec<String> = trimmed.split_whitespace().map(str::to_string).collect();
    let prog = parts.first()?.as_str();
    if !matches!(
        prog,
        "cargo" | "npm" | "pnpm" | "yarn" | "pytest" | "go" | "make" | "npx"
    ) {
        return None;
    }
    Some(VerifyCheck {
        label: trimmed.to_string(),
        program: prog.to_string(),
        args: parts[1..].to_vec(),
    })
}

fn discover_package_json(root: &Path) -> Option<Vec<VerifyCheck>> {
    let raw = std::fs::read_to_string(root.join("package.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let scripts = json.get("scripts")?.as_object()?;
    let runner = if root.join("pnpm-lock.yaml").is_file() {
        "pnpm"
    } else if root.join("yarn.lock").is_file() {
        "yarn"
    } else {
        "npm"
    };

    let mut checks = Vec::new();
    for key in ["typecheck", "check", "lint", "test"] {
        let Some(body) = scripts.get(key).and_then(|v| v.as_str()) else {
            continue;
        };
        if body.contains("no test specified") || body.trim().is_empty() {
            continue;
        }
        let prog = if cfg!(windows) {
            format!("{runner}.cmd")
        } else {
            runner.to_string()
        };
        checks.push(VerifyCheck {
            label: format!("{runner} run {key}"),
            program: prog,
            args: vec!["run".to_string(), key.to_string()],
        });
        if checks.len() >= 2 {
            break;
        }
    }
    Some(checks)
}

fn discover_makefile(root: &Path) -> Option<Vec<VerifyCheck>> {
    let raw = std::fs::read_to_string(root.join("Makefile")).ok()?;
    let mut checks = Vec::new();
    for target in ["check", "test"] {
        let prefix = format!("{target}:");
        if raw.lines().any(|l| l.starts_with(&prefix)) {
            checks.push(VerifyCheck {
                label: format!("make {target}"),
                program: "make".to_string(),
                args: vec![target.to_string()],
            });
            break;
        }
    }
    Some(checks)
}

pub async fn run_verify_checks(
    root: &Path,
    checks: &[VerifyCheck],
    cancel: &CancellationToken,
) -> Vec<VerifyFailure> {
    let mut failures = Vec::new();
    for chk in checks {
        if cancel.is_cancelled() {
            break;
        }
        let mut cmd = TokioCommand::new(&chk.program);
        cmd.args(&chk.args).current_dir(root);
        match run_tokio_cmd_cancellable(&mut cmd, VERIFY_TIMEOUT, cancel).await {
            Ok(CmdRun::Finished {
                status,
                stdout,
                stderr,
            }) => {
                if !status.success() {
                    let out_s = crate::shape::clean(&String::from_utf8_lossy(&stdout));
                    let err_s = crate::shape::clean(&String::from_utf8_lossy(&stderr));
                    let combined = if err_s.is_empty() {
                        out_s
                    } else if out_s.is_empty() {
                        err_s
                    } else {
                        format!("{err_s}\n{out_s}")
                    };
                    let sigs = extract_failure_signatures(&combined);
                    let snippet = sigs
                        .iter()
                        .take(MAX_FAILURE_LINES)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("\n");
                    failures.push(VerifyFailure {
                        label: chk.label.clone(),
                        signatures: sigs,
                        snippet,
                    });
                }
            }
            Ok(CmdRun::TimedOut) => {
                failures.push(VerifyFailure {
                    label: chk.label.clone(),
                    signatures: vec![format!("{}: timed out", chk.label)],
                    snippet: format!(
                        "{} timed out after {}s",
                        chk.label,
                        VERIFY_TIMEOUT.as_secs()
                    ),
                });
            }
            Ok(CmdRun::Cancelled) => break,
            Err(e) => {
                // If the discovered runner binary isn't installed in PATH, skip cleanly.
                tracing::debug!(check = %chk.label, error = %e, "verify check skipped");
            }
        }
    }
    failures
}

pub async fn capture_baseline(root: &Path, cancel: &CancellationToken) -> VerifyBaseline {
    let checks = discover_checks(root);
    if checks.is_empty() {
        return VerifyBaseline::default();
    }
    let failures = run_verify_checks(root, &checks, cancel).await;
    let existing_failures = failures.into_iter().flat_map(|f| f.signatures).collect();
    VerifyBaseline {
        checks,
        existing_failures,
    }
}

pub fn diff_new_failures(baseline: &VerifyBaseline, current: Vec<VerifyFailure>) -> Option<String> {
    let mut newly = Vec::new();
    for f in current {
        let new_lines: Vec<String> = f
            .signatures
            .into_iter()
            .filter(|sig| !baseline.existing_failures.contains(sig))
            .take(MAX_FAILURE_LINES)
            .collect();
        if !new_lines.is_empty() {
            newly.push((f.label, new_lines));
        }
    }
    if newly.is_empty() {
        return None;
    }
    let mut out = String::from(
        "[verification failed — newly introduced errors after your edits; fix these before finishing:]",
    );
    for (label, lines) in newly {
        let _ = write!(out, "\n\n## {label}\n{}", lines.join("\n"));
    }
    Some(out)
}

fn extract_failure_signatures(output: &str) -> Vec<String> {
    let lines: Vec<String> = output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| l.chars().take(200).collect())
        .collect();
    let prioritized: Vec<String> = lines
        .iter()
        .filter(|l| {
            let low = l.to_ascii_lowercase();
            low.starts_with("error")
                || low.contains("error[")
                || low.contains("failed")
                || low.contains("assertion")
                || low.contains("panicked at")
                || low.contains("error ts")
        })
        .take(MAX_FAILURE_LINES * 2)
        .cloned()
        .collect();
    if prioritized.is_empty() {
        lines.into_iter().take(MAX_FAILURE_LINES).collect()
    } else {
        prioritized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_checks_in_priority_order() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // 4. Makefile
        std::fs::write(root.join("Makefile"), "test:\n\techo ok\n").unwrap();
        assert_eq!(discover_checks(root)[0].label, "make test");

        // 3. Cargo.toml overrides Makefile
        std::fs::write(root.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        assert_eq!(discover_checks(root)[0].label, "cargo check");

        // 2. package.json overrides Cargo.toml
        std::fs::write(
            root.join("package.json"),
            r#"{"scripts":{"typecheck":"tsc --noEmit","test":"vitest"}}"#,
        )
        .unwrap();
        let pkg = discover_checks(root);
        assert_eq!(pkg.len(), 2);
        assert!(pkg[0].label.contains("typecheck"));
        assert!(pkg[1].label.contains("test"));

        // 1. AGENTS.md overrides package.json
        std::fs::write(
            root.join("AGENTS.md"),
            "# Rules\n## Verification\n```sh\ncargo test -p core\n```\n",
        )
        .unwrap();
        let ag = discover_checks(root);
        assert_eq!(ag[0].label, "cargo test -p core");
    }

    #[test]
    fn diffs_only_newly_introduced_failures() {
        let mut existing = BTreeSet::new();
        existing.insert("error: preexisting warning-as-error".to_string());
        let baseline = VerifyBaseline {
            checks: Vec::new(),
            existing_failures: existing,
        };
        let current = vec![VerifyFailure {
            label: "cargo check".to_string(),
            signatures: vec![
                "error: preexisting warning-as-error".to_string(),
                "error[E0425]: cannot find value `x`".to_string(),
            ],
            snippet: String::new(),
        }];
        let msg = diff_new_failures(&baseline, current).expect("new failure detected");
        assert!(msg.contains("E0425"));
        assert!(!msg.contains("preexisting"));

        let same_only = vec![VerifyFailure {
            label: "cargo check".to_string(),
            signatures: vec!["error: preexisting warning-as-error".to_string()],
            snippet: String::new(),
        }];
        assert!(diff_new_failures(&baseline, same_only).is_none());
    }
}
