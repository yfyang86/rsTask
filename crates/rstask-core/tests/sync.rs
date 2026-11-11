//! Sprint-01 UAT mirrors: two stores, divergent logs, convergence (UAT U1–U5).

use std::time::Duration;

use rstask_core::sync::{decode_bundle, encode_bundle};
use rstask_core::{ActorKind, Store, Submission, TaskStatus};

async fn store() -> Store {
    Store::open_in_memory().await.expect("open")
}

async fn exchange(a: &Store, b: &Store) {
    let bundle = a.export_bundle().await.unwrap();
    b.receive_bundle(&decode_bundle(&encode_bundle(&bundle)).unwrap())
        .await
        .unwrap();
}

/// An actor provisioned on `a` (signed by its root) whose key is then
/// carried to `b`'s key directory — how an actor comes to work on a second
/// replica (PRD-v3 R1, R3).
async fn provision(a: &Store, b: &Store, kind: ActorKind, name: &str) -> rstask_core::Actor {
    let actor = a.create_actor(kind, name).await.unwrap();
    exchange(a, b).await;
    b.import_key(&actor.id, &a.export_key(&actor.id).unwrap())
        .await
        .unwrap();
    actor
}

async fn boards_equal(a: &Store, b: &Store) -> bool {
    let ba = a.board_state().await.unwrap();
    let bb = b.board_state().await.unwrap();
    ba == bb
}

async fn active_claims(s: &Store, task_id: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT actor_id FROM claims WHERE task_id = ? AND released_at IS NULL
         ORDER BY actor_id",
    )
    .bind(task_id)
    .fetch_all(s.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn u1_one_way_export_import() {
    let a = store().await;
    let alice = a.create_actor(ActorKind::Human, "alice").await.unwrap();
    let task = a.create_task("hello", "", &alice.id).await.unwrap();
    a.claim_task(&task.id, &alice.id, Duration::from_secs(900))
        .await
        .unwrap();
    a.release_claim(&task.id, &alice.id, false).await.unwrap();

    let b = store().await;
    exchange(&a, &b).await;

    let tasks = b.list_tasks(None).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "hello");
    assert_eq!(tasks[0].status, TaskStatus::Queue);
    // The actor directory came along: B resolves the actor.
    b.get_actor(&alice.id).await.unwrap();
    // And B's log shows the full chain.
    let events = b.list_events(&task.id).await.unwrap();
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(
        kinds,
        ["task.created", "task.claimed", "claim.released"],
        "event kinds in lamport order: {kinds:?}"
    );
}

#[tokio::test]
async fn u2_two_way_convergence_and_idempotent_resync() {
    let a = store().await;
    let alice = a.create_actor(ActorKind::Human, "alice").await.unwrap();
    let task = a.create_task("shared", "", &alice.id).await.unwrap();

    let b = store().await;
    let bob = provision(&a, &b, ActorKind::Agent, "bob").await;
    exchange(&a, &b).await;
    exchange(&b, &a).await; // common history

    // Diverge: A approves after submit by alice; B creates a second task.
    a.claim_task(&task.id, &alice.id, Duration::from_secs(900))
        .await
        .unwrap();
    a.submit_for_review(&task.id, &alice.id, &Submission::default())
        .await
        .unwrap();
    a.approve(&task.id, &alice.id, None).await.unwrap();

    b.create_task("from b", "", &bob.id).await.unwrap();

    exchange(&a, &b).await;
    exchange(&b, &a).await;

    assert!(
        boards_equal(&a, &b).await,
        "boards converge\na: {:?}\nb: {:?}",
        a.list_tasks(None).await.unwrap(),
        b.list_tasks(None).await.unwrap()
    );
    let ha: Vec<String> = a.heads().await.unwrap().iter().map(hex::encode).collect();
    let hb: Vec<String> = b.heads().await.unwrap().iter().map(hex::encode).collect();
    assert_eq!(ha, hb, "heads converge");

    // Second exchange moves nothing.
    let m = {
        let bundle = b.export_bundle().await.unwrap();
        a.receive_bundle(&bundle).await.unwrap()
    };
    assert_eq!(m.added, 0);
    assert_eq!(m.pending, 0);
    assert_eq!(m.rejected, 0);
}

