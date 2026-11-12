//! Networking: an iroh endpoint that reconciles the workspace's event DAG with
//! a peer (PRD-v2 D1, D2, D6, D7). The session layer is ported from cSharing's
//! `net.rs`; none of its room, epoch, registry, blob or post-quantum machinery
//! is.
//!
//! Every connection carries one bi-directional stream of length-prefixed
//! postcard frames:
//!
//! ```text
//! C→S Hello{nonce_c}
//! S→C HelloAck{nonce_s, mac_s}   mac_s = MAC(token, "server" ‖ id_c ‖ id_s ‖ nonce_c ‖ nonce_s)
//! C→S Auth{mac_c}                mac_c = MAC(token, "client" ‖ …same…)
//! S→C AuthOk
//! then requests: GetBuckets, GetIds, GetEvents, Push, Bye
//! ```
//!
//! Both endpoint ids are authenticated by iroh's TLS, so binding them into the
//! MACs stops a proof from being relayed. The token only decides who may
//! exchange events at all; what an event may *do* is decided by its signature
//! and the authority rules applied on merge (PRD-v3 R2, R3) — `net` is just
//! another carrier for `Store::receive_events`.

use std::collections::BTreeSet;
use std::fmt;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::{presets, Connection, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, EndpointAddr, PublicKey, RelayMode, RelayUrl, SecretKey};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::{Error, Result};
use crate::event::EventId;
use crate::store::Store;
use crate::sync::{differing_buckets, Bucket, Merge};

pub const ALPN: &[u8] = b"rstask/1";
const MAX_FRAME: usize = 16 << 20;
/// Events per `GetEvents` / `Push` frame.
const EVENT_BATCH: usize = 48;
const MAX_IDS: usize = 400_000;
const DIAL_TIMEOUT: Duration = Duration::from_secs(10);
const IO_TIMEOUT: Duration = Duration::from_secs(60);

fn net<E: fmt::Display>(context: &str) -> impl FnOnce(E) -> Error + '_ {
    move |e| Error::Net(format!("{context}: {e}"))
}

// ---------------------------------------------------------------------------------------
// Configuration and peer addressing
// ---------------------------------------------------------------------------------------

/// Which relay servers to use as fallback when no direct path exists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RelayChoice {
    /// n0's public relays plus n0 DNS address lookup (dial by id alone).
    #[default]
    Default,
    /// Direct connections only (LAN, public IPs, tests). Peers need `ID@host:port`.
    Disabled,
    /// A self-hosted `iroh-relay`. No third-party services are contacted.
    Custom(RelayUrl),
}

impl FromStr for RelayChoice {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s {
            "default" | "n0" => RelayChoice::Default,
            "disabled" | "none" | "off" => RelayChoice::Disabled,
            url => RelayChoice::Custom(url.parse().map_err(net("invalid relay URL"))?),
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct NetConfig {
    pub relay: RelayChoice,
    /// Fixed UDP socket, e.g. `0.0.0.0:7701`; a random port when `None`.
    pub bind: Option<SocketAddr>,
    /// The shared workspace token (PRD-v2 D7). Never logged.
    pub token: String,
}

/// A peer to dial: `ENDPOINT_ID` or `ENDPOINT_ID@host:port[,host:port…]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerSpec {
    pub id: PublicKey,
    pub addrs: Vec<SocketAddr>,
}

impl FromStr for PeerSpec {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        let (id, addrs) = match s.split_once('@') {
            Some((id, addrs)) => (id, addrs),
            None => (s, ""),
        };
        let id: PublicKey = id.trim().parse().map_err(net("invalid endpoint id"))?;
        let mut out = Vec::new();
        for a in addrs.split(',').map(str::trim).filter(|a| !a.is_empty()) {
            out.extend(
                a.to_socket_addrs()
                    .map_err(net("cannot resolve peer address"))?,
            );
        }
        Ok(PeerSpec { id, addrs: out })
    }
}

impl fmt::Display for PeerSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.id)?;
        if !self.addrs.is_empty() {
            let addrs: Vec<String> = self.addrs.iter().map(ToString::to_string).collect();
            write!(f, "@{}", addrs.join(","))?;
        }
        Ok(())
    }
}

impl PeerSpec {
    /// The direct addresses as stored in the `peers` table.
    pub fn addrs_string(&self) -> String {
        let addrs: Vec<String> = self.addrs.iter().map(ToString::to_string).collect();
        addrs.join(",")
    }
}

