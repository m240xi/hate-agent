use std::time::Duration;

use hate_acp::rpc::Connection;
use hate_acp::session::{AcpSessionConfig, spawn_with_connection};
use hate_core::{Command, Event, Outcome};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, duplex};

async fn next_event(rx: &mut tokio::sync::broadcast::Receiver<Event>) -> Event {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out waiting for an event")
        .expect("event channel closed")
}

#[tokio::test]
async fn drives_a_full_turn_through_a_fake_agent() {
    let (client_side, agent_side) = duplex(256 * 1024);
    let (cr, cw) = tokio::io::split(client_side);
    let (ar, mut aw) = tokio::io::split(agent_side);
    let (conn, incoming) = Connection::new(cr, cw);

    let agent = tokio::spawn(async move {
        let mut lines = BufReader::new(ar).lines();
        let mut read = async || -> Value {
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap()
        };
        let mut send = async |v: Value| {
            aw.write_all(format!("{v}\n").as_bytes()).await.unwrap();
        };

        let init = read().await;
        assert_eq!(init["method"], "initialize");
        send(json!({"jsonrpc":"2.0","id":init["id"],"result":{"protocolVersion":1,"agentCapabilities":{},"authMethods":[{"id":"oauth-personal"}]}})).await;

        let new = read().await;
        assert_eq!(new["method"], "session/new");
        assert_eq!(new["params"]["mcpServers"], json!([]));
        send(
            json!({"jsonrpc":"2.0","id":new["id"],"result":{"sessionId":"s1","configOptions":[
            {"id":"model","category":"model","type":"select","currentValue":"flash",
             "options":[{"value":"flash","name":"Flash"},{"value":"pro","name":"Pro"}]}]}}),
        )
        .await;

        let set = read().await;
        assert_eq!(set["method"], "session/set_config_option");
        assert_eq!(set["params"]["value"], "pro");
        send(json!({"jsonrpc":"2.0","id":set["id"],"result":{"configOptions":[]}})).await;

        let prompt = read().await;
        assert_eq!(prompt["method"], "session/prompt");
        assert_eq!(prompt["params"]["prompt"][0]["text"], "fix the bug");
        let upd = |u: Value| json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":u}});
        send(upd(
            json!({"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"hmm"}}),
        ))
        .await;
        send(upd(json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"On it."}}))).await;
        send(upd(json!({"sessionUpdate":"tool_call","toolCallId":"t1","title":"Edit main.rs","kind":"edit","status":"pending","locations":[{"path":"/r/main.rs"}]}))).await;
        send(json!({"jsonrpc":"2.0","id":"perm-1","method":"session/request_permission","params":{"sessionId":"s1",
            "toolCall":{"toolCallId":"t1"},
            "options":[{"optionId":"yes","name":"Allow","kind":"allow_once"},{"optionId":"no","name":"Reject","kind":"reject_once"}]}})).await;

        let reply = read().await;
        assert_eq!(reply["id"], "perm-1");
        assert_eq!(reply["result"]["outcome"]["optionId"], "yes");
        send(upd(
            json!({"sessionUpdate":"tool_call_update","toolCallId":"t1","status":"completed",
            "content":[{"type":"diff","path":"/r/main.rs","oldText":"a","newText":"a\nb"}]}),
        ))
        .await;
        send(upd(json!({"sessionUpdate":"tool_call_update","toolCallId":"t1",
            "content":[{"type":"content","content":{"type":"image","mimeType":"image/png","data":"YWJj"}}]}))).await;
        send(upd(json!({"sessionUpdate":"agent_message_chunk",
            "content":{"type":"image","mimeType":"image/jpeg","data":"ZGVm"}})))
        .await;
        send(upd(
            json!({"sessionUpdate":"usage_update","used":1200,"size":1_000_000}),
        ))
        .await;
        send(upd(json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":" Done."}}))).await;
        send(json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"stopReason":"end_turn"}})).await;
        let _ = lines.next_line().await;
    });

    let handle = spawn_with_connection(
        AcpSessionConfig {
            installed: None,
            cwd: std::env::temp_dir().join("hate-acp-test"),
            model: "pro".into(),
            provider_name: "antigravity".into(),
            mcp_servers: json!([]),
            persist: None,
        },
        conn,
        incoming,
    );
    let mut rx = handle.subscribe();
    handle.command(Command::user("fix the bug")).unwrap();

    let mut text = String::new();
    let mut saw = (false, false, false, false);
    let mut images = Vec::new();
    loop {
        match next_event(&mut rx).await {
            Event::ThinkingDelta { text } => {
                assert_eq!(text, "hmm");
                saw.0 = true;
            }
            Event::Delta { text: t } => text.push_str(&t),
            Event::Image { source, call_id } => images.push((source, call_id)),
            Event::ToolStarted { call } => {
                assert_eq!(call.id, "t1");
                assert_eq!(call.name.0, "edit_file");
                assert_eq!(call.args["path"], "/r/main.rs");
            }
            Event::PermissionRequested { request } => {
                assert_eq!(request.call.id, "t1");
                assert_eq!(request.call.name.0, "edit_file");
                saw.1 = true;
                handle
                    .command(Command::RespondPermission {
                        request_id: request.id,
                        decision: "allow".into(),
                    })
                    .unwrap();
            }
            Event::ToolOutput { preview, .. } => {
                assert_eq!(preview, "edited /r/main.rs (1 → 2 lines)");
            }
            Event::ToolFinished { call_id, ok, .. } => {
                assert_eq!(call_id, "t1");
                assert!(ok);
                saw.2 = true;
            }
            Event::ContextUsage { used, size } => {
                assert_eq!((used, size), (1200, 1_000_000));
                saw.3 = true;
            }
            Event::TurnComplete { outcome } => {
                assert_eq!(outcome.outcome, Outcome::Completed);
                assert_eq!(outcome.tool_calls, 1);
                break;
            }
            Event::Failed { reason } => panic!("turn failed: {reason:?}"),
            Event::Error { error } => panic!("error: {error}"),
            _ => {}
        }
    }
    assert_eq!(
        images,
        vec![
            ("data:image/png;base64,YWJj".into(), Some("t1".into())),
            ("data:image/jpeg;base64,ZGVm".into(), None)
        ]
    );
    assert_eq!(text, "On it. Done.");
    assert_eq!(saw, (true, true, true, true));
    assert!(!handle.snapshot().running);
    agent.abort();
}

