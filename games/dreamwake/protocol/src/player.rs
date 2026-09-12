//! Stable game identity and the opaque credential carried only by secure bootstrap.
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// The high bit belongs to Dreamwake world entities. A transport connection has
/// its own independent u64 identity and never grants ownership by matching this ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct PlayerId(u64);
impl PlayerId {
    pub const MAX: u64 = i64::MAX as u64;
    pub const fn new(value: u64) -> Result<Self, InvalidPlayerId> {
        if value == 0 || value > Self::MAX {
            Err(InvalidPlayerId)
        } else {
            Ok(Self(value))
        }
    }
    pub const fn get(self) -> u64 {
        self.0
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidPlayerId;
impl fmt::Display for InvalidPlayerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Enter a whole-number ID from 1 to 9223372036854775807.")
    }
}
impl std::error::Error for InvalidPlayerId {}
impl FromStr for PlayerId {
    type Err = InvalidPlayerId;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.is_empty()
            || text.len() > 19
            || text.starts_with('0')
            || !text.bytes().all(|v| v.is_ascii_digit())
        {
            return Err(InvalidPlayerId);
        }
        Self::new(text.parse::<u64>().map_err(|_| InvalidPlayerId)?)
    }
}
impl TryFrom<u64> for PlayerId {
    type Error = InvalidPlayerId;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<PlayerId> for u64 {
    fn from(value: PlayerId) -> u64 {
        value.0
    }
}
impl fmt::Display for PlayerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Public numeric ID plus a saved 256-bit possession secret. Never replicate or log.
#[derive(Clone)]
pub struct PlayerCredential {
    player: PlayerId,
    secret: [u8; 32],
}
impl fmt::Debug for PlayerCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlayerCredential")
            .field("player", &self.player)
            .field("secret", &"[redacted]")
            .finish()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidPlayerCredential;
impl fmt::Display for InvalidPlayerCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Invalid saved Traveler credential.")
    }
}
impl std::error::Error for InvalidPlayerCredential {}
impl PlayerCredential {
    pub fn new(player: PlayerId, secret: [u8; 32]) -> Result<Self, InvalidPlayerCredential> {
        if secret == [0; 32] {
            return Err(InvalidPlayerCredential);
        }
        Ok(Self { player, secret })
    }
    pub fn player(&self) -> PlayerId {
        self.player
    }
    pub fn secret(&self) -> &[u8; 32] {
        &self.secret
    }
    pub fn encode(&self) -> String {
        use std::fmt::Write;
        let mut value = format!("dw-player-v1:{}:", self.player);
        for byte in self.secret {
            write!(value, "{byte:02x}").expect("String writes succeed");
        }
        value
    }
    pub fn parse(value: &str) -> Result<Self, InvalidPlayerCredential> {
        if value.len() > 97 {
            return Err(InvalidPlayerCredential);
        }
        let value = value
            .strip_prefix("dw-player-v1:")
            .ok_or(InvalidPlayerCredential)?;
        let (player, secret) = value.split_once(':').ok_or(InvalidPlayerCredential)?;
        let player = player.parse().map_err(|_| InvalidPlayerCredential)?;
        if secret.len() != 64
            || !secret
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(InvalidPlayerCredential);
        }
        let mut bytes = [0; 32];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&secret[i * 2..i * 2 + 2], 16)
                .map_err(|_| InvalidPlayerCredential)?;
        }
        Self::new(player, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ids_reject_reserved_or_ambiguous_values_without_float_conversion() {
        for text in [
            "",
            "0",
            "01",
            "+1",
            "-1",
            "1.0",
            "1e3",
            " 1",
            "1 ",
            "１２",
            "9223372036854775808",
            "18446744073709551615",
        ] {
            assert!(text.parse::<PlayerId>().is_err(), "{text}");
        }
        for value in [1, 9007199254740993, PlayerId::MAX] {
            assert_eq!(value.to_string().parse::<PlayerId>().unwrap().get(), value);
        }
        for value in [0, 1_u64 << 63] {
            assert!(
                PlayerId::deserialize(
                    serde::de::value::U64Deserializer::<serde::de::value::Error>::new(value)
                )
                .is_err()
            );
        }
    }
    #[test]
    fn credential_is_bounded_canonical_and_redacted() {
        let credential =
            PlayerCredential::new(PlayerId::new(PlayerId::MAX).unwrap(), [0x42; 32]).unwrap();
        let text = credential.encode();
        assert_eq!(
            PlayerCredential::parse(&text).unwrap().secret(),
            credential.secret()
        );
        assert!(!format!("{credential:?}").contains("424242"));
        for text in [
            text.to_uppercase(),
            format!("{text}x"),
            text.replace("dw-player-v1", "dw-player-v0"),
            format!("dw-player-v1:1:{}", "0".repeat(64)),
        ] {
            assert!(PlayerCredential::parse(&text).is_err());
        }
    }
}
