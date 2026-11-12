//! Sprint-07 UAT mirrors (U1–U5): keys, provisioning and authority on merge.
//!
//! Forged events are hand-signed with an actor's real key and fed straight to
//! `receive_bundle` — what a modified client holding that key could send.

use std::time::Duration;

use chrono::Utc;
use ed25519_dalek::SigningKey;
use rstask_core::sync::{decode_bundle, encode_bundle};
use rstask_core::{
    Actor, ActorKind, Error, Merge, Role, Signable, Store, StoredEvent, Submission, SyncBundle,
    TaskStatus,
};
use serde_json::json;

const TTL: Duration = Duration::from_secs(900);

async fn store() -> Store {
    Store::open_in_memory().await.expect("open")
}

async fn exchange(a: &Store, b: &Store) {
    let bundle = a.export_bundle().await.unwrap();
    b.receive_bundle(&decode_bundle(&encode_bundle(&bundle)).unwrap())
        .await
        .unwrap();
}

async fn provision(a: &Store, b: &Store, kind: ActorKind, name: &str, roles: &[Role]) -> Actor {
    let actor = a.create_actor_with_roles(kind, name, roles).await.unwrap();
    exchange(a, b).await;
    b.import_key(&actor.id, &a.export_key(&actor.id).unwrap())
        .await
        .unwrap();
    actor
}

/// Hand-sign an event as `actor` on top of `s`'s heads and deliver it to `s`,
/// skipping every check the local write path makes. `lamport_skew` rewinds
/// (or advances) the clock.
async fn forge(
    s: &Store,
    actor: &str,
    task: &str,
    kind: &str,
    payload: serde_json::Value,
    lamport_skew: i64,
) -> Merge {
    let top: i64 = sqlx::query_scalar("SELECT max(lamport) FROM events")
        .fetch_one(s.pool())
        .await
        .unwrap();
    let ev = StoredEvent::sign(
        &SigningKey::from_bytes(&s.export_key(actor).unwrap()),
        Signable {
            lamport: (top + 1 + lamport_skew) as u64,
            parents: s.heads().await.unwrap(),
            actor_id: actor.to_string(),
            task_id: task.to_string(),
            kind: kind.to_string(),
            payload: payload.to_string(),
            at: Utc::now(),
        },
    );
    let bundle = SyncBundle {
        events: vec![ev.encode()],
    };
    s.receive_bundle(&bundle).await.unwrap()
}

struct World {
    s: Store,
    alice: String,
    bot: String,
    rival: String,
    task: String,
}

/// A workspace with root `alice`, agents `bot` and `rival`, one queued task.
async fn world() -> World {
    let s = store().await;
    let alice = s.create_actor(ActorKind::Human, "alice").await.unwrap().id;
    let bot = s.create_actor(ActorKind::Agent, "bot").await.unwrap().id;
    let rival = s.create_actor(ActorKind::Agent, "rival").await.unwrap().id;
    let task = s.create_task("t", "", &alice).await.unwrap().id;
    World {
        s,
        alice,
        bot,
        rival,
        task,
    }
}

async fn status(s: &Store, task: &str) -> TaskStatus {
    s.get_task(task).await.unwrap().status
}

// ---- U1: keys --------------------------------------------------------------

