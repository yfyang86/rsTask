//! Behavioral tests for the actor/claim/workflow semantics.

use std::time::Duration;

use rstask_core::{ActorKind, Error, Role, Store, Submission, TaskStatus};

async fn setup() -> (Store, String) {
    let store = Store::open_in_memory().await.unwrap();
    let human = store.create_actor(ActorKind::Human, "alice").await.unwrap();
    (store, human.id)
}

#[tokio::test]
async fn create_and_list_task() {
    let (store, alice) = setup().await;
    let task = store
        .create_task("write spec", "body", &alice)
        .await
        .unwrap();
    assert_eq!(task.status, TaskStatus::Queue);

    let listed = store.list_tasks(None).await.unwrap();
    assert_eq!(listed.len(), 1);

    let filtered = store.list_tasks(Some(TaskStatus::Done)).await.unwrap();
    assert!(filtered.is_empty());

    // Event log records creation.
    let events = store.list_events(&task.id).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "task.created");
}

#[tokio::test]
async fn claim_is_exclusive_and_moves_to_work() {
    let (store, alice) = setup().await;
    let task = store.create_task("t", "", &alice).await.unwrap();
    let agent = store.create_actor(ActorKind::Agent, "bot").await.unwrap();

    store
        .claim_task(&task.id, &alice, Duration::from_secs(60))
        .await
        .unwrap();

    // Second claim while active → conflict naming the holder.
    let err = store
        .claim_task(&task.id, &agent.id, Duration::from_secs(60))
        .await
        .unwrap_err();
    match err {
        Error::ClaimConflict { actor, .. } => assert_eq!(actor, alice),
        other => panic!("expected ClaimConflict, got {other:?}"),
    }

    assert_eq!(
        store.get_task(&task.id).await.unwrap().status,
        TaskStatus::Work
    );
}

#[tokio::test]
async fn expired_claim_returns_task_to_pool() {
    let (store, alice) = setup().await;
    let task = store.create_task("t", "", &alice).await.unwrap();
    let agent = store.create_actor(ActorKind::Agent, "bot").await.unwrap();

    // Claim with a zero TTL: expires immediately (no heartbeat).
    store
        .claim_task(&task.id, &alice, Duration::ZERO)
        .await
        .unwrap();

    // Next claim attempt sweeps the stale claim and succeeds for someone else.
    store
        .claim_task(&task.id, &agent.id, Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(
        store.get_task(&task.id).await.unwrap().status,
        TaskStatus::Work
    );

    // The stale claim is recorded as expired in the log.
    let events = store.list_events(&task.id).await.unwrap();
    assert!(events.iter().any(|e| e.kind == "task.claimed"));
}

#[tokio::test]
async fn workflow_gate_enforced() {
    let (store, alice) = setup().await;
    let task = store.create_task("t", "", &alice).await.unwrap();
    let agent = store.create_actor(ActorKind::Agent, "bot").await.unwrap();

    // Cannot approve directly from queue.
    let err = store.approve(&task.id, &alice).await.unwrap_err();
    assert!(matches!(err, Error::InvalidTransition { .. }));

    // Cannot submit without claiming first (queue → review is illegal).
    let err = store
        .submit_for_review(&task.id, &agent.id, &Submission::default())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidTransition { .. }));

    // The happy path: claim → work → submit → review → approve → done.
    store
        .claim_task(&task.id, &agent.id, Duration::from_secs(60))
        .await
        .unwrap();
    store
        .submit_for_review(&task.id, &agent.id, &Submission::default())
        .await
        .unwrap();
    assert_eq!(
        store.get_task(&task.id).await.unwrap().status,
        TaskStatus::Review
    );

    store.approve(&task.id, &alice).await.unwrap();
    assert_eq!(
        store.get_task(&task.id).await.unwrap().status,
        TaskStatus::Done
    );

    // Changes can no longer be requested once done.
    let err = store
        .request_changes(&task.id, &alice, "rework")
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidTransition { .. }));
}

