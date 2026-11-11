//! Behavioral tests for the actor/claim/workflow semantics.

use std::time::Duration;

use rstask_core::{ActorKind, Error, Store, Submission, TaskStatus};

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
    assert!(matches!(err, Error::ClaimConflict { .. }));
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