#[tokio::test]
async fn secrets_live_in_the_key_directory_not_the_database() {
    let dir = std::env::temp_dir().join(format!("rstask-keys-{}", uuid::Uuid::new_v4()));
    let db = dir.join("w.sqlite");
    let keys = dir.join("alice-keys");
    let s = Store::open_with_keys(&db, &keys).await.unwrap();
    let alice = s.create_actor(ActorKind::Human, "alice").await.unwrap();
    let task = s.create_task("t", "", &alice.id).await.unwrap();
    let secret = s.export_key(&alice.id).unwrap();
    assert!(keys.join(format!("{}.key", alice.id)).exists());
    drop(s);

    // Neither the raw secret nor its hex form is anywhere in the database files.
    for file in ["w.sqlite", "w.sqlite-wal"] {
        let bytes = std::fs::read(dir.join(file)).unwrap_or_default();
        let hex = hex::encode(secret);
        assert!(
            !bytes.windows(32).any(|w| w == secret),
            "{file} holds the key"
        );
        assert!(
            !bytes.windows(hex.len()).any(|w| w == hex.as_bytes()),
            "{file} holds the key"
        );
    }

    // Without the key file the same database is read-only for that actor.
    let other = Store::open_with_keys(&db, &dir.join("empty"))
        .await
        .unwrap();
    assert_eq!(other.get_task(&task.id).await.unwrap().title, "t");
    let err = other.create_task("x", "", &alice.id).await.unwrap_err();
    assert!(matches!(err, Error::NoSigningKey(_)), "got {err:?}");
    // A wrong key is not accepted for the actor; the right one is.
    let err = other.import_key(&alice.id, &[7; 32]).await.unwrap_err();
    assert!(matches!(err, Error::KeyMismatch(_)), "got {err:?}");
    other.import_key(&alice.id, &secret).await.unwrap();
    other.create_task("x", "", &alice.id).await.unwrap();
    drop(other);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- U2: provisioning ------------------------------------------------------

#[tokio::test]
async fn actors_are_provisioned_by_a_human_from_one_root() {
    let s = store().await;
    let err = s.create_actor(ActorKind::Agent, "bot").await.unwrap_err();
    assert!(matches!(err, Error::NotPermitted { .. }), "root: {err:?}");

    let alice = s.create_actor(ActorKind::Human, "alice").await.unwrap();
    let bot = s.create_actor(ActorKind::Agent, "bot").await.unwrap();
    let err = s
        .provision_actor(ActorKind::Agent, "minion", &[], Some(&bot.id))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotPermitted { .. }), "agent: {err:?}");

    // Humans provision humans; the creation is an event signed by the provisioner.
    let carol = s
        .provision_actor(ActorKind::Human, "carol", &[], Some(&alice.id))
        .await
        .unwrap();
    let dave = s
        .provision_actor(ActorKind::Agent, "dave", &[Role::Worker], Some(&carol.id))
        .await
        .unwrap();
    let signer: String = sqlx::query_scalar(
        "SELECT actor_id FROM events WHERE kind = 'actor.created' AND task_id = ?",
    )
    .bind(&dave.id)
    .fetch_one(s.pool())
    .await
    .unwrap();
    assert_eq!(signer, carol.id);

    // A replica that only imported the workspace holds no keys: it can read,
    // not provision.
    let b = store().await;
    exchange(&s, &b).await;
    assert_eq!(
        b.get_actor(&dave.id).await.unwrap().roles.0,
        vec![Role::Worker]
    );
    let err = b.create_actor(ActorKind::Agent, "x").await.unwrap_err();
    assert!(matches!(err, Error::NoSigningKey(_)), "got {err:?}");

    // A forged provisioning by an agent is refused on merge.
    let m = forge(
        &s,
        &bot.id,
        "minion-id",
        "actor.created",
        json!({ "kind": "human", "name": "minion", "roles": ["reviewer"], "pubkey": hex::encode([9u8; 32]) }),
        0,
    )
    .await;
    assert_eq!(m.rejected, 1, "{m:?}");
    assert!(matches!(
        s.get_actor("minion-id").await.unwrap_err(),
        Error::ActorNotFound(_)
    ));
}

#[tokio::test]
async fn a_different_root_is_a_different_workspace() {
    let ours = store().await;
    let alice = ours.create_actor(ActorKind::Human, "alice").await.unwrap();
    ours.create_task("ours", "", &alice.id).await.unwrap();

    let theirs = store().await;
    let mallory = theirs
        .create_actor(ActorKind::Human, "mallory")
        .await
        .unwrap();
    theirs.create_task("theirs", "", &mallory.id).await.unwrap();

    let m = ours
        .receive_bundle(&theirs.export_bundle().await.unwrap())
        .await
        .unwrap();
    assert_eq!(m.added, 0, "{m:?}");
    assert!(m.rejected >= 1, "{m:?}");
    assert_eq!(ours.list_tasks(None).await.unwrap().len(), 1);
    assert!(ours.get_actor(&mallory.id).await.is_err());
}

// ---- U3: authority on merge ------------------------------------------------

#[tokio::test]
async fn forged_self_approval_changes_nothing() {
    let w = world().await;
    w.s.claim_task(&w.task, &w.bot, TTL).await.unwrap();
    w.s.submit_for_review(&w.task, &w.bot, &Submission::default())
        .await
        .unwrap();
    let submission =
        w.s.task_detail(&w.task)
            .await
            .unwrap()
            .submission
            .unwrap()
            .id;

    let m = forge(
        &w.s,
        &w.bot,
        &w.task,
        "task.approved",
        json!({ "submission": submission }),
        0,
    )
    .await;
    assert_eq!((m.added, m.rejected), (1, 0), "logged, not refused: {m:?}");
    assert_eq!(status(&w.s, &w.task).await, TaskStatus::Review);

    // Someone else's approval of that same submission counts.
    w.s.approve(&w.task, &w.rival, Some(&submission))
        .await
        .unwrap();
    assert_eq!(status(&w.s, &w.task).await, TaskStatus::Done);
}

