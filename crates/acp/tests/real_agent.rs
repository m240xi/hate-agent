use hate_acp::Agent;
use hate_acp::runtime::Installed;

#[tokio::test]
#[ignore = "needs a downloaded Antigravity binary in HATE_AGY_EXE"]
async fn real_agent_handshake_and_auth_gate() {
    let exe = std::env::var("HATE_AGY_EXE").expect("set HATE_AGY_EXE");
    let inst = Installed {
        version: "test".into(),
        exe: exe.into(),
        args: vec![],
    };
    let started = Agent::start(Some(&inst)).await.expect("agent starts");
    let agent = started.agent;
    assert!(agent.supports_google_login());
    assert!(agent.can_resume());
    assert!(agent.can_logout());
    eprintln!("agent version: {:?}", agent.version());
    let cwd = std::env::temp_dir().join("hate-acp-real");
    std::fs::create_dir_all(&cwd).unwrap();
    match agent
        .new_session(&cwd.to_string_lossy(), serde_json::json!([]))
        .await
    {
        Ok(s) => eprintln!("signed in already; session {}", s.session_id),
        Err(e) => {
            eprintln!("session/new: {e}");
            assert!(e.is_auth_required());
        }
    }
    agent.shutdown().await;
}

#[tokio::test]
#[ignore = "downloads ~100 MB from Google into the real app data dir"]
async fn real_install_from_registry() {
    let mut last = 0u64;
    let inst = hate_acp::runtime::install(|d, t| {
        if d - last > 25 * 1024 * 1024 {
            last = d;
            eprintln!(
                "downloaded {} MB of {:?}",
                d / 1_048_576,
                t.map(|t| t / 1_048_576)
            );
        }
    })
    .await
    .expect("install");
    eprintln!("installed {} at {}", inst.version, inst.exe.display());
    assert!(inst.exe.is_file());
    assert_eq!(hate_acp::runtime::installed(), Some(inst.clone()));
    let started = hate_acp::Agent::start(Some(&inst))
        .await
        .expect("installed agent starts");
    assert!(started.agent.supports_google_login());
    started.agent.shutdown().await;
}