/// This replica's network identity: 32 secret bytes, hex, in `path` (created
/// on first use, owner-only). It names the *node*, not an actor: it signs no
/// events.
pub fn load_or_create_node_key(path: &Path) -> Result<SecretKey> {
    if let Ok(hex) = std::fs::read_to_string(path) {
        let bytes: [u8; 32] = hex::decode(hex.trim())
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| Error::Net(format!("{} is not a node key", path.display())))?;
        return Ok(SecretKey::from_bytes(&bytes));
    }
    let secret = SecretKey::generate();
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    writeln!(options.open(path)?, "{}", hex::encode(secret.to_bytes()))?;
    Ok(secret)
}

// ---------------------------------------------------------------------------------------
// Wire protocol
// ---------------------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
enum Msg {
    Hello {
        nonce: [u8; 32],
    },
    HelloAck {
        nonce: [u8; 32],
        mac: [u8; 32],
    },
    Auth {
        mac: [u8; 32],
    },
    AuthOk,
    GetBuckets,
    Buckets(Vec<Bucket>),
    GetIds {
        buckets: Vec<u64>,
    },
    Ids(Vec<EventId>),
    GetEvents(Vec<EventId>),
    /// Postcard-encoded `StoredEvent`s.
    Events(Vec<Vec<u8>>),
    Push(Vec<Vec<u8>>),
    PushAck {
        added: u32,
        rejected: u32,
    },
    Bye,
    Error(String),
}

async fn write_msg(w: &mut (impl AsyncWrite + Unpin), m: &Msg) -> Result<()> {
    let buf = postcard::to_stdvec(m).map_err(net("cannot encode frame"))?;
    if buf.len() > MAX_FRAME {
        return Err(Error::Net("frame too large".into()));
    }
    w.write_all(&(buf.len() as u32).to_be_bytes()).await?;
    w.write_all(&buf).await?;
    Ok(())
}

async fn read_msg(r: &mut (impl AsyncRead + Unpin)) -> Result<Msg> {
    async fn fill(r: &mut (impl AsyncRead + Unpin), buf: &mut [u8]) -> Result<()> {
        tokio::time::timeout(IO_TIMEOUT, r.read_exact(buf))
            .await
            .map_err(|_| Error::Net("peer timed out".into()))?
            .map_err(net("connection lost"))?;
        Ok(())
    }
    let mut len = [0u8; 4];
    fill(r, &mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(Error::Net("frame too large".into()));
    }
    let mut buf = vec![0u8; len];
    fill(r, &mut buf).await?;
    postcard::from_bytes(&buf).map_err(net("malformed frame"))
}

/// The MAC key behind a workspace token.
fn auth_key(token: &str) -> [u8; 32] {
    blake3::derive_key("rstask p2p workspace token v1", token.as_bytes())
}

fn handshake_mac(
    key: &[u8; 32],
    role: &[u8],
    client: &PublicKey,
    server: &PublicKey,
    nonce_c: &[u8; 32],
    nonce_s: &[u8; 32],
) -> [u8; 32] {
    let mut h = blake3::Hasher::new_keyed(key);
    let parts: [&[u8]; 6] = [
        b"rstask/handshake/v1",
        role,
        client.as_bytes(),
        server.as_bytes(),
        nonce_c,
        nonce_s,
    ];
    for part in parts {
        h.update(&(part.len() as u64).to_be_bytes());
        h.update(part);
    }
    *h.finalize().as_bytes()
}

/// Constant-time equality for MACs.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ---------------------------------------------------------------------------------------
// Node
// ---------------------------------------------------------------------------------------

/// Outcome of one two-way sync.
#[derive(Debug, Default, Clone, Serialize)]
pub struct SyncStats {
    /// What merging the peer's events did here.
    pub received: Merge,
    /// Events sent to the peer.
    pub sent: usize,
    /// Of those, how many the peer refused (authority, PRD-v3 R2).
    pub refused_by_peer: usize,
}

/// A running network node: one iroh endpoint serving one workspace.
#[derive(Clone)]
pub struct Node {
    router: Router,
    store: Arc<Store>,
    cfg: NetConfig,
    key: [u8; 32],
}

struct Session {
    conn: Connection,
    send: SendStream,
    recv: RecvStream,
}

impl Session {
    async fn request(&mut self, m: &Msg) -> Result<Msg> {
        write_msg(&mut self.send, m).await?;
        match read_msg(&mut self.recv).await? {
            Msg::Error(e) => Err(Error::Net(format!("peer error: {e}"))),
            other => Ok(other),
        }
    }