#[tokio::test]
async fn u3_concurrent_claims_pick_same_winner() {
    let a = store().await;
    let alice = a.create_actor(ActorKind::Human, "alice").await.unwrap();
    let task = a.create_task("contested", "", &alice.id).await.unwrap();

    let b = store().await;
    let y = provision(&a, &b, ActorKind::Agent, "y").await;
    exchange(&a, &b).await;
    exchange(&b, &a).await; // identical history, identical heads

    let x = a.create_actor(ActorKind::Agent, "x").await.unwrap();

    // Both sides claim the same task while disconnected.
    a.claim_task(&task.id, &x.id, Duration::from_secs(900))
        .await
        .unwrap();
    b.claim_task(&task.id, &y.id, Duration::from_secs(900))
        .await
        .unwrap();

    exchange(&a, &b).await;
    exchange(&b, &a).await;

    for (name, s) in [("a", &a), ("b", &b)] {
        let t = s.get_task(&task.id).await.unwrap();
        assert_eq!(t.status, TaskStatus::Work, "{name}: task is worked");
        let active = active_claims(s, &task.id).await;
        assert_eq!(active.len(), 1, "{name}: exactly one active claim");
        // The losing claim event stays in the log.
        let events = s.list_events(&task.id).await.unwrap();
        assert_eq!(
            events.iter().filter(|e| e.kind == "task.claimed").count(),
            2,
            "{name}: both claim events logged"
        );
    }
    // Both replicas agree on the winner (D8: min lamport, then event id —
    // with identical lamports the winner is hash-determined, so don't
    // assume it is x).
    let active_a = active_claims(&a, &task.id).await;
    let active_b = active_claims(&b, &task.id).await;
    assert_eq!(active_a, active_b, "replicas agree on the winner");
    assert!(
        active_a[0] == x.id || active_a[0] == y.id,
        "winner is one of the two claimants"
    );
}

#[tokio::test]
async fn u4_shuffled_delivery_parks_and_drains() {
    let a = store().await;
    let alice = a.create_actor(ActorKind::Human, "alice").await.unwrap();
    let task = a.create_task("chain", "", &alice.id).await.unwrap();
    a.claim_task(&task.id, &alice.id, Duration::from_secs(900))
        .await
        .unwrap();
    a.submit_for_review(&task.id, &alice.id, &Submission::default())
        .await
        .unwrap();
    a.approve(&task.id, &alice.id, None).await.unwrap();

    let b = store().await;
    // Reverse the event order in the bundle.
    let mut bundle = a.export_bundle().await.unwrap();
    bundle.events.reverse();

    let m = b.receive_bundle(&bundle).await.unwrap();
    assert_eq!(b.pending_count().await.unwrap(), 0, "pending drained");
    assert!(
        boards_equal(&a, &b).await,
        "converged despite shuffle: {m:?}"
    );
    let ha: Vec<String> = a.heads().await.unwrap().iter().map(hex::encode).collect();
    let hb: Vec<String> = b.heads().await.unwrap().iter().map(hex::encode).collect();
    assert_eq!(hb, ha, "heads converge after shuffle");
}

