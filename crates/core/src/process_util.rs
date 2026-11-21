use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command as TokioCommand};
use tokio_util::sync::CancellationToken;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Maximum bytes kept per stream (stdout/stderr) from a child process so noisy
/// commands cannot exhaust memory while still preserving full token counts up
/// to `FULL_COUNT_CAP`.
const MAX_STREAM_CAPTURE: usize = 2 * 1024 * 1024;

/// Maximum time to wait for stdout/stderr pipes to flush after the child exits
/// (guards against detached background descendants keeping pipe handles open).
const PIPE_DRAIN_GRACE: Duration = Duration::from_millis(500);

pub fn hide_std_cmd(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

pub fn hide_tokio_cmd(cmd: &mut TokioCommand) {
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

pub(crate) async fn kill_process_tree(pid: u32) {
    #[cfg(windows)]
    {
        let mut kill = TokioCommand::new("taskkill");
        kill.args(["/F", "/T", "/PID", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        hide_tokio_cmd(&mut kill);
        let _ = tokio::time::timeout(Duration::from_secs(3), kill.status()).await;
    }
    #[cfg(not(windows))]
    let _ = pid;
}

pub(crate) enum CmdRun {
    Finished {
        status: ExitStatus,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    TimedOut,
    Cancelled,
}

pub(crate) async fn run_tokio_cmd_cancellable(
    cmd: &mut TokioCommand,
    timeout: Duration,
    cancel: &CancellationToken,
) -> Result<CmdRun, std::io::Error> {
    if cancel.is_cancelled() {
        return Ok(CmdRun::Cancelled);
    }

    hide_tokio_cmd(cmd);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn()?;
    let pid = child.id();

    let stdout_task = child
        .stdout
        .take()
        .map(|s| tokio::spawn(drain_capped(s, MAX_STREAM_CAPTURE)));
    let stderr_task = child
        .stderr
        .take()
        .map(|s| tokio::spawn(drain_capped(s, MAX_STREAM_CAPTURE)));

    tokio::select! {
        res = child.wait() => {
            match res {
                Ok(status) => {
                    let stdout = join_pipe(stdout_task).await;
                    let stderr = join_pipe(stderr_task).await;
                    Ok(CmdRun::Finished { status, stdout, stderr })
                }
                Err(e) => Err(e),
            }
        }
        () = tokio::time::sleep(timeout) => {
            terminate_child(&mut child, pid).await;
            abort_pipe(stdout_task);
            abort_pipe(stderr_task);
            Ok(CmdRun::TimedOut)
        }
        () = cancel.cancelled() => {
            terminate_child(&mut child, pid).await;
            abort_pipe(stdout_task);
            abort_pipe(stderr_task);
            Ok(CmdRun::Cancelled)
        }
    }
}

async fn terminate_child(child: &mut Child, pid: Option<u32>) {
    if let Some(pid) = pid {
        kill_process_tree(pid).await;
    }
    let _ = child.kill().await;
    let _ = tokio::time::timeout(Duration::from_millis(500), child.wait()).await;
}

async fn drain_capped<R: tokio::io::AsyncRead + Unpin>(mut reader: R, cap: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if out.len() < cap {
                    let take = (cap - out.len()).min(n);
                    out.extend_from_slice(&buf[..take]);
                }
            }
        }
    }
    out
}

async fn join_pipe(handle: Option<tokio::task::JoinHandle<Vec<u8>>>) -> Vec<u8> {
    let Some(mut h) = handle else {
        return Vec::new();
    };
    if let Ok(Ok(bytes)) = tokio::time::timeout(PIPE_DRAIN_GRACE, &mut h).await {
        bytes
    } else {
        h.abort();
        Vec::new()
    }
}

fn abort_pipe(handle: Option<tokio::task::JoinHandle<Vec<u8>>>) {
    if let Some(h) = handle {
        h.abort();
    }
}
