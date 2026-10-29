//! Play with Steam friends: e4steam's multiplayer, ported to Horizon.
//!
//! e4steam is a Java mod (Fabric / Forge / NeoForge) that opens a singleplayer
//! world to Steam friends without port forwarding; Java mod jars do not run on
//! Horizon, so this crate brings what it does natively: the host creates a
//! Steam lobby behind its world, friends join from a Steam invite or the
//! lobby's address, and the game's stream travels over Steam's encrypted P2P
//! networking between the two ends of the integrated server — the same shape
//! as Play with Friends ([horizon_friends]), with Steam as the transport.
//!
//! What is carried over from the mod, and how:
//!
//! - **App ID 480.** e4steam permanently uses Valve's shared test App ID
//!   (Spacewar) so no Steam app of its own is needed, and filters unrelated
//!   App ID 480 traffic. So does this port: the lobby holds Horizon's own
//!   metadata keys (below) and a join is refused when they are missing, so a
//!   lobby from any other App ID 480 program is not joinable by mistake.
//! - **Access modes.** A *Steam friends* lobby (friends join freely) or an
//!   *invitation only* one (joining needs the host's invite), like the mod's
//!   "Open to LAN → Steam friends" and "Invitation only".
//! - **Identity.** e4steam 0.3.0 derives each guest's stable Minecraft
//!   identity from the authenticated SteamID instead of trusting the name the
//!   client sends. This port keeps the mod's scheme ([identity]): the name
//!   `s_<steamid in base 36>` and the UUID the mod derives for the SteamID.
//!   The host's tunnel checks the name the guest logs in with against its
//!   Steam connection's identity and refuses a mismatch, so a guest cannot
//!   join under another name than its Steam account's ([guard]).
//! - **Virtual port 480** for the listen socket, as the mod's dedicated
//!   transport uses.
//!
//! Not ported (yet): the dedicated-server Steam mode (the mod's anonymous
//! GameServer sign-in and `d-...steam` addresses) and the UDP voice-chat
//! bridge (Horizon has no Java voice mods). The `steamworks` crate version
//! and this crate's lobby keys are not shared with the Java mod: a Java
//! e4steam host and a Horizon guest cannot interop (Horizon cannot run the
//! mod's Java code either way).
//!
//! [horizon_friends]: https://docs.rs/horizon-friends
pub mod guard;
pub mod guest;
pub mod identity;
pub mod share;

use steamworks::Client;

/// Steam's shared test App ID (Spacewar), as e4steam uses.
pub const APP_ID: u32 = 480;
/// The virtual port of the host's P2P listen socket (e4steam's own).
pub const VIRTUAL_PORT: i32 = 480;
/// The address prefix a guest pastes: `steam:<lobby id>` (the host's screen
/// shows and copies it).
pub const ADDRESS_PREFIX: &str = "steam:";
/// Lobby metadata: the game's protocol number, as e4steam keys its lobbies.
pub const LOBBY_KEY_PROTOCOL: &str = "horizon_protocol";
/// Lobby metadata: the game's version name, for the mismatch message.
pub const LOBBY_KEY_GAME: &str = "horizon_game";
/// Lobby metadata: the host's SteamID (where the P2P connection goes).
pub const LOBBY_KEY_HOST: &str = "horizon_host";
/// Guests at once, like the integrated server's own limit.
pub const MAX_GUESTS: usize = 8;
/// Lobby members at once (the host is one of them).
pub const MAX_LOBBY_MEMBERS: u32 = MAX_GUESTS as u32 + 1;

/// Something the Steam runtime told us, on the callback thread.
#[derive(Clone, Copy, Debug)]
pub enum SteamEvent {
    /// A lobby's membership changed (`LobbyChatUpdate`).
    LobbyMemberChanged {
        lobby: u64,
        member: u64,
        entered: bool,
    },
    /// The player accepted a Steam invite while the game runs
    /// (`GameLobbyJoinRequested`): the lobby to join.
    JoinRequested { lobby: u64 },
}

/// The process's Steam runtime: the client and the thread that pumps its
/// callbacks. One per process; acquired on demand and never shut down (the
/// game ends with the process).
pub struct Steam {
    client: Client,
    subscribers: std::sync::Mutex<Vec<std::sync::mpsc::Sender<SteamEvent>>>,
}

static RUNTIME: std::sync::OnceLock<Result<std::sync::Arc<Steam>, String>> =
    std::sync::OnceLock::new();

