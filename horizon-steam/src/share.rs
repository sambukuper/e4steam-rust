//! The host's Steam share: a lobby carries the invite (Steam friends join
//! freely, or only by invitation), a P2P listen socket carries the game.
//! Every guest's connection is checked ([guard](crate::guard)) and then
//! pumped to the integrated server's loopback port byte for byte, as Play
//! with Friends carries its sessions: the server never sees the difference.
//! Steam encrypts the transport; identity is the Steam account.
use crate::{
    MAX_GUESTS, VIRTUAL_PORT,
    guard::{self, Opening},
    identity,
};
use std::{
    collections::HashMap,
    io::{self, Read, Write},
    net::{Shutdown, SocketAddr, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender, channel},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use steamworks::networking_sockets::NetConnection;
use steamworks::networking_types::{
    AppNetConnectionEnd, ListenSocketEvent, NetConnectionEnd, NetworkingConnectionState,
    NetworkingMessage,
};
use steamworks::{LobbyId, LobbyType, SteamId};

/// Who may join.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Steam friends join freely (e4steam's "Steam friends").
    Friends,
    /// Joining needs the host's invitation (e4steam's "Invitation only";
    /// invitations go through the Steam overlay).
    Invitation,
}

/// How the share is set up.
pub struct ShareConfig {
    /// The integrated server's loopback address (Open to LAN's port).
    pub lan: SocketAddr,
    /// The game's protocol number: a guest's lobby check refuses a mismatch
    /// before any connection is made.
    pub protocol: i32,
    /// The game's version name, for the same message.
    pub game: String,
    pub access: Access,
}

/// What the host's game shows.
#[derive(Clone, Debug, Default)]
pub struct ShareState {
    /// The lobby's address (`steam:<id>`), once Steam made the lobby.
    pub address: Option<String>,
    /// The safe names of the guests being carried.
    pub guests: Vec<String>,
    /// Why Steam sharing isn't serving, when the player asked for it.
    pub problem: Option<String>,
}

/// What the share reports, polled like a tick.
#[derive(Debug)]
pub enum ShareEvent {
    /// The lobby is ready (its address).
    Lobby(String),
    /// A guest's session is being carried.
    GuestJoined { steam_id: u64 },
    /// The guest's session ended.
    GuestLeft { steam_id: u64 },
}

struct Inner {
    config: ShareConfig,
    lobby: Mutex<Option<LobbyId>>,
    listen: Mutex<Option<steamworks::networking_sockets::ListenSocket>>,
    events: Mutex<Sender<ShareEvent>>,
    state: Mutex<ShareState>,
    guests: Mutex<HashMap<u64, ()>>,
    stop: AtomicBool,
}

impl Inner {
    fn emit(&self, event: ShareEvent) {
        let _ = self.events.lock().unwrap().send(event);
    }
    /// Whether the Steam account may connect now: room at the server and a
    /// seat in the lobby (which Steam fills by friendship or invitation,
    /// whichever the host chose).
    fn admitted(&self, steam: &crate::Steam, id: SteamId) -> bool {
        self.guests.lock().unwrap().len() < MAX_GUESTS && self.is_member(steam, id)
    }
    fn is_member(&self, steam: &crate::Steam, id: SteamId) -> bool {
        self.lobby.lock().unwrap().is_some_and(|lobby| {
            steam
                .client()
                .matchmaking()
                .lobby_members(lobby)
                .contains(&id)
        })
    }
}

/// The host's end, like `horizon_friends::Host`: started in the background,
/// the game polls it.
pub struct Share {
    inner: Arc<Inner>,
    events: Receiver<ShareEvent>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl Share {
    /// Start sharing: create the lobby (a few seconds at most), publish the
    /// address, listen for friends on virtual port 480.
    pub fn start(config: ShareConfig) -> Result<Self, String> {
        let steam = crate::acquire()?;
        let client = steam.client();
        let (tx, events) = channel();
        let (created, created_rx) = channel();
        let ty = match config.access {
            Access::Friends => LobbyType::FriendsOnly,
            Access::Invitation => LobbyType::Private,
        };
        client
            .matchmaking()
            .create_lobby(ty, crate::MAX_LOBBY_MEMBERS, move |r| {
                let _ = created.send(r);
            });
        let lobby = created_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| "Steam didn't answer the lobby request".to_owned())?
            .map_err(|e| format!("Steam refused the lobby ({e}). Restart Steam and try again."))?;
        let host = client.user().steam_id();
        let mm = client.matchmaking();
        mm.set_lobby_data(
            lobby,
            crate::LOBBY_KEY_PROTOCOL,
            &config.protocol.to_string(),
        );
        mm.set_lobby_data(lobby, crate::LOBBY_KEY_GAME, &config.game);
        mm.set_lobby_data(lobby, crate::LOBBY_KEY_HOST, &host.raw().to_string());
        if !mm.set_lobby_joinable(lobby, true) {
            return Err("Steam wouldn't make the lobby joinable".into());
        }
        let listen = client
            .networking_sockets()
            .create_listen_socket_p2p(VIRTUAL_PORT, [])
            .map_err(|_| "Steam wouldn't open the world's P2P listener".to_owned())?;
        let address = crate::lobby_address(lobby.raw());
        let inner = Arc::new(Inner {
            config,
            lobby: Mutex::new(Some(lobby)),
            listen: Mutex::new(Some(listen)),
            events: Mutex::new(tx),
            state: Mutex::new(ShareState {
                address: Some(address.clone()),
                ..ShareState::default()
            }),
            guests: Mutex::new(HashMap::new()),
            stop: AtomicBool::new(false),
        });
        inner.emit(ShareEvent::Lobby(address));
        let service = std::thread::Builder::new()
            .name("horizon-steam-share".into())
            .spawn({
                let inner = inner.clone();
                move || serve(inner, steam)
            })
            .map_err(|e| format!("Steam sharing's thread could not start: {e}"))?;
        Ok(Self {
            inner,
            events,
            threads: Mutex::new(vec![service]),
        })
    }

