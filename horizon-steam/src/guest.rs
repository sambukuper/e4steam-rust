//! The guest's end: join the host's lobby, check it is a Horizon world of
//! this game's version, connect to the host over Steam's P2P network, then
//! offer the session on a loopback port the client connects to like to any
//! server. Joining logs in as this player's own Steam identity
//! ([identity](crate::identity)); the host's check ([guard](crate::guard))
//! expects exactly that.
use crate::VIRTUAL_PORT;
use std::{
    fmt, io,
    net::{Ipv4Addr, TcpListener, TcpStream},
    thread::JoinHandle,
    time::{Duration, Instant},
};
use steamworks::networking_sockets::NetConnection;
use steamworks::networking_types::{
    AppNetConnectionEnd, NetConnectionEnd, NetworkingConnectionState, NetworkingIdentity,
};
use steamworks::{LobbyId, SteamId};

/// How a join is set up.
pub struct GuestConfig {
    /// The lobby as pasted: a `steam:<id>` address, or the bare id.
    pub lobby: String,
    /// This game's protocol number: a lobby whose protocol differs is
    /// refused before connecting, as the friends relay refuses a guest.
    pub protocol: i32,
    /// This game's version name, for the mismatch message.
    pub game: String,
}

/// Why joining failed, in words for the player.
#[derive(Debug)]
pub enum JoinError {
    /// Steam isn't running or refused the game.
    NoSteam(String),
    /// Not a `steam:<id>` address.
    BadAddress,
    /// No lobby with this id, or it ended.
    LobbyGone,
    /// The lobby holds no Horizon metadata: it belongs to another program on
    /// the shared App ID 480, as e4steam filters its own traffic.
    NotHorizon,
    /// The host's world is another game version.
    VersionMismatch {
        host: Option<String>,
        mine: String,
    },
    /// The lobby is full.
    Full,
    /// The host's game didn't take the connection.
    HostUnreachable,
    Failed(String),
}
impl fmt::Display for JoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSteam(e) => write!(f, "{e}"),
            Self::BadAddress => write!(
                f,
                "That's not a Steam world's address: it should be steam: followed by a number from your friend's Play with Friends screen."
            ),
            Self::LobbyGone => write!(
                f,
                "No Steam world uses this address. Check it, or ask your friend to start sharing again."
            ),
            Self::NotHorizon => write!(
                f,
                "This Steam lobby isn't a Horizon world. Check the address."
            ),
            Self::VersionMismatch {
                host: Some(v),
                mine,
            } => {
                write!(f, "Your friend plays Minecraft {v}; you have {mine}.")
            }
            Self::VersionMismatch { host: None, mine } => {
                write!(
                    f,
                    "Your friend plays another version of Minecraft than {mine}."
                )
            }
            Self::Full => write!(f, "Your friend's world is full."),
            Self::HostUnreachable => write!(f, "Your friend's game didn't answer."),
            Self::Failed(e) => write!(f, "Couldn't connect to your friend ({e})."),
        }
    }
}
impl std::error::Error for JoinError {}

/// A settled session: the Steam connection carrying the game.
pub struct Tunnel {
    conn: NetConnection,
}