#[tokio::test]
async fn u5_tampered_event_rejected() {
    let a = store().await;
    let alice = a.create_actor(ActorKind::Human, "alice").await.unwrap();
    let task = a.create_task("signed", "", &alice.id).await.unwrap();

    let b = store().await;
    let mut bundle = a.export_bundle().await.unwrap();

    // Corrupt one byte inside the first (only) event's payload string.
    let byte = &mut bundle.events[0][8];
    *byte = byte.wrapping_add(1);

    let m = b.receive_bundle(&bundle).await.unwrap();
    assert_eq!(m.rejected, 1, "tampered event rejected: {m:?}");
    assert!(!m.rejected_ids.is_empty());
    assert!(
        b.list_tasks(None).await.unwrap().is_empty(),
        "no state applied"
    );
    // Untouched events would still apply — sanity-check the good path in the
    // same setup.
    let good = a.export_bundle().await.unwrap();
    let m = b.receive_bundle(&good).await.unwrap();
    assert_eq!(m.added, 1);
    assert_eq!(b.get_task(&task.id).await.unwrap().title, "signed");
}

#[tokio::test]
async fn merge_survives_partial_parents_then_catches_up() {
    let a = store().await;
    let alice = a.create_actor(ActorKind::Human, "alice").await.unwrap();
    let t1 = a.create_task("one", "", &alice.id).await.unwrap();
    let _t2 = a.create_task("two", "", &alice.id).await.unwrap();

    let b = store().await;
    // B receives the second task event without its parent — as a fragment
    // cut from a bigger bundle.
    let bundle = a.export_bundle().await.unwrap();
    let mut fragment = bundle.clone();
    fragment.events.remove(1); // [root actor, task one, task two]
    let m = b.receive_bundle(&fragment).await.unwrap();
    assert!(m.pending > 0, "parent-missing event parked: {m:?}");
    assert_eq!(b.pending_count().await.unwrap(), 1);

    // The parent arrives later; the park drains and applies.
    let mut rest = bundle;
    rest.events = vec![rest.events.remove(1)];
    let m = b.receive_bundle(&rest).await.unwrap();
    assert_eq!(b.pending_count().await.unwrap(), 0);
    let _ = t1;
    assert!(boards_equal(&a, &b).await, "caught up: {m:?}");
}

// ---- Sprint 03: gate correctness (UAT U1, U2, U4) ----

#[tokio::test]
async fn s03_u1_reject_then_reclaim_replicates() {
    let a = store().await;
    let alice = a.create_actor(ActorKind::Human, "alice").await.unwrap();
    let bot = a.create_actor(ActorKind::Agent, "bot").await.unwrap();
    let task = a.create_task("rework", "", &alice.id).await.unwrap();
    let ttl = Duration::from_secs(900);
    a.claim_task(&task.id, &bot.id, ttl).await.unwrap();
    a.submit_for_review(&task.id, &bot.id, &Submission::default())
        .await
        .unwrap();

    // B follows along incrementally; C imports everything at the end.
    let b = store().await;
    exchange(&a, &b).await;

    a.request_changes(&task.id, &alice.id, "rework")
        .await
        .unwrap();
    a.claim_task(&task.id, &bot.id, ttl).await.unwrap();
    exchange(&a, &b).await;
    let c = store().await;
    exchange(&a, &c).await;

    assert_eq!(a.get_task(&task.id).await.unwrap().status, TaskStatus::Work);
    for (name, s) in [("b", &b), ("c", &c)] {
        assert!(
            boards_equal(&a, s).await,
            "{name} matches origin\na: {:?}\n{name}: {:?}",
            a.list_tasks(None).await.unwrap(),
            s.list_tasks(None).await.unwrap()
        );
        assert_eq!(active_claims(s, &task.id).await, vec![bot.id.clone()]);
    }
}