    pub fn state(&self) -> ShareState {
        self.inner.state.lock().unwrap().clone()
    }
    pub fn poll(&self) -> Vec<ShareEvent> {
        self.events.try_iter().collect()
    }
    /// The guests being carried over Steam.
    pub fn sessions(&self) -> usize {
        self.inner.guests.lock().unwrap().len()
    }
    /// Open the Steam overlay's invite dialog for the lobby.
    pub fn invite_friends(&self) -> Result<(), String> {
        let lobby = self
            .inner
            .lobby
            .lock()
            .unwrap()
            .ok_or_else(|| "The Steam lobby is gone".to_owned())?;
        invite_friends_overlay(lobby);
        Ok(())
    }
    /// End the share: leave the lobby, close the listener. Sessions already
    /// playing go on until the integrated server closes them, as Play with
    /// Friends' stop does.
    pub fn stop(&self) {
        self.inner.stop.store(true, Ordering::Relaxed);
        // Dropping the listen socket rejects new connections at once.
        *self.inner.listen.lock().unwrap() = None;
        if let Some(lobby) = self.inner.lobby.lock().unwrap().take()
            && let Ok(steam) = crate::acquire()
        {
            steam.client().matchmaking().leave_lobby(lobby);
        }
        for t in self.threads.lock().unwrap().drain(..) {
            let _ = t.join();
        }
    }
}

