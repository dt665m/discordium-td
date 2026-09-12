//! Player IDs are first-claim identities protected by a locally saved random
//! credential. The server stores only its BLAKE3 digest, never the secret.
use dreamwake_protocol::player::{PlayerCredential, PlayerId};
use renet_cross::{BootstrapAuthError, SessionAdmission, SessionCreateRequest, SessionGrant};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

const MAX_IDENTITIES: usize = 4096;
const MAGIC: &[u8; 8] = b"DWIDS001";
const GRANT_MAGIC: &[u8; 5] = b"DWID1";

#[derive(Default)]
pub(crate) struct GuestAdmission {
    identities: Mutex<BTreeMap<u64, blake3::Hash>>,
    path: Option<PathBuf>,
    _lock: Option<File>,
}
impl GuestAdmission {
    /// One server process owns this registry file. Atomic replacement preserves
    /// the previous complete registry when writing the next revision fails.
    pub(crate) fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_owned();
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        let mut lock_path = path.as_os_str().to_owned();
        lock_path.push(".lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options.open(PathBuf::from(lock_path))?;
        lock.try_lock().map_err(|_| {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                "player identity registry is already owned by another server",
            )
        })?;
        let identities = match File::open(&path) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take((12 + MAX_IDENTITIES * 40 + 1) as u64)
                    .read_to_end(&mut bytes)?;
                decode_store(&bytes)?
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e),
        };
        Ok(Self {
            identities: Mutex::new(identities),
            path: Some(path),
            _lock: Some(lock),
        })
    }
    fn verify_or_register(&self, credential: &PlayerCredential) -> Result<(), BootstrapAuthError> {
        let mut identities = self
            .identities
            .lock()
            .map_err(|_| denied("Player identity store is unavailable."))?;
        let id = credential.player().get();
        let digest = blake3::hash(credential.secret());
        if let Some(existing) = identities.get(&id) {
            // blake3::Hash's PartialEq is explicitly constant-time.
            return if *existing == digest {
                Ok(())
            } else {
                Err(denied("Player ID belongs to another saved profile."))
            };
        }
        if identities.len() >= MAX_IDENTITIES {
            return Err(denied("Player identity registry is full."));
        }
        if let Some(path) = &self.path {
            let mut next = identities.clone();
            next.insert(id, digest);
            persist(path, &next).map_err(|_| denied("Player identity store is unavailable."))?;
        }
        identities.insert(id, digest);
        Ok(())
    }
}
fn denied(message: &str) -> BootstrapAuthError {
    BootstrapAuthError::Message {
        message: message.into(),
    }
}
pub(crate) fn grant_player(grant: &SessionGrant) -> Option<PlayerId> {
    if grant.protocol_id != dreamwake_protocol::PROTOCOL_ID
        || grant.service != dreamwake_protocol::SESSION_SERVICE
        || grant.match_id != dreamwake_protocol::SESSION_MATCH
        || grant.application.len() != 13
        || &grant.application[..5] != GRANT_MAGIC
    {
        return None;
    }
    PlayerId::new(u64::from_be_bytes(grant.application[5..].try_into().ok()?)).ok()
}
impl SessionAdmission for GuestAdmission {
    fn admit(
        &self,
        request: &SessionCreateRequest,
        now: Duration,
    ) -> Result<SessionGrant, BootstrapAuthError> {
        request.validate()?;
        if request.protocol_id != Some(dreamwake_protocol::PROTOCOL_ID)
            || request.service != dreamwake_protocol::SESSION_SERVICE
            || request.match_id != dreamwake_protocol::SESSION_MATCH
            || !request.require_secure
        {
            return Err(BootstrapAuthError::InvalidRequest);
        }
        let credential = PlayerCredential::parse(&request.credential)
            .map_err(|_| BootstrapAuthError::InvalidRequest)?;
        let expires_at = now
            .as_secs()
            .checked_add(60)
            .ok_or(BootstrapAuthError::InvalidGrant)?;
        self.verify_or_register(&credential)?;
        let mut application = GRANT_MAGIC.to_vec();
        application.extend_from_slice(&credential.player().get().to_be_bytes());
        Ok(SessionGrant {
            protocol_id: dreamwake_protocol::PROTOCOL_ID,
            service: dreamwake_protocol::SESSION_SERVICE.into(),
            match_id: dreamwake_protocol::SESSION_MATCH.into(),
            expires_at,
            replay_key: renet_cross::generate_random_bytes(),
            application,
        })
    }
}
fn decode_store(bytes: &[u8]) -> io::Result<BTreeMap<u64, blake3::Hash>> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid player identity registry",
        )
    };
    if bytes.len() < 12 || &bytes[..8] != MAGIC {
        return Err(invalid());
    }
    let count = u32::from_be_bytes(bytes[8..12].try_into().unwrap()) as usize;
    if count > MAX_IDENTITIES || bytes.len() != 12 + count * 40 {
        return Err(invalid());
    }
    let mut identities = BTreeMap::new();
    for entry in bytes[12..].chunks_exact(40) {
        let id = u64::from_be_bytes(entry[..8].try_into().unwrap());
        if PlayerId::new(id).is_err()
            || identities
                .insert(id, blake3::Hash::from_bytes(entry[8..].try_into().unwrap()))
                .is_some()
        {
            return Err(invalid());
        }
    }
    Ok(identities)
}
fn persist(path: &Path, identities: &BTreeMap<u64, blake3::Hash>) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let suffix = u64::from_le_bytes(renet_cross::generate_random_bytes());
    let temporary = parent.join(format!(".dreamwake-identities-{suffix:016x}.tmp"));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(MAGIC)?;
        file.write_all(&(identities.len() as u32).to_be_bytes())?;
        for (&id, hash) in identities {
            file.write_all(&id.to_be_bytes())?;
            file.write_all(hash.as_bytes())?;
        }
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        // Once renamed, keep the in-memory revision in agreement even if the
        // filesystem cannot sync directory metadata. The file itself is synced.
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(id: u64, secret: u8) -> SessionCreateRequest {
        SessionCreateRequest {
            protocol_id: Some(dreamwake_protocol::PROTOCOL_ID),
            service: dreamwake_protocol::SESSION_SERVICE.into(),
            match_id: dreamwake_protocol::SESSION_MATCH.into(),
            require_secure: true,
            credential: PlayerCredential::new(PlayerId::new(id).unwrap(), [secret; 32])
                .unwrap()
                .encode(),
            ..Default::default()
        }
    }
    #[test]
    fn first_claim_resume_and_wrong_profile_preserve_exact_identity() {
        let admission = GuestAdmission::default();
        let first = admission
            .admit(&request(PlayerId::MAX, 42), Duration::from_secs(100))
            .unwrap();
        assert_eq!(grant_player(&first).unwrap().get(), PlayerId::MAX);
        assert_eq!(first.expires_at, 160);
        let second = admission
            .admit(&request(PlayerId::MAX, 42), Duration::from_secs(101))
            .unwrap();
        assert_ne!(first.replay_key, second.replay_key);
        assert!(
            admission
                .admit(&request(PlayerId::MAX, 43), Duration::ZERO)
                .is_err()
        );
        assert_eq!(admission.identities.lock().unwrap().len(), 1);
        for bad in [
            SessionCreateRequest {
                credential: "".into(),
                ..request(1, 42)
            },
            SessionCreateRequest {
                require_secure: false,
                ..request(1, 42)
            },
            SessionCreateRequest {
                match_id: "other".into(),
                ..request(1, 42)
            },
            SessionCreateRequest {
                service: "other".into(),
                ..request(1, 42)
            },
            SessionCreateRequest {
                protocol_id: Some(0),
                ..request(1, 42)
            },
        ] {
            assert!(admission.admit(&bad, Duration::ZERO).is_err());
        }
    }
    #[test]
    fn atomic_private_registry_survives_restart_without_persisting_secret() {
        let suffix = u64::from_le_bytes(renet_cross::generate_random_bytes());
        let directory = std::env::temp_dir().join(format!("dreamwake-identity-test-{suffix:016x}"));
        let path = directory.join("players.bin");
        let admission = GuestAdmission::open(&path).unwrap();
        admission.admit(&request(7, 99), Duration::ZERO).unwrap();
        assert!(
            GuestAdmission::open(&path).is_err(),
            "one writer owns the registry"
        );
        drop(admission);
        let stored = fs::read(&path).unwrap();
        assert_eq!(stored.len(), 52);
        assert!(!stored.windows(32).any(|bytes| bytes == [99; 32]));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let reopened = GuestAdmission::open(&path).unwrap();
        assert!(reopened.admit(&request(7, 99), Duration::ZERO).is_ok());
        assert!(reopened.admit(&request(7, 98), Duration::ZERO).is_err());
        assert!(reopened.admit(&request(8, 98), Duration::ZERO).is_ok());
        drop(reopened);
        assert_eq!(
            GuestAdmission::open(&path)
                .unwrap()
                .identities
                .lock()
                .unwrap()
                .len(),
            2
        );
        fs::write(&path, b"truncated").unwrap();
        assert!(GuestAdmission::open(&path).is_err());
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn storage_and_registration_are_bounded_and_malformed_grants_fail_closed() {
        let admission = GuestAdmission::default();
        for id in 1..=MAX_IDENTITIES as u64 {
            admission
                .identities
                .lock()
                .unwrap()
                .insert(id, blake3::hash(&[1; 32]));
        }
        assert!(admission.admit(&request(1, 1), Duration::ZERO).is_ok());
        assert!(
            admission
                .admit(&request(MAX_IDENTITIES as u64 + 1, 1), Duration::ZERO)
                .is_err()
        );
        assert!(decode_store(&vec![0; 12 + MAX_IDENTITIES * 40 + 1]).is_err());
        let mut grant = admission.admit(&request(1, 1), Duration::ZERO).unwrap();
        grant.application[5..].copy_from_slice(&0u64.to_be_bytes());
        assert!(grant_player(&grant).is_none());
        grant.application = b"guest-v1".to_vec();
        assert!(grant_player(&grant).is_none());
    }
}