#[tokio::test]
async fn only_the_holder_releases_or_submits_on_merge() {
    let w = world().await;
    let claim = w.s.claim_task(&w.task, &w.bot, TTL).await.unwrap();
    for kind in ["claim.released", "task.submitted"] {
        let m = forge(
            &w.s,
            &w.rival,
            &w.task,
            kind,
            json!({ "claim_id": claim.id }),
            0,
        )
        .await;
        assert_eq!(m.added, 1, "{m:?}");
        let d = w.s.task_detail(&w.task).await.unwrap();
        assert_eq!(d.task.status, TaskStatus::Work, "{kind}");
        assert_eq!(d.claim.unwrap().actor_id, w.bot, "{kind}");
    }
    // A forged edit by a non-holder does not change the content either.
    forge(
        &w.s,
        &w.rival,
        &w.task,
        "task.updated",
        json!({ "title": "hijacked" }),
        0,
    )
    .await;
    assert_eq!(w.s.get_task(&w.task).await.unwrap().title, "t");
}

#[tokio::test]
async fn roles_and_kind_are_checked_on_merge() {
    let w = world().await;
    let worker =
        w.s.create_actor_with_roles(ActorKind::Agent, "w", &[Role::Worker])
            .await
            .unwrap()
            .id;
    let reviewer =
        w.s.create_actor_with_roles(ActorKind::Agent, "r", &[Role::Reviewer])
            .await
            .unwrap()
            .id;
    let claim = w.s.claim_task(&w.task, &worker, TTL).await.unwrap();
    let before = w.s.dag_event_count().await.unwrap();

    for (actor, task, kind, payload) in [
        (
            &worker,
            "new-task",
            "task.created",
            json!({ "title": "x", "body_md": "" }),
        ),
        (&worker, w.task.as_str(), "task.protected", json!({})),
        (
            &reviewer,
            w.task.as_str(),
            "task.claimed",
            json!({ "claim_id": "c", "expires_at": Utc::now() }),
        ),
        (&worker, w.task.as_str(), "task.approved", json!({})),
        (
            &worker,
            w.task.as_str(),
            "task.changes_requested",
            json!({ "reason": "no" }),
        ),
        (
            &reviewer,
            w.task.as_str(),
            "claim.force_released",
            json!({ "claim_id": claim.id }),
        ),
    ] {
        let m = forge(&w.s, actor, task, kind, payload, 0).await;
        assert_eq!((m.added, m.rejected), (0, 1), "{kind}: {m:?}");
    }
    assert_eq!(
        w.s.dag_event_count().await.unwrap(),
        before,
        "none entered the log"
    );
    assert_eq!(status(&w.s, &w.task).await, TaskStatus::Work);
    assert!(w.s.get_task("new-task").await.is_err());
}

#[tokio::test]
async fn a_rewound_lamport_clock_is_refused() {
    let w = world().await;
    // A claim back-dated in log order would win first-claim-wins after the fact.
    for skew in [-2, 3] {
        let m = forge(
            &w.s,
            &w.rival,
            &w.task,
            "task.claimed",
            json!({ "claim_id": "c", "expires_at": Utc::now() + chrono::Duration::hours(1) }),
            skew,
        )
        .await;
        assert_eq!((m.added, m.rejected), (0, 1), "skew {skew}: {m:?}");
    }
    assert_eq!(status(&w.s, &w.task).await, TaskStatus::Queue);
}

#[tokio::test]
async fn concurrent_content_edits_converge() {
    let a = store().await;
    let alice = a.create_actor(ActorKind::Human, "alice").await.unwrap();
    let task = a.create_task("draft", "", &alice.id).await.unwrap();
    let b = store().await;
    let carol = provision(&a, &b, ActorKind::Human, "carol", &[]).await;

    a.update_task(&task.id, Some("alice's title"), None, Some(3), &alice.id)
        .await
        .unwrap();
    b.update_task(
        &task.id,
        Some("carol's title"),
        Some("carol's body"),
        None,
        &carol.id,
    )
    .await
    .unwrap();
    exchange(&a, &b).await;
    exchange(&b, &a).await;

    let (ta, tb) = (
        a.get_task(&task.id).await.unwrap(),
        b.get_task(&task.id).await.unwrap(),
    );
    assert_eq!(ta, tb, "replicas agree on content");
    assert_eq!((ta.priority, ta.body_md.as_str()), (3, "carol's body"));
}

