//! Sprint-02 UAT mirrors (U1, U4, U5): two nodes on one runtime reconcile
//! their DAGs over iroh QUIC, relay disabled.
#![cfg(feature = "net")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use rstask_core::net::{NetConfig, Node, PeerSpec, RelayChoice};
use rstask_core::{ActorKind, Error, Store, Submission, TaskStatus};

struct Replica {
    store: Arc<Store>,
    node: Node,
}

impl Replica {
    async fn new(token: &str) -> Replica {
        let store = Arc::new(Store::open_in_memory().await.unwrap());
        let cfg = NetConfig {
            relay: RelayChoice::Disabled,
            bind: Some("127.0.0.1:0".parse().unwrap()),
            token: token.into(),
        };
        let node = Node::spawn(iroh::SecretKey::generate(), cfg, store.clone())
            .await
            .unwrap();
        Replica { store, node }
    }

    fn spec(&self) -> PeerSpec {
        self.node.peer_spec()
    }

    async fn heads(&self) -> Vec<String> {
        let heads = self.store.heads().await.unwrap();
        heads.iter().map(hex::encode).collect()
    }
}

async fn converged(a: &Replica, b: &Replica) -> bool {
    a.heads().await == b.heads().await
        && a.store.board_state().await.unwrap() == b.store.board_state().await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn u1_two_nodes_converge_over_quic() {
    let a = Replica::new("tok").await;
    let b = Replica::new("tok").await;
    let alice = a
        .store
        .create_actor(ActorKind::Human, "alice")
        .await
        .unwrap();
    let bob = a.store.create_actor(ActorKind::Agent, "bob").await.unwrap();
    let task = a.store.create_task("shared", "", &alice.id).await.unwrap();

    // First contact: B starts empty and pulls the workspace, root included.
    let stats = b.node.sync_with(&a.spec()).await.unwrap();
    assert_eq!((stats.received.rejected, stats.sent), (0, 0), "{stats:?}");
    assert!(
        converged(&a, &b).await,
        "{stats:?}\na: {:?} {:?}\nb: {:?} {:?}",
        a.heads().await,
        a.store.list_tasks(None).await.unwrap(),
        b.heads().await,
        b.store.list_tasks(None).await.unwrap()
    );
    b.store
        .import_key(&bob.id, &a.store.export_key(&bob.id).unwrap())
        .await
        .unwrap();

    // Diverge: A walks the task through review; B creates enough tasks to
    // need several frames each way.
    let ttl = Duration::from_secs(900);
    a.store.claim_task(&task.id, &alice.id, ttl).await.unwrap();
    a.store
        .submit_for_review(&task.id, &alice.id, &Submission::default())
        .await
        .unwrap();
    a.store.approve(&task.id, &alice.id, None).await.unwrap();
    for i in 0..120 {
        let title = format!("from b {i}");
        b.store.create_task(&title, "", &bob.id).await.unwrap();
    }
    assert!(!converged(&a, &b).await);

    let stats = b.node.sync_with(&a.spec()).await.unwrap();
    assert_eq!(stats.received.added, 3, "{stats:?}");
    assert_eq!((stats.sent, stats.refused_by_peer), (120, 0), "{stats:?}");
    assert!(converged(&a, &b).await, "one sync converges both sides");
    assert_eq!(
        b.store.get_task(&task.id).await.unwrap().status,
        TaskStatus::Done
    );
    assert_eq!(a.store.list_tasks(None).await.unwrap().len(), 121);

    // Nothing left to move, in either direction.
    for (x, y) in [(&a, &b), (&b, &a)] {
        let stats = x.node.sync_with(&y.spec()).await.unwrap();
        assert_eq!((stats.received.added, stats.sent), (0, 0), "{stats:?}");
    }
    // The dialer remembers who it synced with.
    let peers = b.store.list_peers().await.unwrap();
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].id, a.node.id().to_string());
    assert!(peers[0].last_seen.is_some());
    a.node.shutdown().await;
    b.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn u4_wrong_token_is_refused_before_any_exchange() {
    let a = Replica::new("token-one").await;
    let b = Replica::new("token-two").await;
    let alice = a
        .store
        .create_actor(ActorKind::Human, "alice")
        .await
        .unwrap();
    a.store
        .create_task("secret plans", "", &alice.id)
        .await
        .unwrap();
    let carol = b
        .store
        .create_actor(ActorKind::Human, "carol")
        .await
        .unwrap();
    b.store.create_task("b's own", "", &carol.id).await.unwrap();
    let before = (a.heads().await, b.heads().await);

    let err = b.node.sync_with(&a.spec()).await.unwrap_err();
    assert!(
        matches!(&err, Error::Net(m) if m.contains("token")),
        "got {err:?}"
    );
    assert_eq!(
        (a.heads().await, b.heads().await),
        before,
        "stores untouched"
    );
    assert!(b.store.list_peers().await.unwrap().is_empty());

    // No token at all: the node does not start.
    let cfg = NetConfig {
        relay: RelayChoice::Disabled,
        ..Default::default()
    };
    let err = Node::spawn(iroh::SecretKey::generate(), cfg, a.store.clone())
        .await
        .err()
        .expect("refuses to start");
    assert!(
        matches!(&err, Error::Net(m) if m.contains("token")),
        "got {err:?}"
    );
    a.node.shutdown().await;
    b.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn u5_unreachable_peer_fails_in_bounded_time() {
    let b = Replica::new("tok").await;
    // An id nobody serves, at an address nothing listens on.
    let nobody = PeerSpec {
        id: iroh::SecretKey::generate().public(),
        addrs: vec!["127.0.0.1:9".parse().unwrap()],
    };
    let started = Instant::now();
    let err = b.node.sync_with(&nobody).await.unwrap_err();
    assert!(matches!(err, Error::Net(_)), "got {err:?}");
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "{:?}",
        started.elapsed()
    );
    assert!(b.heads().await.is_empty());

    // Peer specs round-trip through their text form.
    let spec: PeerSpec = nobody.to_string().parse().unwrap();
    assert_eq!(spec, nobody);
    assert!("not-an-id@127.0.0.1:1".parse::<PeerSpec>().is_err());
    b.node.shutdown().await;
}