/// The service loop: accept the lobby's friends, hand each connected guest
/// to its bridge, stop when the share does.
fn serve(inner: Arc<Inner>, steam: Arc<crate::Steam>) {
    while !inner.stop.load(Ordering::Relaxed) {
        let event = inner
            .listen
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|l| l.try_receive_event());
        match event {
            Some(ListenSocketEvent::Connecting(request)) => {
                let remote = request.remote().steam_id();
                if remote.is_some_and(|id| inner.admitted(&steam, id)) {
                    // Failures surface through Disconnected.
                    let _ = request.accept();
                } else {
                    let _ = request.reject(
                        NetConnectionEnd::App(AppNetConnectionEnd::generic_normal()),
                        Some("Horizon: the world is full, or you were not invited"),
                    );
                    if remote.is_some() {
                        log::info!("Steam share: refused a guest who has no seat in the lobby");
                    }
                }
            }
            Some(ListenSocketEvent::Connected(connected)) => {
                if let Some(id) = connected.remote().steam_id() {
                    let raw = id.raw();
                    let mut guests = inner.guests.lock().unwrap();
                    if guests.len() < MAX_GUESTS && guests.insert(raw, ()).is_none() {
                        drop(guests);
                        spawn_bridge(&inner, raw, connected.take_connection());
                        inner.emit(ShareEvent::GuestJoined { steam_id: raw });
                    }
                }
            }
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// One guest: check its opening (its first bytes prove it logs in as its
/// Steam account), then carry the stream between Steam and the integrated
/// server until either side ends it.
fn spawn_bridge(inner: &Arc<Inner>, steam_id: u64, conn: NetConnection) {
    let inner = inner.clone();
    let started = std::thread::Builder::new()
        .name("horizon-steam-guest".into())
        .spawn(move || bridge(inner, steam_id, conn));
    if let Err(e) = started {
        log::warn!("Steam share: a guest's bridge could not start: {e}");
    }
}

/// How long a guest has to send a proper opening.
const OPENING: Duration = Duration::from_secs(10);
/// How much of the game's stream goes in one Steam message (e4steam's
/// DATA_CHUNK_SIZE).
const CHUNK: usize = 32 * 1024;

fn bridge(inner: Arc<Inner>, steam_id: u64, mut conn: NetConnection) {
    let end = |reason: &str| {
        log::info!("Steam share: guest {steam_id}: {reason}");
        let mut guests = inner.guests.lock().unwrap();
        if guests.remove(&steam_id).is_some() {
            inner.emit(ShareEvent::GuestLeft { steam_id });
        }
    };
    // The opening: the handshake and the login's Hello, straight from the
    // guest's first messages.
    let mut opening = Opening::default();
    let mut front: Vec<u8> = Vec::new();
    let mut fed = 0usize;
    let deadline = Instant::now() + OPENING;
    let verdict = loop {
        match opening.feed(&front[fed..]) {
            Ok(verdict) => break Some(verdict),
            Err(true) => {}
            Err(false) => break None,
        }
        fed = front.len();
        if let Some(messages) = receive(&mut conn) {
            for message in messages {
                front.extend_from_slice(message.data());
            }
        }
        if let Some(steamworks::networking_types::NetConnectionEvent {
            new_state:
                NetworkingConnectionState::ClosedByPeer
                | NetworkingConnectionState::ProblemDetectedLocally,
            ..
        }) = conn.try_receive_event()
        {
            end("hung up before logging in");
            return;
        }
        if Instant::now() > deadline {
            end("never finished logging in");
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    match verdict {
        Some(guard::Verdict::Allowed(name)) if name == identity::safe_name(steam_id) => {}
        other => {
            let reason = match other {
                Some(guard::Verdict::Refused(reason)) => reason,
                Some(_) => "logged in under another name than its Steam account",
                None => "sent an opening Horizon doesn't understand",
            };
            let _ = conn.close(
                NetConnectionEnd::App(AppNetConnectionEnd::generic_normal()),
                Some("Horizon: log in with the name of your Steam account"),
                false,
            );
            end(reason);
            return;
        }
    }
    let Ok(mut server) = TcpStream::connect_timeout(&inner.config.lan, Duration::from_secs(5))
    else {
        let _ = conn.close(
            NetConnectionEnd::App(AppNetConnectionEnd::generic_exception()),
            Some("Horizon: the world's server isn't reachable"),
            false,
        );
        end("the integrated server didn't answer");
        return;
    };
    let _ = server.set_nodelay(true);
    inner
        .state
        .lock()
        .unwrap()
        .guests
        .push(identity::safe_name(steam_id));
    inner.emit(ShareEvent::GuestJoined { steam_id });
    // The bytes read before the verdict were the guest's opening: the server
    // still needs them.
    if server.write_all(&front).is_err() {
        end("the integrated server closed the connection");
        return;
    }
    carry(&mut conn, &mut server);
    let _ = server.shutdown(Shutdown::Both);
    let name = identity::safe_name(steam_id);
    inner.state.lock().unwrap().guests.retain(|g| *g != name);
    end("the session ended");
}

/// The pump: Steam messages to the server, the server's bytes to Steam,
/// until either side ends the session. One thread per guest; the game's
/// traffic is modest, so a short poll keeps latency low without blocking
/// reads on either side.
fn carry(conn: &mut NetConnection, server: &mut TcpStream) {
    let _ = server.set_nonblocking(true);
    let mut buf = vec![0u8; CHUNK];
    loop {
        let mut idle = true;
        if let Some(messages) = receive(conn) {
            for message in messages {
                if server.write_all(message.data()).is_err() {
                    return;
                }
                idle = false;
            }
        }
        if let Some(event) = conn.try_receive_event()
            && matches!(
                event.new_state,
                NetworkingConnectionState::ClosedByPeer
                    | NetworkingConnectionState::ProblemDetectedLocally
                    | NetworkingConnectionState::None
            )
        {
            return;
        }
        match server.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => {
                let Ok(_) = conn.send_message(
                    &buf[..n],
                    steamworks::networking_types::SendFlags::RELIABLE
                        | steamworks::networking_types::SendFlags::NO_NAGLE,
                ) else {
                    return;
                };
                idle = false;
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(_) => return,
        }
        if idle {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// The connection's queued messages, or None when its handle is gone.
fn receive(conn: &mut NetConnection) -> Option<Vec<NetworkingMessage>> {
    conn.receive_messages(64).ok()
}

/// The Steam overlay's invite dialog for a lobby: the player picks friends
/// and Steam sends the invitations, as e4steam's "Invite friends" does.
pub(crate) fn invite_friends_overlay(lobby: LobbyId) {
    // steamworks has no wrapper for this one; the flat C API takes the
    // friends interface and the lobby's id.
    unsafe {
        let friends = steamworks::sys::SteamAPI_SteamFriends_v018();
        steamworks::sys::SteamAPI_ISteamFriends_ActivateGameOverlayInviteDialog(
            friends,
            lobby.raw(),
        );
    }
}