#[tokio::test]
async fn request_changes_sends_back_to_queue() {
    let (store, alice) = setup().await;
    let task = store.create_task("t", "", &alice).await.unwrap();
    store
        .claim_task(&task.id, &alice, Duration::from_secs(60))
        .await
        .unwrap();
    store
        .submit_for_review(&task.id, &alice, &Submission::default())
        .await
        .unwrap();
    store
        .request_changes(&task.id, &alice, "rework")
        .await
        .unwrap();
    assert_eq!(
        store.get_task(&task.id).await.unwrap().status,
        TaskStatus::Queue
    );

    // Can be re-claimed after rework.
    store
        .claim_task(&task.id, &alice, Duration::from_secs(60))
        .await
        .unwrap();
}

#[tokio::test]
async fn force_release_frees_stale_claim() {
    let (store, alice) = setup().await;
    let task = store.create_task("t", "", &alice).await.unwrap();
    let agent = store.create_actor(ActorKind::Agent, "bot").await.unwrap();

    store
        .claim_task(&task.id, &agent.id, Duration::from_secs(3600))
        .await
        .unwrap();

    // Alice force-releases the agent's stale claim.
    store.release_claim(&task.id, &alice, true).await.unwrap();
    assert_eq!(
        store.get_task(&task.id).await.unwrap().status,
        TaskStatus::Queue
    );

    // Task is claimable again.
    store
        .claim_task(&task.id, &alice, Duration::from_secs(60))
        .await
        .unwrap();
}

#[tokio::test]
async fn release_without_active_claim_fails() {
    let (store, alice) = setup().await;
    let task = store.create_task("t", "", &alice).await.unwrap();
    let err = store
        .release_claim(&task.id, &alice, false)
        .await
        .unwrap_err();
    // Nobody holds it: there is nothing to release.
    assert!(
        matches!(err, Error::InvalidTransition { .. }),
        "got {err:?}"
    );
}

#[tokio::test]
async fn board_state_groups_by_status() {
    let (store, alice) = setup().await;
    let a = store.create_task("a", "", &alice).await.unwrap();
    let b = store.create_task("b", "", &alice).await.unwrap();
    store
        .claim_task(&b.id, &alice, Duration::from_secs(60))
        .await
        .unwrap();

    let board = store.board_state().await.unwrap();
    let counts: Vec<usize> = board.iter().map(|(_, tasks)| tasks.len()).collect();
    assert_eq!(counts, vec![1, 1, 0, 0]);
    assert_eq!(board[0].1[0].id, a.id);
    assert_eq!(board[1].1[0].id, b.id);
}

#[tokio::test]
async fn unknown_actor_rejected() {
    let store = Store::open_in_memory().await.unwrap();
    let err = store
        .create_task("t", "", "no-such-actor")
        .await
        .unwrap_err();
    assert!(matches!(err, Error::ActorNotFound(_)));
}

// ---- Sprint 03: gate correctness (UAT U3, U4) ----

#[tokio::test]
async fn agent_cannot_approve_own_submission() {
    let (store, alice) = setup().await;
    let task = store.create_task("t", "", &alice).await.unwrap();
    let bot = store.create_actor(ActorKind::Agent, "bot").await.unwrap();
    let peer = store.create_actor(ActorKind::Agent, "peer").await.unwrap();
    store
        .claim_task(&task.id, &bot.id, Duration::from_secs(60))
        .await
        .unwrap();
    store
        .submit_for_review(&task.id, &bot.id, &Submission::default())
        .await
        .unwrap();

    let err = store.approve(&task.id, &bot.id).await.unwrap_err();
    assert!(matches!(err, Error::NotPermitted { .. }), "got {err:?}");
    assert_eq!(
        store.get_task(&task.id).await.unwrap().status,
        TaskStatus::Review
    );
    // The refusal leaves no trace in the log.
    let events = store.list_events(&task.id).await.unwrap();
    assert!(!events.iter().any(|e| e.kind == "task.approved"));

    // A different agent may review it.
    store.approve(&task.id, &peer.id).await.unwrap();
    assert_eq!(
        store.get_task(&task.id).await.unwrap().status,
        TaskStatus::Done
    );
}