/// Join the lobby of the address: read the host's metadata, connect, wait
/// for Steam to route the two. Blocks for a few seconds.
pub fn join(config: &GuestConfig) -> Result<Tunnel, JoinError> {
    let steam = crate::acquire().map_err(JoinError::NoSteam)?;
    let id = crate::parse_lobby_address(config.lobby.trim())
        .or_else(|| config.lobby.trim().parse().ok())
        .ok_or(JoinError::BadAddress)?;
    let client = steam.client();
    let (joined, joined_rx) = std::sync::mpsc::channel();
    client
        .matchmaking()
        .join_lobby(LobbyId::from_raw(id), move |r| {
            let _ = joined.send(r);
        });
    let lobby = joined_rx
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| JoinError::Failed("Steam didn't answer the join".into()))?
        .map_err(|_| JoinError::LobbyGone)?;
    let mm = client.matchmaking();
    // The lobby's own metadata says whether it is a Horizon world at all and
    // whether it is this game's version: on the shared App ID 480 that is
    // what keeps another program's lobby out, as e4steam filters its traffic.
    let protocol = mm.lobby_data(lobby, crate::LOBBY_KEY_PROTOCOL);
    let Some(protocol) = protocol.and_then(|p| p.parse::<i32>().ok()) else {
        mm.leave_lobby(lobby);
        return Err(JoinError::NotHorizon);
    };
    if protocol != config.protocol {
        mm.leave_lobby(lobby);
        return Err(JoinError::VersionMismatch {
            host: mm.lobby_data(lobby, crate::LOBBY_KEY_GAME),
            mine: config.game.clone(),
        });
    }
    let host = mm
        .lobby_data(lobby, crate::LOBBY_KEY_HOST)
        .and_then(|h| h.parse::<u64>().ok())
        .filter(|h| *h != 0)
        .unwrap_or_else(|| mm.lobby_owner(lobby).raw());
    if mm
        .lobby_member_limit(lobby)
        .is_some_and(|limit| mm.lobby_member_count(lobby) >= limit)
    {
        mm.leave_lobby(lobby);
        return Err(JoinError::Full);
    }
    let conn = client
        .networking_sockets()
        .connect_p2p(
            NetworkingIdentity::new_steam_id(SteamId::from_raw(host)),
            VIRTUAL_PORT,
            [],
        )
        .map_err(|_| {
            mm.leave_lobby(lobby);
            JoinError::HostUnreachable
        })?;
    // Steam finds the route (direct when it can, Valve's relays when not);
    // the host's answer completes it.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match conn.try_receive_event() {
            Some(event) => match event.new_state {
                NetworkingConnectionState::Connected => break,
                NetworkingConnectionState::ClosedByPeer
                | NetworkingConnectionState::ProblemDetectedLocally
                | NetworkingConnectionState::None => {
                    mm.leave_lobby(lobby);
                    return Err(JoinError::HostUnreachable);
                }
                _ => {}
            },
            None => {
                if Instant::now() > deadline {
                    mm.leave_lobby(lobby);
                    return Err(JoinError::HostUnreachable);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
    // The host checks that we log in as our own Steam identity: the login
    // name is decided here, in net.rs, from the same derivation.
    Ok(Tunnel { conn })
}

impl Tunnel {
    /// Offer the session on `127.0.0.1:port` for one connection (the game's
    /// client), as the friends tunnel does; the returned thread ends when
    /// that connection does.
    pub fn serve_loopback(self) -> io::Result<(u16, JoinHandle<()>)> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let thread = std::thread::Builder::new()
            .name("horizon-steam-guest".into())
            .spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(30);
                let plain = loop {
                    match listener.accept() {
                        Ok((s, from)) if from.ip().is_loopback() => break Some(s),
                        Ok(_) => {}
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            if Instant::now() > deadline {
                                break None;
                            }
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(_) => break None,
                    }
                };
                drop(listener);
                let mut conn = self.conn;
                let Some(mut plain) = plain else {
                    let _ = conn.close(
                        NetConnectionEnd::App(AppNetConnectionEnd::generic_normal()),
                        None,
                        false,
                    );
                    return;
                };
                let _ = plain.set_nodelay(true);
                carry(&mut conn, &mut plain);
            })?;
        Ok((port, thread))
    }
}

/// The pump: Steam messages to the game's client, its bytes to Steam, until
/// either side ends. Both sides are polled (a blocking read could sit on one
/// while the other side's answer waits), a millisecond when idle.
fn carry(conn: &mut NetConnection, plain: &mut TcpStream) {
    use std::io::{Read, Write};
    let _ = plain.set_nonblocking(true);
    let mut buf = vec![0u8; 32 * 1024];
    loop {
        let mut idle = true;
        if let Ok(messages) = conn.receive_messages(64) {
            for message in messages {
                if plain.write_all(message.data()).is_err() {
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
        match plain.read(&mut buf) {
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
