//! Behavioral tests for the actor/claim/workflow semantics.

use std::time::Duration;

use rstask_core::{ActorKind, Error, Store, TaskStatus};

async fn setup() -> (Store, String) {
    let store = Store::open_in_memory().await.unwrap();
    let human = store.create_actor(ActorKind::Human, "alice").await.unwrap();
    (store, human.id)
}

#[tokio::test]
async fn create_and_list_task() {
    let (store, alice) = setup().await;
    let task = store.create_task("write spec", "body", &alice).await.unwrap();
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

    assert_eq!(store.get_task(&task.id).await.unwrap().status, TaskStatus::Work);
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
    assert_eq!(store.get_task(&task.id).await.unwrap().status, TaskStatus::Work);

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
    let err = store.submit_for_review(&task.id, &agent.id).await.unwrap_err();
    assert!(matches!(err, Error::InvalidTransition { .. }));

    // The happy path: claim → work → submit → review → approve → done.
    store
        .claim_task(&task.id, &agent.id, Duration::from_secs(60))
        .await
        .unwrap();
    store.submit_for_review(&task.id, &agent.id).await.unwrap();
    assert_eq!(store.get_task(&task.id).await.unwrap().status, TaskStatus::Review);

    store.approve(&task.id, &alice).await.unwrap();
    assert_eq!(store.get_task(&task.id).await.unwrap().status, TaskStatus::Done);

    // Changes can no longer be requested once done.
    let err = store.request_changes(&task.id, &alice).await.unwrap_err();
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
    store.submit_for_review(&task.id, &alice).await.unwrap();
    store.request_changes(&task.id, &alice).await.unwrap();
    assert_eq!(store.get_task(&task.id).await.unwrap().status, TaskStatus::Queue);

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
    store.release_claim(&task.id, &agent.id, true).await.unwrap();
    assert_eq!(store.get_task(&task.id).await.unwrap().status, TaskStatus::Queue);

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
    let err = store.release_claim(&task.id, &alice, false).await.unwrap_err();
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