#[tokio::test]
async fn only_humans_force_release() {
    let (store, alice) = setup().await;
    let task = store.create_task("t", "", &alice).await.unwrap();
    let bot = store.create_actor(ActorKind::Agent, "bot").await.unwrap();
    let peer = store.create_actor(ActorKind::Agent, "peer").await.unwrap();
    store
        .claim_task(&task.id, &bot.id, Duration::from_secs(3600))
        .await
        .unwrap();

    // An agent can neither force another's claim nor plainly release it.
    let err = store
        .release_claim(&task.id, &peer.id, true)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotPermitted { .. }), "got {err:?}");
    let err = store
        .release_claim(&task.id, &peer.id, false)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::ClaimConflict { .. }), "got {err:?}");
    assert_eq!(
        store.get_task(&task.id).await.unwrap().status,
        TaskStatus::Work
    );

    // The refusal names the real holder, not the caller.
    assert!(
        matches!(&err, Error::ClaimConflict { actor, .. } if *actor == bot.id),
        "got {err:?}"
    );

    // The human's forced release is signed by the human and names the claim.
    store.release_claim(&task.id, &alice, true).await.unwrap();
    let events = store.list_events(&task.id).await.unwrap();
    let forced = events
        .iter()
        .find(|e| e.kind == "claim.force_released")
        .expect("force event logged");
    assert_eq!(forced.actor_id.as_deref(), Some(alice.as_str()));
    assert!(
        forced.payload.contains("claim_id\":\""),
        "{}",
        forced.payload
    );
}

// ---- Sprint 04: claim liveness and handoff (UAT U1–U4) ----

async fn active_claim(store: &Store, task_id: &str) -> Option<rstask_core::Claim> {
    sqlx::query_as("SELECT * FROM claims WHERE task_id = ? AND released_at IS NULL")
        .bind(task_id)
        .fetch_optional(store.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn renew_extends_claim_and_logs_event() {
    let (store, alice) = setup().await;
    let task = store.create_task("t", "", &alice).await.unwrap();
    let bot = store.create_actor(ActorKind::Agent, "bot").await.unwrap();
    let rival = store.create_actor(ActorKind::Agent, "rival").await.unwrap();
    let claim = store
        .claim_task(&task.id, &bot.id, Duration::from_secs(60))
        .await
        .unwrap();

    // Only the holder renews.
    let err = store
        .renew_claim(&task.id, &rival.id, Duration::from_secs(3600))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::ClaimConflict { .. }), "got {err:?}");

    let renewed = store
        .renew_claim(&task.id, &bot.id, Duration::from_secs(3600))
        .await
        .unwrap();
    assert_eq!(renewed.id, claim.id);
    assert!(renewed.expires_at > claim.expires_at);
    assert_eq!(
        active_claim(&store, &task.id).await.unwrap().expires_at,
        renewed.expires_at
    );
    let events = store.list_events(&task.id).await.unwrap();
    let ev = events
        .iter()
        .find(|e| e.kind == "task.renewed")
        .expect("renewal logged");
    assert!(ev.payload.contains(&claim.id), "{}", ev.payload);
}

#[tokio::test]
async fn stale_claim_is_swept_on_read_and_cannot_act() {
    let (store, alice) = setup().await;
    let task = store.create_task("t", "", &alice).await.unwrap();
    let bot = store.create_actor(ActorKind::Agent, "bot").await.unwrap();
    store
        .claim_task(&task.id, &bot.id, Duration::from_secs(0))
        .await
        .unwrap();

    // A plain read — no claim attempt — returns the task to the pool.
    let board = store.board_state().await.unwrap();
    assert_eq!(board[0].1.len(), 1, "task is back in queue: {board:?}");
    assert!(active_claim(&store, &task.id).await.is_none());

    let err = store
        .renew_claim(&task.id, &bot.id, Duration::from_secs(60))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::ClaimExpired(_)), "got {err:?}");
    let err = store
        .submit_for_review(&task.id, &bot.id, &Submission::default())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::ClaimExpired(_)), "got {err:?}");
    let events = store.list_events(&task.id).await.unwrap();
    assert_eq!(events.len(), 2, "only created + claimed are logged");
}