#[tokio::test]
async fn unauthenticated_agent_points_to_sign_in() {
    let (client_side, agent_side) = duplex(64 * 1024);
    let (cr, cw) = tokio::io::split(client_side);
    let (ar, mut aw) = tokio::io::split(agent_side);
    let (conn, incoming) = Connection::new(cr, cw);

    let agent = tokio::spawn(async move {
        let mut lines = BufReader::new(ar).lines();
        let init: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        aw.write_all(
            format!(
                "{}\n",
                json!({"jsonrpc":"2.0","id":init["id"],"result":{"protocolVersion":1}})
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        let new: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        aw.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":new["id"],"error":{"code":-32000,"message":"Authentication required"}})).as_bytes()).await.unwrap();
        let _ = lines.next_line().await;
    });

    let handle = spawn_with_connection(
        AcpSessionConfig {
            installed: None,
            cwd: std::env::temp_dir().join("hate-acp-test-auth"),
            model: String::new(),
            provider_name: "antigravity".into(),
            mcp_servers: json!([]),
            persist: None,
        },
        conn,
        incoming,
    );
    let mut rx = handle.subscribe();
    handle.command(Command::user("hi")).unwrap();
    loop {
        if let Event::Failed { reason } = next_event(&mut rx).await {
            let msg = format!("{reason:?}");
            assert!(msg.contains("Sign in with Google"), "{msg}");
            break;
        }
    }
    agent.abort();
}