/// The runtime, started on first use: `Err` when Steam isn't running or its
/// API refused us (the message is for the player, in e4steam's
/// troubleshooting spirit).
pub fn acquire() -> Result<std::sync::Arc<Steam>, String> {
    RUNTIME
        .get_or_init(start)
        .as_ref()
        .map_err(|e| e.clone())
        .cloned()
}

/// Whether Steam sharing can be offered (Steam is running and signed in).
/// False without the client's `steam` feature: the runtime is not linked.
pub fn available() -> bool {
    acquire().is_ok()
}

/// This player's SteamID, when Steam is available.
pub fn own_steam_id() -> Option<u64> {
    acquire().ok().map(|s| s.client.user().steam_id().raw())
}

/// This player's persona name, when Steam is available.
pub fn persona_name() -> Option<String> {
    acquire().ok().map(|s| s.client.friends().name())
}

/// Receive the events Steam's callbacks produce (invites accepted, lobby
/// members coming and going). Each call adds a receiver; drop it to leave.
pub fn subscribe() -> Result<std::sync::mpsc::Receiver<SteamEvent>, String> {
    let steam = acquire()?;
    let (tx, rx) = std::sync::mpsc::channel();
    steam.subscribers.lock().unwrap().push(tx);
    Ok(rx)
}

fn start() -> Result<std::sync::Arc<Steam>, String> {
    // App ID 480 without a steam_appid.txt beside the game: the SDK reads the
    // `SteamAppId` environment variable, which `init_app` sets.
    let client = Client::init_app(APP_ID)
        .map_err(|e| format!("Steam isn't running, or refused Horizon ({e}). Start Steam and sign in, then restart the game."))?;
    let steam = std::sync::Arc::new(Steam {
        client,
        subscribers: std::sync::Mutex::new(Vec::new()),
    });
    // One thread pumps every callback, all the time: lobby results, invites
    // and the networking sockets' connection events are delivered during it.
    std::thread::Builder::new()
        .name("horizon-steam-callbacks".into())
        .spawn({
            let steam = steam.clone();
            move || loop {
                steam.client.run_callbacks();
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        })
        .map_err(|e| format!("Steam's callback thread could not start: {e}"))?;
    steam.client.register_callback({
        let steam = steam.clone();
        move |update: steamworks::LobbyChatUpdate| {
            steam.announce(SteamEvent::LobbyMemberChanged {
                lobby: update.lobby.raw(),
                member: update.user_changed.raw(),
                entered: update.member_state_change == steamworks::ChatMemberStateChange::Entered,
            });
        }
    });
    steam.client.register_callback({
        let steam = steam.clone();
        move |requested: steamworks::GameLobbyJoinRequested| {
            // The player accepted an invite in the overlay; the client's
            // Steam screen turns it into a join.
            steam.announce(SteamEvent::JoinRequested {
                lobby: requested.lobby_steam_id.raw(),
            });
        }
    });
    log::info!(
        "Steam is ready: app id {APP_ID}, persona '{}'",
        persona_name().unwrap_or_default()
    );
    Ok(steam)
}

impl Steam {
    /// Send an event to every subscriber.
    pub(crate) fn announce(&self, event: SteamEvent) {
        self.subscribers
            .lock()
            .unwrap()
            .retain(|tx| tx.send(event).is_ok());
    }
    pub(crate) fn client(&self) -> &Client {
        &self.client
    }
}

/// The address a host's lobby is joined by: `steam:<lobby id>`.
pub fn lobby_address(lobby: u64) -> String {
    format!("{ADDRESS_PREFIX}{lobby}")
}

/// The lobby id in a `steam:<lobby id>` address.
pub fn parse_lobby_address(address: &str) -> Option<u64> {
    address
        .strip_prefix(ADDRESS_PREFIX)
        .and_then(|id| id.trim().parse().ok())
        .filter(|id| *id != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lobby_addresses_round_trip() {
        let id = 109775241234567890;
        assert_eq!(parse_lobby_address(&lobby_address(id)), Some(id));
        assert_eq!(parse_lobby_address("steam: 109775241234567890"), Some(id));
        assert_eq!(parse_lobby_address("steam:0"), None);
        assert_eq!(parse_lobby_address("steam:"), None);
        assert_eq!(parse_lobby_address("steam:none"), None);
        assert_eq!(parse_lobby_address("friend:K7QM-3XPD"), None);
    }
}