#[tokio::test]
async fn only_the_holder_submits() {
    let (store, alice) = setup().await;
    let task = store.create_task("t", "", &alice).await.unwrap();
    let bot = store.create_actor(ActorKind::Agent, "bot").await.unwrap();
    let rival = store.create_actor(ActorKind::Agent, "rival").await.unwrap();
    store
        .claim_task(&task.id, &bot.id, Duration::from_secs(60))
        .await
        .unwrap();
    let err = store
        .submit_for_review(&task.id, &rival.id, &Submission::default())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::ClaimConflict { .. }), "got {err:?}");
    assert_eq!(
        store.get_task(&task.id).await.unwrap().status,
        TaskStatus::Work
    );
}

#[tokio::test]
async fn submission_and_reason_are_logged() {
    let (store, alice) = setup().await;
    let task = store.create_task("t", "", &alice).await.unwrap();
    let bot = store.create_actor(ActorKind::Agent, "bot").await.unwrap();
    store
        .claim_task(&task.id, &bot.id, Duration::from_secs(60))
        .await
        .unwrap();
    let submission = Submission {
        summary: "fixed the fold".into(),
        refs: vec!["c265568".into(), "PR 12".into()],
        evidence: "27/27 green".into(),
    };
    store
        .submit_for_review(&task.id, &bot.id, &submission)
        .await
        .unwrap();
    store
        .request_changes(&task.id, &alice, "missing a test")
        .await
        .unwrap();

    let events = store.list_events(&task.id).await.unwrap();
    let payload = |kind: &str| -> serde_json::Value {
        let ev = events.iter().find(|e| e.kind == kind).unwrap();
        serde_json::from_str(&ev.payload).unwrap()
    };
    let submitted = payload("task.submitted");
    assert_eq!(submitted["summary"], "fixed the fold");
    assert_eq!(submitted["refs"], serde_json::json!(["c265568", "PR 12"]));
    assert_eq!(submitted["evidence"], "27/27 green");
    assert_eq!(
        payload("task.changes_requested")["reason"],
        "missing a test"
    );
}

// ---- Sprint 05: agent interface (UAT U2–U4) ----

#[tokio::test]
async fn claim_next_takes_highest_priority_then_oldest() {
    let (store, alice) = setup().await;
    let bot = store.create_actor(ActorKind::Agent, "bot").await.unwrap();
    let ttl = Duration::from_secs(60);
    assert!(store.claim_next(&bot.id, ttl).await.unwrap().is_none());

    let old = store.create_task("old", "", &alice).await.unwrap();
    let _new = store.create_task("new", "", &alice).await.unwrap();
    let urgent = store.create_task("urgent", "", &alice).await.unwrap();
    store
        .update_task(&urgent.id, None, None, Some(5), &alice)
        .await
        .unwrap();

    let first = store.claim_next(&bot.id, ttl).await.unwrap().unwrap();
    assert_eq!(first.task_id, urgent.id);
    assert_eq!(first.actor_id, bot.id);
    let second = store.claim_next(&bot.id, ttl).await.unwrap().unwrap();
    assert_eq!(second.task_id, old.id);
    assert_eq!(
        store.get_task(&urgent.id).await.unwrap().status,
        TaskStatus::Work
    );
    let events = store.list_events(&urgent.id).await.unwrap();
    assert!(events.iter().any(|e| e.kind == "task.claimed"));
}