#[tokio::test]
async fn s03_u2_concurrent_approve_and_reject_converge() {
    let a = store().await;
    let alice = a.create_actor(ActorKind::Human, "alice").await.unwrap();
    let bot = a.create_actor(ActorKind::Agent, "bot").await.unwrap();
    let task = a
        .create_task("contested review", "", &alice.id)
        .await
        .unwrap();
    a.claim_task(&task.id, &bot.id, Duration::from_secs(900))
        .await
        .unwrap();
    a.submit_for_review(&task.id, &bot.id, &Submission::default())
        .await
        .unwrap();

    let b = store().await;
    let carol = provision(&a, &b, ActorKind::Human, "carol").await;
    exchange(&a, &b).await;

    // Disconnected reviewers disagree.
    a.approve(&task.id, &alice.id, None).await.unwrap();
    b.request_changes(&task.id, &carol.id, "rework")
        .await
        .unwrap();

    exchange(&a, &b).await;
    exchange(&b, &a).await;

    // Same lamport on both sides ⇒ the winner is hash-determined: assert
    // agreement, not which verdict.
    let sa = a.get_task(&task.id).await.unwrap().status;
    let sb = b.get_task(&task.id).await.unwrap().status;
    assert_eq!(sa, sb, "replicas agree on the review verdict");
    assert!(sa == TaskStatus::Done || sa == TaskStatus::Queue);
    assert!(boards_equal(&a, &b).await, "boards converge");
}

#[tokio::test]
async fn s03_u4_force_release_replicates() {
    let a = store().await;
    let alice = a.create_actor(ActorKind::Human, "alice").await.unwrap();
    let bot = a.create_actor(ActorKind::Agent, "bot").await.unwrap();
    let task = a.create_task("hung", "", &alice.id).await.unwrap();
    a.claim_task(&task.id, &bot.id, Duration::from_secs(900))
        .await
        .unwrap();
    let b = store().await;
    exchange(&a, &b).await;
    assert_eq!(active_claims(&b, &task.id).await.len(), 1);

    a.release_claim(&task.id, &alice.id, true).await.unwrap();
    exchange(&a, &b).await;

    assert_eq!(
        b.get_task(&task.id).await.unwrap().status,
        TaskStatus::Queue
    );
    assert!(active_claims(&b, &task.id).await.is_empty());
}

// ---- Sprint 04: claim liveness (UAT U1) ----

#[tokio::test]
async fn s04_u1_renewed_claim_holds_on_every_replica() {
    let a = store().await;
    let alice = a.create_actor(ActorKind::Human, "alice").await.unwrap();
    let bot = a.create_actor(ActorKind::Agent, "bot").await.unwrap();
    let task = a.create_task("long job", "", &alice.id).await.unwrap();
    a.claim_task(&task.id, &bot.id, Duration::from_millis(300))
        .await
        .unwrap();

    let b = store().await;
    let rival = provision(&a, &b, ActorKind::Agent, "rival").await;
    exchange(&a, &b).await;

    // The holder renews on A; B does not hear about it before the original
    // TTL runs out, so a rival there takes the task in good faith.
    a.renew_claim(&task.id, &bot.id, Duration::from_secs(900))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    b.claim_task(&task.id, &rival.id, Duration::from_secs(900))
        .await
        .unwrap();

    exchange(&a, &b).await;
    exchange(&b, &a).await;
    // C never saw the short TTL live: it learns everything after the fact.
    let c = store().await;
    exchange(&a, &c).await;

    for (name, s) in [("a", &a), ("b", &b), ("c", &c)] {
        assert_eq!(
            active_claims(s, &task.id).await,
            vec![bot.id.clone()],
            "{name}: the renewing holder keeps the task"
        );
        assert!(boards_equal(&a, s).await, "{name} matches a");
    }
    assert_eq!(a.get_task(&task.id).await.unwrap().status, TaskStatus::Work);
}