    async fn close(mut self) {
        let _ = write_msg(&mut self.send, &Msg::Bye).await;
        let _ = self.send.finish();
        let _ = tokio::time::timeout(Duration::from_secs(2), self.send.stopped()).await;
        self.conn.close(0u32.into(), b"bye");
    }
}

async fn bind_endpoint(secret: SecretKey, cfg: &NetConfig) -> Result<Endpoint> {
    let mut builder = match &cfg.relay {
        RelayChoice::Default => Endpoint::builder(presets::N0),
        RelayChoice::Disabled => {
            Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled)
        }
        RelayChoice::Custom(url) => {
            Endpoint::builder(presets::Minimal).relay_mode(RelayMode::custom([url.clone()]))
        }
    };
    builder = builder.secret_key(secret);
    if let Some(addr) = cfg.bind {
        builder = builder
            .clear_ip_transports()
            .bind_addr(addr)
            .map_err(net("cannot bind"))?;
    }
    builder
        .bind()
        .await
        .map_err(net("cannot start network endpoint"))
}

impl Node {
    /// Start serving `store` to peers that hold the workspace token.
    pub async fn spawn(secret: SecretKey, cfg: NetConfig, store: Arc<Store>) -> Result<Node> {
        if cfg.token.is_empty() {
            return Err(Error::Net(
                "no workspace token: set p2p.token or RSTASK_P2P_TOKEN".into(),
            ));
        }
        let key = auth_key(&cfg.token);
        let me = secret.public();
        let endpoint = bind_endpoint(secret, &cfg).await?;
        let handler = Handler {
            me,
            store: store.clone(),
            key,
        };
        let router = Router::builder(endpoint).accept(ALPN, handler).spawn();
        Ok(Node {
            router,
            store,
            cfg,
            key,
        })
    }

    pub fn endpoint(&self) -> &Endpoint {
        self.router.endpoint()
    }

    pub fn id(&self) -> PublicKey {
        self.endpoint().id()
    }

    /// This node as a dialable peer spec (`ID@addr,…` for its direct addresses).
    pub fn peer_spec(&self) -> PeerSpec {
        let addr = self.endpoint().addr();
        let mut addrs: Vec<SocketAddr> = addr.ip_addrs().copied().collect();
        if addrs.is_empty() {
            addrs = self.endpoint().bound_sockets();
        }
        PeerSpec {
            id: self.id(),
            addrs,
        }
    }

    pub async fn shutdown(&self) {
        let _ = self.router.shutdown().await;
    }

    fn dial_addr(&self, peer: &PeerSpec) -> EndpointAddr {
        let mut addr = EndpointAddr::new(peer.id);
        for a in &peer.addrs {
            addr = addr.with_ip_addr(*a);
        }
        if let RelayChoice::Custom(url) = &self.cfg.relay {
            addr = addr.with_relay_url(url.clone());
        }
        addr
    }

    /// Connect and prove, both ways, knowledge of the workspace token.
    async fn open_session(&self, peer: &PeerSpec) -> Result<Session> {
        if peer.id == self.id() {
            return Err(Error::Net("refusing to dial ourselves".into()));
        }
        let me = self.id();
        let connecting = self.endpoint().connect(self.dial_addr(peer), ALPN);
        let conn = tokio::time::timeout(DIAL_TIMEOUT, connecting)
            .await
            .map_err(|_| Error::Net("connection timed out".into()))?
            .map_err(net("cannot connect"))?;
        let (send, recv) = conn.open_bi().await.map_err(net("cannot open stream"))?;
        let mut s = Session { conn, send, recv };
        let nonce_c: [u8; 32] = rand::random();
        let Msg::HelloAck {
            nonce: nonce_s,
            mac,
        } = s.request(&Msg::Hello { nonce: nonce_c }).await?
        else {
            return Err(Error::Net("unexpected handshake reply".into()));
        };
        let expected = handshake_mac(&self.key, b"server", &me, &peer.id, &nonce_c, &nonce_s);
        if !ct_eq(&mac, &expected) {
            s.conn.close(1u32.into(), b"auth");
            return Err(Error::Net(
                "peer does not hold this workspace's token".into(),
            ));
        }
        let mac = handshake_mac(&self.key, b"client", &me, &peer.id, &nonce_c, &nonce_s);
        match s.request(&Msg::Auth { mac }).await? {
            Msg::AuthOk => Ok(s),
            _ => Err(Error::Net("unexpected handshake reply".into())),
        }
    }