#[tokio::test]
async fn claim_next_race_hands_out_distinct_tasks() {
    let path = std::env::temp_dir().join(format!("rstask-race-{}.sqlite", uuid::Uuid::new_v4()));
    let store = std::sync::Arc::new(Store::open(&path).await.unwrap());
    let alice = store.create_actor(ActorKind::Human, "alice").await.unwrap();
    for i in 0..3 {
        store
            .create_task(&format!("t{i}"), "", &alice.id)
            .await
            .unwrap();
    }
    let mut agents = Vec::new();
    for i in 0..8 {
        let name = format!("bot{i}");
        agents.push(store.create_actor(ActorKind::Agent, &name).await.unwrap());
    }
    let mut handles = Vec::new();
    for agent in agents {
        let store = store.clone();
        handles.push(tokio::spawn(async move {
            store
                .claim_next(&agent.id, Duration::from_secs(60))
                .await
                .unwrap()
        }));
    }
    let mut won = Vec::new();
    for h in handles {
        if let Some(claim) = h.await.unwrap() {
            won.push(claim.task_id);
        }
    }
    won.sort();
    won.dedup();
    assert_eq!(won.len(), 3, "three distinct tasks handed out");
    assert!(store
        .list_tasks(Some(TaskStatus::Queue))
        .await
        .unwrap()
        .is_empty());
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn update_needs_the_holder_while_claimed() {
    let (store, alice) = setup().await;
    let task = store.create_task("t", "", &alice).await.unwrap();
    let bot = store.create_actor(ActorKind::Agent, "bot").await.unwrap();
    let rival = store.create_actor(ActorKind::Agent, "rival").await.unwrap();

    let err = store
        .update_task("no-such-task", Some("x"), None, None, &alice)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::TaskNotFound(_)), "got {err:?}");

    // Unclaimed: anyone may edit (planning).
    store
        .update_task(&task.id, None, Some("plan"), None, &rival.id)
        .await
        .unwrap();
    store
        .claim_task(&task.id, &bot.id, Duration::from_secs(60))
        .await
        .unwrap();
    let err = store
        .update_task(&task.id, None, Some("hijack"), None, &rival.id)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::ClaimConflict { .. }), "got {err:?}");
    let updated = store
        .update_task(&task.id, None, Some("notes"), None, &bot.id)
        .await
        .unwrap();
    assert_eq!(updated.body_md, "notes");
}

#[tokio::test]
async fn task_detail_carries_claim_and_handoff() {
    let (store, alice) = setup().await;
    let task = store.create_task("t", "", &alice).await.unwrap();
    let bot = store.create_actor(ActorKind::Agent, "bot").await.unwrap();
    let ttl = Duration::from_secs(60);

    let d = store.task_detail(&task.id).await.unwrap();
    assert!(d.claim.is_none() && d.submission.is_none() && d.rejection.is_none());

    store.claim_task(&task.id, &bot.id, ttl).await.unwrap();
    let d = store.task_detail(&task.id).await.unwrap();
    assert_eq!(d.claim.unwrap().actor_id, bot.id);

    let submission = Submission {
        summary: "done".into(),
        refs: vec!["abc123".into()],
        evidence: "green".into(),
    };
    store
        .submit_for_review(&task.id, &bot.id, &submission)
        .await
        .unwrap();
    store
        .request_changes(&task.id, &alice, "add a test")
        .await
        .unwrap();

    let d = store.task_detail(&task.id).await.unwrap();
    assert_eq!(d.task.status, TaskStatus::Queue);
    assert!(d.claim.is_none());
    let s = d.submission.unwrap();
    assert_eq!(
        (s.by.as_str(), &s.submission),
        (bot.id.as_str(), &submission)
    );
    let r = d.rejection.unwrap();
    assert_eq!(
        (r.by.as_str(), r.reason.as_str()),
        (alice.as_str(), "add a test")
    );
}

