//! Sprint-05 UAT mirrors: drive the real `rstask-mcp` binary over stdio
//! (UAT U1, U3, U4). One server process per agent.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use rstask_core::{ActorKind, Store};
use serde_json::{json, Value};

struct Agent {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    instructions: String,
}

impl Agent {
    fn start(db: &Path, actor: &str) -> Agent {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rstask-mcp"))
            .arg("--db")
            .arg(db)
            .args(["--actor", actor])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn rstask-mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut agent = Agent {
            child,
            stdin,
            stdout,
            next_id: 0,
            instructions: String::new(),
        };
        let init = agent.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "stdio-test", "version": "0" }
            }),
        );
        agent.instructions = init["instructions"].as_str().unwrap().to_string();
        agent.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        agent
    }

    fn send(&mut self, msg: Value) {
        writeln!(self.stdin, "{msg}").unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let mut line = String::new();
            assert!(
                self.stdout.read_line(&mut line).unwrap() > 0,
                "server closed"
            );
            let msg: Value = serde_json::from_str(&line).unwrap();
            if msg["id"] == id {
                assert!(msg["error"].is_null(), "{method}: {msg}");
                return msg["result"].clone();
            }
        }
    }

    /// Call a tool: `Ok(parsed JSON)` or `Err(tool error text)`.
    fn call(&mut self, name: &str, args: Value) -> Result<Value, String> {
        let result = self.request("tools/call", json!({ "name": name, "arguments": args }));
        let text = result["content"][0]["text"].as_str().unwrap().to_string();
        if result["isError"] == true {
            Err(text)
        } else {
            Ok(serde_json::from_str(&text).unwrap())
        }
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Workspace {
    db: PathBuf,
    alice: String,
    bot: String,
    reviewer: String,
}

async fn workspace() -> Workspace {
    let db = std::env::temp_dir().join(format!("rstask-mcp-{}.sqlite", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let store = Store::open(&db).await.unwrap();
    let mut ids = Vec::new();
    for (kind, name) in [
        (ActorKind::Human, "alice"),
        (ActorKind::Agent, "bot"),
        (ActorKind::Agent, "reviewer"),
    ] {
        ids.push(store.create_actor(kind, name).await.unwrap().id);
    }
    Workspace {
        db,
        alice: ids[0].clone(),
        bot: ids[1].clone(),
        reviewer: ids[2].clone(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn agents_work_and_review_over_mcp() {
    let ws = workspace().await;
    let store = Store::open(&ws.db).await.unwrap();
    let task = store
        .create_task("ship it", "do the thing", &ws.alice)
        .await
        .unwrap();

    let mut bot = Agent::start(&ws.db, &ws.bot);
    let mut reviewer = Agent::start(&ws.db, &ws.reviewer);

    // U1 — identity and tool surface.
    assert!(bot.instructions.contains(&ws.bot), "{}", bot.instructions);
    let tools = bot.request("tools/list", json!({}));
    let mut names: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "board_state",
            "claim_next",
            "task_approve",
            "task_block",
            "task_claim",
            "task_complete",
            "task_create",
            "task_get",
            "task_protect",
            "task_reject",
            "task_release",
            "task_renew",
            "task_unblock",
            "task_update",
            "tasks_list"
        ]
    );
    for tool in tools["tools"].as_array().unwrap() {
        assert!(
            tool["inputSchema"]["properties"]["actor"].is_null(),
            "no tool takes an actor: {tool}"
        );
    }

    // U3 — worker loop.
    let claimed = bot.call("claim_next", json!({})).unwrap();
    assert_eq!(claimed["id"], task.id.as_str());
    assert_eq!(claimed["claim"]["actor_id"], ws.bot.as_str());
    assert_eq!(bot.call("claim_next", json!({})).unwrap(), Value::Null);
    bot.call("task_renew", json!({ "id": task.id, "ttl_secs": 1800 }))
        .unwrap();
    bot.call("task_update", json!({ "id": task.id, "body_md": "notes" }))
        .unwrap();

    // U4 — the gate holds for a non-holder.
    for (tool, args) in [
        ("task_update", json!({ "id": task.id, "title": "hijack" })),
        ("task_complete", json!({ "id": task.id, "summary": "mine" })),
        ("task_release", json!({ "id": task.id })),
    ] {
        let err = reviewer.call(tool, args).unwrap_err();
        assert!(err.contains("already claimed"), "{tool}: {err}");
    }

    bot.call(
        "task_complete",
        json!({ "id": task.id, "summary": "did it", "refs": ["abc123"], "evidence": "green" }),
    )
    .unwrap();
    let submitted = bot.call("task_get", json!({ "id": task.id })).unwrap();
    let first = submitted["submission"]["id"].clone();
    let err = bot
        .call(
            "task_approve",
            json!({ "id": task.id, "submission": first }),
        )
        .unwrap_err();
    assert!(err.contains("own submission"), "{err}");

    // U3 — reviewer loop: reject with a reason, worker sees it, second round passes.
    let in_review = reviewer
        .call("tasks_list", json!({ "status": "review" }))
        .unwrap();
    assert_eq!(in_review[0]["id"], task.id.as_str());
    let seen = reviewer.call("task_get", json!({ "id": task.id })).unwrap();
    assert_eq!(seen["submission"]["summary"], "did it");
    assert_eq!(seen["submission"]["by"], ws.bot.as_str());
    reviewer
        .call(
            "task_reject",
            json!({ "id": task.id, "reason": "add a test" }),
        )
        .unwrap();

    let again = bot.call("claim_next", json!({})).unwrap();
    assert_eq!(again["rejection"]["reason"], "add a test");
    bot.call(
        "task_complete",
        json!({ "id": task.id, "summary": "added the test" }),
    )
    .unwrap();
    // An approval names what was reviewed: the first submission is stale now.
    let err = reviewer
        .call(
            "task_approve",
            json!({ "id": task.id, "submission": first }),
        )
        .unwrap_err();
    assert!(err.contains("resubmitted"), "{err}");
    let second =
        reviewer.call("task_get", json!({ "id": task.id })).unwrap()["submission"]["id"].clone();
    let done = reviewer
        .call(
            "task_approve",
            json!({ "id": task.id, "submission": second }),
        )
        .unwrap();
    assert_eq!(done["status"], "done");

    let board = bot.call("board_state", json!({})).unwrap();
    assert_eq!(board["done"][0]["id"], task.id.as_str());
    let kinds: Vec<String> = store
        .list_events(&task.id)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert_eq!(
        kinds,
        [
            "task.created",
            "task.claimed",
            "task.renewed",
            "task.updated",
            "task.submitted",
            "task.changes_requested",
            "task.claimed",
            "task.submitted",
            "task.approved"
        ]
    );
    drop((bot, reviewer));
    let _ = std::fs::remove_file(&ws.db);
}

#[test]
fn unknown_actor_fails_before_serving() {
    let db = std::env::temp_dir().join(format!("rstask-mcp-noactor-{}.sqlite", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_rstask-mcp"))
        .arg("--db")
        .arg(&db)
        .args(["--actor", "nobody"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty(), "nothing on the protocol channel");
    assert!(String::from_utf8_lossy(&out.stderr).contains("actor not found"));
    let _ = std::fs::remove_file(&db);
}