// ---- U4: evidence-bound approval -------------------------------------------

#[tokio::test]
async fn an_approval_counts_only_for_the_submission_it_names() {
    let a = store().await;
    let alice = a.create_actor(ActorKind::Human, "alice").await.unwrap();
    let bot = a.create_actor(ActorKind::Agent, "bot").await.unwrap();
    let task = a.create_task("t", "", &alice.id).await.unwrap();
    a.claim_task(&task.id, &bot.id, TTL).await.unwrap();
    let first = Submission {
        summary: "first try".into(),
        ..Default::default()
    };
    a.submit_for_review(&task.id, &bot.id, &first)
        .await
        .unwrap();
    let b = store().await;
    let rev = provision(
        &a,
        &b,
        ActorKind::Agent,
        "rev",
        &[Role::Reviewer, Role::Planner],
    )
    .await;
    let s1 = b
        .task_detail(&task.id)
        .await
        .unwrap()
        .submission
        .unwrap()
        .id;

    // On A the work is rejected and resubmitted; B, not having heard, is busy
    // elsewhere and then approves what it read.
    a.request_changes(&task.id, &alice.id, "not yet")
        .await
        .unwrap();
    a.claim_task(&task.id, &bot.id, TTL).await.unwrap();
    let second = Submission {
        summary: "second try".into(),
        ..Default::default()
    };
    a.submit_for_review(&task.id, &bot.id, &second)
        .await
        .unwrap();
    for i in 0..4 {
        b.create_task(&format!("busy {i}"), "", &rev.id)
            .await
            .unwrap();
    }
    b.approve(&task.id, &rev.id, Some(&s1)).await.unwrap();
    assert_eq!(status(&b, &task.id).await, TaskStatus::Done);

    exchange(&a, &b).await;
    exchange(&b, &a).await;
    for (name, s) in [("a", &a), ("b", &b)] {
        let d = s.task_detail(&task.id).await.unwrap();
        assert_eq!(
            d.task.status,
            TaskStatus::Review,
            "{name}: S1's approval is not S2's"
        );
        assert_eq!(d.submission.unwrap().submission.summary, "second try");
    }

    // Locally, naming a superseded submission is refused outright.
    let err = a.approve(&task.id, &alice.id, Some(&s1)).await.unwrap_err();
    assert!(matches!(err, Error::StaleApproval { .. }), "got {err:?}");
    let s2 = a
        .task_detail(&task.id)
        .await
        .unwrap()
        .submission
        .unwrap()
        .id;
    a.approve(&task.id, &alice.id, Some(&s2)).await.unwrap();
    assert_eq!(status(&a, &task.id).await, TaskStatus::Done);
}

// ---- U5: protected tasks ---------------------------------------------------

#[tokio::test]
async fn a_protected_task_needs_a_human_approval() {
    let w = world().await;
    let protected = w.s.protect_task(&w.task, &w.alice).await.unwrap();
    assert!(protected.protected);
    w.s.claim_task(&w.task, &w.bot, TTL).await.unwrap();
    w.s.submit_for_review(&w.task, &w.bot, &Submission::default())
        .await
        .unwrap();
    let submission =
        w.s.task_detail(&w.task)
            .await
            .unwrap()
            .submission
            .unwrap()
            .id;

    let err = w.s.approve(&w.task, &w.rival, None).await.unwrap_err();
    assert!(matches!(err, Error::NotPermitted { .. }), "got {err:?}");
    let m = forge(
        &w.s,
        &w.rival,
        &w.task,
        "task.approved",
        json!({ "submission": submission }),
        0,
    )
    .await;
    assert_eq!(m.added, 1, "{m:?}");
    assert_eq!(status(&w.s, &w.task).await, TaskStatus::Review);

    // Protection replicates, and the human's approval lands.
    let b = store().await;
    exchange(&w.s, &b).await;
    assert!(b.get_task(&w.task).await.unwrap().protected);
    assert_eq!(status(&b, &w.task).await, TaskStatus::Review);
    w.s.approve(&w.task, &w.alice, None).await.unwrap();
    exchange(&w.s, &b).await;
    assert_eq!(status(&b, &w.task).await, TaskStatus::Done);
}