#[tokio::test]
async fn s04_expired_claim_is_displaced_on_every_replica() {
    let a = store().await;
    let alice = a.create_actor(ActorKind::Human, "alice").await.unwrap();
    let x = a.create_actor(ActorKind::Agent, "x").await.unwrap();
    let y = a.create_actor(ActorKind::Agent, "y").await.unwrap();
    let task = a.create_task("abandoned", "", &alice.id).await.unwrap();
    a.claim_task(&task.id, &x.id, Duration::from_millis(100))
        .await
        .unwrap();
    let b = store().await;
    exchange(&a, &b).await; // B holds x's claim live
    tokio::time::sleep(Duration::from_millis(200)).await;
    a.claim_task(&task.id, &y.id, Duration::from_secs(900))
        .await
        .unwrap();
    exchange(&a, &b).await;
    let c = store().await;
    exchange(&a, &c).await;

    for (name, s) in [("b", &b), ("c", &c)] {
        assert_eq!(
            active_claims(s, &task.id).await,
            vec![y.id.clone()],
            "{name}"
        );
        assert!(
            boards_equal(&a, s).await,
            "{name} matches a\na: {:?}\n{name}: {:?}",
            a.list_tasks(None).await.unwrap(),
            s.list_tasks(None).await.unwrap()
        );
    }
}

// ---- Sprint 06: dependencies and roles replicate (UAT U1, U4) ----

#[tokio::test]
async fn s06_links_and_roles_replicate() {
    let a = store().await;
    let alice = a.create_actor(ActorKind::Human, "alice").await.unwrap();
    let worker = a
        .create_actor_with_roles(ActorKind::Agent, "worker", &[rstask_core::Role::Worker])
        .await
        .unwrap();
    let epic = a.create_task("epic", "", &alice.id).await.unwrap();
    let part = a
        .create_subtask(&epic.id, "part", "", &alice.id)
        .await
        .unwrap();
    let deploy = a.create_task("deploy", "", &alice.id).await.unwrap();
    a.add_dependency(&deploy.id, &part.id, &alice.id)
        .await
        .unwrap();

    let b = store().await;
    let bob = provision(&a, &b, ActorKind::Agent, "bob").await;
    exchange(&a, &b).await;

    assert!(boards_equal(&a, &b).await, "parent_id rides on the board");
    let d = b.task_detail(&deploy.id).await.unwrap();
    assert_eq!(d.blocked_by, vec![part.id.clone()]);
    assert_eq!(
        b.task_detail(&epic.id).await.unwrap().children,
        vec![part.id.clone()]
    );
    for blocked in [&deploy.id, &epic.id] {
        let err = b
            .claim_task(blocked, &bob.id, Duration::from_secs(60))
            .await
            .unwrap_err();
        assert!(
            matches!(err, rstask_core::Error::Blocked { .. }),
            "got {err:?}"
        );
    }
    assert_eq!(
        b.get_actor(&worker.id).await.unwrap().roles.0,
        vec![rstask_core::Role::Worker]
    );
}

#[tokio::test]
async fn s06_concurrent_dependency_edits_converge() {
    let a = store().await;
    let alice = a.create_actor(ActorKind::Human, "alice").await.unwrap();
    let t = a.create_task("t", "", &alice.id).await.unwrap();
    let x = a.create_task("x", "", &alice.id).await.unwrap();
    let y = a.create_task("y", "", &alice.id).await.unwrap();
    a.add_dependency(&t.id, &x.id, &alice.id).await.unwrap();
    let b = store().await;
    let carol = provision(&a, &b, ActorKind::Human, "carol").await;
    exchange(&a, &b).await;

    // Disconnected planners edit the same task's dependencies.
    a.remove_dependency(&t.id, &x.id, &alice.id).await.unwrap();
    a.add_dependency(&t.id, &y.id, &alice.id).await.unwrap();
    b.remove_dependency(&t.id, &x.id, &carol.id).await.unwrap();
    b.add_dependency(&t.id, &x.id, &carol.id).await.unwrap();

    exchange(&a, &b).await;
    exchange(&b, &a).await;

    let da = a.task_detail(&t.id).await.unwrap().blocked_by;
    let db = b.task_detail(&t.id).await.unwrap().blocked_by;
    assert_eq!(da, db, "replicas agree on the dependency set");
    assert!(da.contains(&y.id));
}