#[tokio::test]
async fn concurrent_writers_never_see_a_locked_database() {
    let path = std::env::temp_dir().join(format!("rstask-busy-{}.sqlite", uuid::Uuid::new_v4()));
    let store = std::sync::Arc::new(Store::open(&path).await.unwrap());
    let alice = store.create_actor(ActorKind::Human, "alice").await.unwrap();
    let bot = store.create_actor(ActorKind::Agent, "bot").await.unwrap();
    let ttl = Duration::from_secs(60);
    let mut in_review = Vec::new();
    for i in 0..8 {
        let t = store
            .create_task(&format!("r{i}"), "", &alice.id)
            .await
            .unwrap();
        store.claim_task(&t.id, &bot.id, ttl).await.unwrap();
        store
            .submit_for_review(&t.id, &bot.id, &Submission::default())
            .await
            .unwrap();
        in_review.push(t.id);
        store
            .create_task(&format!("q{i}"), "", &alice.id)
            .await
            .unwrap();
    }
    // Reviewers approve while workers claim: read-then-write transactions
    // interleaved with writers.
    let mut handles = Vec::new();
    for id in in_review {
        let (s, alice, bot) = (store.clone(), alice.id.clone(), bot.id.clone());
        handles.push(tokio::spawn(async move {
            s.approve(&id, &alice).await.map(|_| ())
        }));
        let s = store.clone();
        handles.push(tokio::spawn(async move {
            s.claim_next(&bot, ttl).await.map(|_| ())
        }));
    }
    for h in handles {
        h.await.unwrap().expect("no writer is turned away");
    }
    assert_eq!(
        store
            .list_tasks(Some(TaskStatus::Done))
            .await
            .unwrap()
            .len(),
        8
    );
    let _ = std::fs::remove_file(&path);
}

// ---- Sprint 06: dependencies and roles (UAT U1–U4) ----

async fn finish(store: &Store, task_id: &str, worker: &str, reviewer: &str) {
    store
        .submit_for_review(task_id, worker, &Submission::default())
        .await
        .unwrap();
    store.approve(task_id, reviewer).await.unwrap();
}

#[tokio::test]
async fn blocked_task_waits_for_its_blocker() {
    let (store, alice) = setup().await;
    let bot = store.create_actor(ActorKind::Agent, "bot").await.unwrap();
    let ttl = Duration::from_secs(60);
    let deploy = store.create_task("deploy", "", &alice).await.unwrap();
    let build = store.create_task("build", "", &alice).await.unwrap();
    store
        .update_task(&deploy.id, None, None, Some(9), &alice)
        .await
        .unwrap();
    store
        .add_dependency(&deploy.id, &build.id, &alice)
        .await
        .unwrap();

    let err = store
        .claim_task(&deploy.id, &bot.id, ttl)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Blocked { .. }), "got {err:?}");
    let d = store.task_detail(&deploy.id).await.unwrap();
    assert_eq!(d.blocked_by, vec![build.id.clone()]);
    assert_eq!(d.waiting_on, vec![build.id.clone()]);

    // The lower-priority blocker is handed out first.
    let first = store.claim_next(&bot.id, ttl).await.unwrap().unwrap();
    assert_eq!(first.task_id, build.id);
    assert!(store.claim_next(&bot.id, ttl).await.unwrap().is_none());
    finish(&store, &build.id, &bot.id, &alice).await;

    let next = store.claim_next(&bot.id, ttl).await.unwrap().unwrap();
    assert_eq!(next.task_id, deploy.id);
    let d = store.task_detail(&deploy.id).await.unwrap();
    assert_eq!(d.blocked_by, vec![build.id.clone()]);
    assert!(d.waiting_on.is_empty());
}

#[tokio::test]
async fn dependencies_must_exist_and_not_cycle() {
    let (store, alice) = setup().await;
    let a = store.create_task("a", "", &alice).await.unwrap();
    let b = store.create_task("b", "", &alice).await.unwrap();
    let c = store.create_task("c", "", &alice).await.unwrap();

    let err = store
        .add_dependency(&a.id, &a.id, &alice)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::DependencyCycle { .. }), "got {err:?}");
    let err = store
        .add_dependency(&a.id, "no-such-task", &alice)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::TaskNotFound(_)), "got {err:?}");

    store.add_dependency(&a.id, &b.id, &alice).await.unwrap();
    store.add_dependency(&b.id, &c.id, &alice).await.unwrap();
    let err = store
        .add_dependency(&c.id, &a.id, &alice)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::DependencyCycle { .. }), "got {err:?}");
    assert_eq!(store.list_events(&c.id).await.unwrap().len(), 1);

    store.remove_dependency(&a.id, &b.id, &alice).await.unwrap();
    assert!(store
        .task_detail(&a.id)
        .await
        .unwrap()
        .blocked_by
        .is_empty());
    store
        .claim_task(&a.id, &alice, Duration::from_secs(60))
        .await
        .unwrap();
}