#[tokio::test]
async fn a_legacy_workspace_has_its_secrets_moved_out_and_scrubbed() {
    let dir = std::env::temp_dir().join(format!("rstask-legacy-{}", uuid::Uuid::new_v4()));
    let db = dir.join("w.sqlite");
    let s = Store::open(&db).await.unwrap();
    let alice = s.create_actor(ActorKind::Human, "alice").await.unwrap();
    let secret = s.export_key(&alice.id).unwrap();
    // Recreate the pre-sprint-07 layout: the secret in the database, no key file.
    sqlx::query("UPDATE actor_keys SET secret = ? WHERE actor_id = ?")
        .bind(secret.to_vec())
        .bind(&alice.id)
        .execute(s.pool())
        .await
        .unwrap();
    s.pool().close().await;
    drop(s);
    std::fs::remove_dir_all(dir.join("keys")).unwrap();
    let holds_secret = |file: &str| {
        let bytes = std::fs::read(dir.join(file)).unwrap_or_default();
        bytes.windows(32).any(|w| w == secret)
    };
    assert!(holds_secret("w.sqlite") || holds_secret("w.sqlite-wal"));

    let s = Store::open(&db).await.unwrap();
    assert_eq!(
        s.export_key(&alice.id).unwrap(),
        secret,
        "key moved to the directory"
    );
    s.create_task("still works", "", &alice.id).await.unwrap();
    s.pool().close().await;
    drop(s);
    for file in ["w.sqlite", "w.sqlite-wal"] {
        assert!(!holds_secret(file), "{file} still holds the secret");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- Sprint 08: enrolment without moving a secret (UAT U1) ----

#[tokio::test]
async fn an_actor_enrols_from_another_replica_without_its_key_moving() {
    let a = store().await;
    let alice = a.create_actor(ActorKind::Human, "alice").await.unwrap();
    let task = a.create_task("t", "", &alice.id).await.unwrap();
    let b = store().await; // empty: has not even synced yet

    let request = b
        .request_actor(ActorKind::Agent, "bob", &[Role::Worker])
        .unwrap();
    // What travels is public: no secret in it.
    let wire = serde_json::to_string(&request).unwrap();
    assert!(!wire.contains(&hex::encode(b.export_key(&request.id).unwrap())));

    let bob = a
        .admit_actor(&serde_json::from_str(&wire).unwrap(), None)
        .await
        .unwrap();
    assert_eq!(
        (bob.id.as_str(), bob.kind),
        (request.id.as_str(), ActorKind::Agent)
    );
    assert_eq!(bob.roles.0, vec![Role::Worker]);
    // A never held bob's key; the creation is signed by the root.
    assert!(matches!(a.export_key(&bob.id), Err(Error::NoSigningKey(_))));
    let signer: String = sqlx::query_scalar(
        "SELECT actor_id FROM events WHERE kind = 'actor.created' AND task_id = ?",
    )
    .bind(&bob.id)
    .fetch_one(a.pool())
    .await
    .unwrap();
    assert_eq!(signer, alice.id);

    // B learns of its own admission by sync and can then act; A accepts it.
    exchange(&a, &b).await;
    b.claim_task(&task.id, &bob.id, TTL).await.unwrap();
    exchange(&b, &a).await;
    assert_eq!(
        a.task_detail(&task.id)
            .await
            .unwrap()
            .claim
            .unwrap()
            .actor_id,
        bob.id
    );
}

#[tokio::test]
async fn a_request_must_be_intact_and_cannot_found_a_workspace() {
    let a = store().await;
    let b = store().await;
    let request = b.request_actor(ActorKind::Agent, "bob", &[]).unwrap();

    // Admission needs a root to sign it.
    let err = a.admit_actor(&request, None).await.unwrap_err();
    assert!(matches!(err, Error::NotPermitted { .. }), "got {err:?}");
    let alice = a.create_actor(ActorKind::Human, "alice").await.unwrap();
    let bot = a.create_actor(ActorKind::Agent, "bot").await.unwrap();

    // Edited after it was made: a worker's request turned into a human's.
    let mut forged = request.clone();
    forged.kind = ActorKind::Human;
    let err = a.admit_actor(&forged, None).await.unwrap_err();
    assert!(matches!(err, Error::InvalidRequest(_)), "got {err:?}");
    let mut forged = request.clone();
    forged.pubkey = hex::encode([7u8; 32]);
    let err = a.admit_actor(&forged, None).await.unwrap_err();
    assert!(matches!(err, Error::InvalidRequest(_)), "got {err:?}");

    // Only a human admits; and an id is admitted once.
    let err = a.admit_actor(&request, Some(&bot.id)).await.unwrap_err();
    assert!(matches!(err, Error::NotPermitted { .. }), "got {err:?}");
    a.admit_actor(&request, Some(&alice.id)).await.unwrap();
    let err = a.admit_actor(&request, None).await.unwrap_err();
    assert!(matches!(err, Error::InvalidRequest(_)), "got {err:?}");
}
