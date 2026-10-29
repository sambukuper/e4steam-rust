//! The host's check on a guest over Steam: the login name the guest's client
//! sends must be the safe name of the Steam account the P2P connection is
//! authenticated as. e4steam derives the guest's identity on the host from
//! the SteamID and never trusts the connecting client; without the JVM to
//! swap the profile, this port checks instead: the guest's own client sends
//! its Steam-derived name ([identity::safe_name](crate::identity::safe_name)),
//! and a guest that logs in as anything else is refused before a byte of its
//! stream reaches the integrated server.
//!
//! The check reads the guest's first two packets (the handshake's Intention,
//! then the login's Hello, both uncompressed plaintext before the server
//! answers) straight from the Steam connection. Anything malformed is
//! refused: only Horizon's own client joins this way.

use crate::identity;

/// How many bytes of the guest's opening the check reads at most: the two
/// packets together are a few dozen bytes; anything past this is not a
/// Horizon join.
const MAX_OPENING: usize = 1024;

/// What the check found in the guest's opening.
#[derive(Debug)]
pub enum Verdict {
    /// The name carries a SteamID; the caller compares it with the
    /// connection's. The name is included for the comparison.
    Allowed(String),
    /// Not a Horizon join, or the name is not a Steam identity: refuse.
    Refused(&'static str),
}

/// A feed of the guest's first bytes, ending in the verdict.
#[derive(Default)]
pub struct Opening {
    buf: Vec<u8>,
}

impl Opening {
    /// The next bytes of the guest's opening (hand them over as they arrive).
    /// Returns the verdict once the Hello's name has been read, `Err(true)`
    /// while more bytes are wanted (the caller goes on feeding), and
    /// `Err(false)` when the opening is malformed or too long.
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Verdict, bool> {
        if self.buf.len() + bytes.len() > MAX_OPENING {
            return Err(false);
        }
        self.buf.extend_from_slice(bytes);
        let mut cursor = 0;
        // Intention: the handshake's first packet. Only a login intent goes on.
        let (id, body_end, rest) = packet(&self.buf, cursor).ok_or(false)?;
        if id != 0 {
            return Err(false);
        }
        cursor = body_end;
        match Intention::read(rest) {
            Some(intent) if intent.login => {}
            _ => return Err(false),
        }
        // Hello: the login's first packet, carrying the name.
        let (id, _, rest) = packet(&self.buf, cursor).ok_or(false)?;
        if id != 0 {
            return Err(false);
        }
        let Some(hello) = Hello::read(rest) else {
            return Err(false);
        };
        match identity::steam_id_of(&hello.name) {
            Some(_) => Ok(Verdict::Allowed(hello.name)),
            None => Ok(Verdict::Refused(
                "the guest's login name is not a Steam identity",
            )),
        }
    }
}

/// One packet from `buf[cursor..]`: its id, where the packet ends (an index
/// into the whole buffer) and the body. `None` while the whole packet hasn't
/// arrived.
fn packet(buf: &[u8], cursor: usize) -> Option<(i32, usize, &[u8])> {
    let (length, header) = varint(&buf[cursor..])?;
    if !(0..=4096).contains(&length) {
        return None;
    }
    let end = cursor + header + length as usize;
    if buf.len() < end {
        return None;
    }
    let (id, id_len) = varint(&buf[cursor + header..end])?;
    Some((id, end, &buf[cursor + header + id_len..end]))
}

/// The handshake's Intention: protocol, host, port, next state.
struct Intention {
    login: bool,
}

impl Intention {
    fn read(body: &[u8]) -> Option<Self> {
        let (_, used) = varint(body)?; // protocol version
        let rest = &body[used..];
        let (host_len, used) = varint(rest)?;
        if host_len < 0 || rest.len() < used + host_len as usize + 3 {
            return None;
        }
        let rest = &rest[used + host_len as usize..];
        let (state, _) = varint(&rest[2..])?; // past the port
        Some(Self { login: state == 2 })
    }
}

/// The login's Hello: the player's name (and the client's uuid, which the
/// offline server ignores).
struct Hello {
    name: String,
}

impl Hello {
    fn read(body: &[u8]) -> Option<Self> {
        let (len, used) = varint(body)?;
        if len < 0 || body.len() < used + len as usize {
            return None;
        }
        let name = String::from_utf8(body[used..used + len as usize].to_vec()).ok()?;
        if name.chars().count() > 16 {
            return None;
        }
        Some(Self { name })
    }
}

/// A protocol varint: the value and how many bytes it took.
fn varint(buf: &[u8]) -> Option<(i32, usize)> {
    let mut value = 0i32;
    for (i, &b) in buf.iter().take(5).enumerate() {
        value |= i32::from(b & 0x7f) << (i * 7);
        if b & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_varint(mut value: i32, out: &mut Vec<u8>) {
        loop {
            let byte = (value & 0x7f) as u8;
            value = ((value as u32) >> 7) as i32;
            if value == 0 {
                out.push(byte);
                return;
            }
            out.push(byte | 0x80);
        }
    }

    fn put_string(s: &str, out: &mut Vec<u8>) {
        put_varint(s.len() as i32, out);
        out.extend_from_slice(s.as_bytes());
    }

    fn frame(id: i32, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        put_varint(body.len() as i32 + 1, &mut out); // + the packet id
        put_varint(id, &mut out);
        out.extend_from_slice(body);
        out
    }

    /// Intention (login) and Hello packets as Horizon's client sends them.
    fn opening(name: &str) -> Vec<u8> {
        let mut handshake = Vec::new();
        put_varint(763, &mut handshake);
        put_string("127.0.0.1", &mut handshake);
        handshake.extend_from_slice(&25565u16.to_be_bytes());
        put_varint(2, &mut handshake); // next state: login
        let mut hello = Vec::new();
        put_string(name, &mut hello);
        hello.extend_from_slice(&[0u8; 16]); // the client's uuid
        [frame(0, &handshake), frame(0, &hello)].concat()
    }

    fn verdict(name: &str) -> Result<Verdict, bool> {
        let mut check = Opening::default();
        check.feed(&opening(name))
    }

    #[test]
    fn a_steam_safe_name_passes() {
        assert!(matches!(
            verdict("s_kxuogshzmyo"),
            Ok(Verdict::Allowed(name)) if name == "s_kxuogshzmyo"
        ));
    }

    #[test]
    fn any_other_name_is_refused() {
        for name in ["Steve", "host", "s_"] {
            assert!(
                matches!(verdict(name), Ok(Verdict::Refused(_))),
                "{name} was not refused"
            );
        }
    }

    #[test]
    fn bytes_arriving_in_pieces_still_parse() {
        let bytes = opening("s_3dk6el9t85j");
        let mut check = Opening::default();
        let mut verdict = Err(true);
        for chunk in bytes.chunks(3) {
            verdict = check.feed(chunk);
            if verdict.is_ok() {
                break;
            }
        }
        assert!(matches!(verdict, Ok(Verdict::Allowed(_))));
    }

    #[test]
    fn a_status_ping_is_not_a_join() {
        // next state 1: the client asks the server's status, it isn't joining.
        let mut handshake = Vec::new();
        put_varint(763, &mut handshake);
        put_string("127.0.0.1", &mut handshake);
        handshake.extend_from_slice(&25565u16.to_be_bytes());
        put_varint(1, &mut handshake);
        let mut check = Opening::default();
        assert!(matches!(check.feed(&frame(0, &handshake)), Err(false)));
    }

    #[test]
    fn garbage_is_refused_without_hanging() {
        let mut check = Opening::default();
        assert!(matches!(check.feed(&[0xff; 40]), Err(false)));
    }
}