    /// Two-way reconcile the workspace's event DAG with one peer.
    pub async fn sync_with(&self, peer: &PeerSpec) -> Result<SyncStats> {
        let mut s = self.open_session(peer).await?;
        let res = self.reconcile(&mut s).await;
        s.close().await;
        if res.is_ok() {
            self.store
                .touch_peer(&peer.id.to_string(), &peer.addrs_string())
                .await?;
        }
        res
    }

    /// Bucket fingerprints → id diff → batched exchange (PRD-v2 D2).
    async fn reconcile(&self, s: &mut Session) -> Result<SyncStats> {
        let unexpected = |what: &str| Error::Net(format!("unexpected reply to {what}"));
        let Msg::Buckets(remote) = s.request(&Msg::GetBuckets).await? else {
            return Err(unexpected("GetBuckets"));
        };
        let diff = differing_buckets(&self.store.buckets().await?, &remote);
        let mut stats = SyncStats::default();
        if diff.is_empty() {
            return Ok(stats);
        }
        let request = Msg::GetIds {
            buckets: diff.clone(),
        };
        let Msg::Ids(remote_ids) = s.request(&request).await? else {
            return Err(unexpected("GetIds"));
        };
        let remote_set: BTreeSet<EventId> = remote_ids.into_iter().collect();
        let local_set: BTreeSet<EventId> = self
            .store
            .ids_in_buckets(&diff)
            .await?
            .into_iter()
            .collect();
        let want: Vec<EventId> = remote_set.difference(&local_set).copied().collect();
        let give: Vec<EventId> = local_set.difference(&remote_set).copied().collect();

        let mut fetched = Vec::with_capacity(want.len());
        for chunk in want.chunks(EVENT_BATCH) {
            let Msg::Events(events) = s.request(&Msg::GetEvents(chunk.to_vec())).await? else {
                return Err(unexpected("GetEvents"));
            };
            if events.len() > chunk.len() {
                return Err(Error::Net("peer sent unrequested events".into()));
            }
            fetched.extend(events);
        }
        stats.received = self.store.receive_events(&fetched).await?;

        // Ids sort by hash, not causally: send parents-first so the peer
        // never has to park (and then report as pending) what it is given.
        for chunk in self.store.events_by_id(&give).await?.chunks(EVENT_BATCH) {
            let Msg::PushAck { rejected, .. } = s.request(&Msg::Push(chunk.to_vec())).await? else {
                return Err(unexpected("Push"));
            };
            stats.sent += chunk.len();
            stats.refused_by_peer += rejected as usize;
        }
        Ok(stats)
    }
}

// ---------------------------------------------------------------------------------------
// Serving
// ---------------------------------------------------------------------------------------

struct Handler {
    me: PublicKey,
    store: Arc<Store>,
    key: [u8; 32],
}

impl fmt::Debug for Handler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Handler").finish_non_exhaustive()
    }
}

impl ProtocolHandler for Handler {
    async fn accept(&self, conn: Connection) -> std::result::Result<(), AcceptError> {
        let res = serve(self, &conn).await;
        // Give the client time to read our last frame before tearing down.
        let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
        res.map_err(|e| AcceptError::from_boxed(e.into()))
    }
}