#[tokio::test]
async fn parent_waits_for_its_subtasks() {
    let (store, alice) = setup().await;
    let bot = store.create_actor(ActorKind::Agent, "bot").await.unwrap();
    let ttl = Duration::from_secs(60);
    let epic = store.create_task("epic", "", &alice).await.unwrap();
    let s1 = store
        .create_subtask(&epic.id, "part 1", "", &alice)
        .await
        .unwrap();
    let s2 = store
        .create_subtask(&epic.id, "part 2", "", &alice)
        .await
        .unwrap();
    assert_eq!(s1.parent_id.as_deref(), Some(epic.id.as_str()));
    let err = store
        .create_subtask("no-such-task", "orphan", "", &alice)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::TaskNotFound(_)), "got {err:?}");

    let d = store.task_detail(&epic.id).await.unwrap();
    assert_eq!(d.children, vec![s1.id.clone(), s2.id.clone()]);
    assert_eq!(d.waiting_on, vec![s1.id.clone(), s2.id.clone()]);

    // The epic is older, but its parts go first.
    for part in [&s1, &s2] {
        let claim = store.claim_next(&bot.id, ttl).await.unwrap().unwrap();
        assert_eq!(claim.task_id, part.id);
        finish(&store, &part.id, &bot.id, &alice).await;
    }
    let claim = store.claim_next(&bot.id, ttl).await.unwrap().unwrap();
    assert_eq!(claim.task_id, epic.id);
}

#[tokio::test]
async fn roles_gate_planning_working_and_reviewing() {
    let (store, alice) = setup().await;
    let ttl = Duration::from_secs(60);
    let mut agent = Vec::new();
    for role in [Role::Planner, Role::Worker, Role::Reviewer] {
        let a = store
            .create_actor_with_roles(ActorKind::Agent, &role.to_string(), &[role])
            .await
            .unwrap();
        assert_eq!(store.get_actor(&a.id).await.unwrap().roles.0, vec![role]);
        agent.push(a.id);
    }
    let (planner, worker, reviewer) = (&agent[0], &agent[1], &agent[2]);
    let denied = |r: Result<(), Error>| {
        let err = r.unwrap_err();
        assert!(matches!(err, Error::NotPermitted { .. }), "got {err:?}");
    };

    // Planning.
    denied(store.create_task("t", "", worker).await.map(|_| ()));
    denied(store.create_task("t", "", reviewer).await.map(|_| ()));
    let task = store.create_task("t", "", planner).await.unwrap();
    let other = store.create_task("other", "", planner).await.unwrap();
    denied(store.add_dependency(&other.id, &task.id, worker).await);
    denied(
        store
            .update_task(&task.id, Some("x"), None, None, worker)
            .await
            .map(|_| ()),
    );
    store
        .add_dependency(&other.id, &task.id, planner)
        .await
        .unwrap();

    // Working.
    denied(store.claim_task(&task.id, planner, ttl).await.map(|_| ()));
    denied(store.claim_next(reviewer, ttl).await.map(|_| ()));
    let claim = store.claim_next(worker, ttl).await.unwrap().unwrap();
    assert_eq!(claim.task_id, task.id);
    // The holder may edit its own task without being a planner.
    store
        .update_task(&task.id, None, Some("notes"), None, worker)
        .await
        .unwrap();
    store
        .submit_for_review(&task.id, worker, &Submission::default())
        .await
        .unwrap();

    // Reviewing.
    denied(store.approve(&task.id, worker).await.map(|_| ()));
    denied(
        store
            .request_changes(&task.id, planner, "no")
            .await
            .map(|_| ()),
    );
    store.approve(&task.id, reviewer).await.unwrap();

    // No roles given: every role (alice and the README quick start).
    assert_eq!(store.get_actor(&alice).await.unwrap().roles.0.len(), 3);
}