async fn serve(h: &Handler, conn: &Connection) -> Result<()> {
    let remote = conn.remote_id();
    let (mut send, mut recv) = conn
        .accept_bi()
        .await
        .map_err(net("cannot accept stream"))?;
    let Msg::Hello { nonce: nonce_c } = read_msg(&mut recv).await? else {
        return Err(Error::Net("expected Hello".into()));
    };
    let nonce_s: [u8; 32] = rand::random();
    let mac = handshake_mac(&h.key, b"server", &remote, &h.me, &nonce_c, &nonce_s);
    let ack = Msg::HelloAck {
        nonce: nonce_s,
        mac,
    };
    write_msg(&mut send, &ack).await?;
    // A dialer holding another token stops here: it rejects our MAC and hangs up.
    let Msg::Auth { mac } = read_msg(&mut recv).await? else {
        return Err(Error::Net("expected Auth".into()));
    };
    let expected = handshake_mac(&h.key, b"client", &remote, &h.me, &nonce_c, &nonce_s);
    if !ct_eq(&mac, &expected) {
        let _ = write_msg(&mut send, &Msg::Error("wrong workspace token".into())).await;
        let _ = send.finish();
        return Err(Error::Net(format!(
            "refused {}: wrong workspace token",
            remote.fmt_short()
        )));
    }
    write_msg(&mut send, &Msg::AuthOk).await?;

    loop {
        let reply = match read_msg(&mut recv).await? {
            Msg::Bye => break,
            Msg::GetBuckets => Msg::Buckets(h.store.buckets().await?),
            Msg::GetIds { buckets } => {
                let ids = h.store.ids_in_buckets(&buckets).await?;
                if ids.len() > MAX_IDS {
                    Msg::Error("too many ids".into())
                } else {
                    Msg::Ids(ids)
                }
            }
            Msg::GetEvents(ids) if ids.len() <= EVENT_BATCH => {
                Msg::Events(h.store.events_by_id(&ids).await?)
            }
            Msg::Push(events) if events.len() <= EVENT_BATCH => {
                let merge = h.store.receive_events(&events).await?;
                for lost in &merge.lost_claims {
                    tracing::warn!(
                        task = %lost.task_id,
                        actor = %lost.actor_id,
                        reason = %lost.reason,
                        "claim lost in a sync from {}",
                        remote.fmt_short()
                    );
                }
                Msg::PushAck {
                    added: merge.added as u32,
                    rejected: merge.rejected as u32,
                }
            }
            Msg::GetEvents(_) | Msg::Push(_) => Msg::Error("batch too large".into()),
            _ => Msg::Error("unexpected request".into()),
        };
        write_msg(&mut send, &reply).await?;
    }
    let _ = send.finish();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A client that ignores the server's proof and sends its own with the
    /// wrong token gets an error frame and nothing else.
    #[tokio::test(flavor = "multi_thread")]
    async fn server_refuses_a_client_with_the_wrong_token() {
        let store = Arc::new(Store::open_in_memory().await.unwrap());
        let cfg = NetConfig {
            relay: RelayChoice::Disabled,
            bind: Some("127.0.0.1:0".parse().unwrap()),
            token: "right".into(),
        };
        let server = Node::spawn(SecretKey::generate(), cfg.clone(), store)
            .await
            .unwrap();
        let client = bind_endpoint(SecretKey::generate(), &cfg).await.unwrap();
        let peer = server.peer_spec();
        let mut addr = EndpointAddr::new(peer.id);
        for a in &peer.addrs {
            addr = addr.with_ip_addr(*a);
        }
        let conn = client.connect(addr, ALPN).await.unwrap();
        let (mut send, mut recv) = conn.open_bi().await.unwrap();

        let nonce_c = [1u8; 32];
        write_msg(&mut send, &Msg::Hello { nonce: nonce_c })
            .await
            .unwrap();
        let Msg::HelloAck { nonce: nonce_s, .. } = read_msg(&mut recv).await.unwrap() else {
            panic!("expected HelloAck");
        };
        let wrong = auth_key("wrong");
        let mac = handshake_mac(
            &wrong,
            b"client",
            &client.id(),
            &peer.id,
            &nonce_c,
            &nonce_s,
        );
        write_msg(&mut send, &Msg::Auth { mac }).await.unwrap();
        let reply = read_msg(&mut recv).await.unwrap();
        assert!(
            matches!(&reply, Msg::Error(why) if why.contains("token")),
            "{reply:?}"
        );
        // The session is over: a request after the refusal gets no answer.
        let _ = write_msg(&mut send, &Msg::GetBuckets).await;
        assert!(read_msg(&mut recv).await.is_err());
        server.shutdown().await;
    }

    #[test]
    fn handshake_macs_bind_role_ids_and_nonces() {
        let key = auth_key("t");
        let (a, b) = (
            SecretKey::generate().public(),
            SecretKey::generate().public(),
        );
        let mac = |role: &[u8], c: &PublicKey, s: &PublicKey, nc: u8, ns: u8| {
            handshake_mac(&key, role, c, s, &[nc; 32], &[ns; 32])
        };
        let base = mac(b"client", &a, &b, 1, 2);
        assert_eq!(base, mac(b"client", &a, &b, 1, 2));
        for other in [
            mac(b"server", &a, &b, 1, 2),
            mac(b"client", &b, &a, 1, 2),
            mac(b"client", &a, &b, 3, 2),
            mac(b"client", &a, &b, 1, 3),
            handshake_mac(&auth_key("u"), b"client", &a, &b, &[1; 32], &[2; 32]),
        ] {
            assert_ne!(base, other);
        }
    }
}
