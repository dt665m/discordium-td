//! Local Traveler credentials. Only the numeric ID is presentation data.
use bevy::prelude::*;
use dreamwake_protocol::player::{PlayerCredential, PlayerId};
use std::sync::{Mutex, mpsc};

const MAX_PROFILES: usize = 64;
const MAX_STORE_BYTES: usize = 16_384;
#[cfg(target_arch = "wasm32")]
const STORAGE_KEY: &str = "dreamwake.traveler-profiles.v1";

#[derive(Clone)]
struct SavedProfile {
    server: String,
    credential: PlayerCredential,
}
enum Store {
    Memory,
    #[cfg(not(target_arch = "wasm32"))]
    File(std::path::PathBuf),
    #[cfg(target_arch = "wasm32")]
    Browser,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectionStatus {
    Ready,
    // Native persistence is synchronous; callers share the same result API.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    Pending,
}
#[derive(Clone, Copy)]
enum SelectionRequest {
    // Browser startup restores the per-tab selection inside the Web Lock.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    Restore,
    Select(PlayerId),
}
struct SavedDocument {
    selected: PlayerId,
    profiles: Vec<SavedProfile>,
}
struct PreparedSelection {
    server: String,
    selected: PlayerCredential,
    profiles: Vec<SavedProfile>,
    bytes: Vec<u8>,
}
type SelectionCompletion = Result<PreparedSelection, String>;

#[derive(Resource)]
pub(crate) struct TravelerProfile {
    server: String,
    selected: PlayerCredential,
    profiles: Vec<SavedProfile>,
    store: Store,
    locked: bool,
    initialized: bool,
    pending: Option<Mutex<mpsc::Receiver<SelectionCompletion>>>,
    error: Option<String>,
    read_failed: bool,
}
impl Default for TravelerProfile {
    fn default() -> Self {
        Self::empty("http://127.0.0.1:8080".into(), Store::Memory)
    }
}
impl TravelerProfile {
    fn empty(server: String, store: Store) -> Self {
        let random: [u8; 32] = renet_cross::generate_random_bytes();
        let id = PlayerId::new(
            u64::from_le_bytes(random[..8].try_into().unwrap()) % 900_000_000 + 100_000_000,
        )
        .unwrap();
        Self {
            server,
            selected: fresh_credential(id),
            profiles: Vec::new(),
            store,
            locked: false,
            initialized: true,
            pending: None,
            error: None,
            read_failed: false,
        }
    }
    pub(crate) fn selected_id(&self) -> PlayerId {
        self.selected.player()
    }
    pub(crate) fn is_locked(&self) -> bool {
        self.locked
    }
    pub(crate) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
    pub(crate) fn is_initializing(&self) -> bool {
        !self.initialized && self.is_pending()
    }
    pub(crate) fn is_ready(&self) -> bool {
        self.initialized && !self.is_pending() && self.error.is_none()
    }
    pub(crate) fn storage_error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub(crate) fn lock(&mut self) -> Result<String, String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        if !self.is_ready() {
            return Err("Preparing your saved Traveler ID. Please wait.".into());
        }
        self.locked = true;
        Ok(self.selected.encode())
    }
    pub(crate) fn unlock(&mut self) {
        self.locked = false;
    }
    pub(crate) fn use_server(&mut self, server: &str) -> Result<(), String> {
        let server = server.trim_end_matches('/');
        if server == self.server {
            return Ok(());
        }
        if self.locked {
            return Err("Disconnect before changing server.".into());
        }
        if self.is_pending() {
            return Err("Traveler profiles are being saved. Please wait.".into());
        }
        validate_server(server)?;
        let fallback = fresh_credential(self.selected_id());
        #[cfg(target_arch = "wasm32")]
        if matches!(self.store, Store::Browser) {
            self.begin_browser_selection(server.into(), SelectionRequest::Restore, fallback);
            return Ok(());
        }
        let old_server = std::mem::replace(&mut self.server, server.into());
        let old_credential = std::mem::replace(&mut self.selected, fallback);
        if let Err(error) = self.select_id(old_credential.player()) {
            self.server = old_server;
            self.selected = old_credential;
            return Err(error);
        }
        Ok(())
    }
    pub(crate) fn load(server: &str) -> Self {
        #[cfg(target_arch = "wasm32")]
        {
            let mut profile = Self::empty(server.trim_end_matches('/').into(), Store::Browser);
            profile.initialized = false;
            profile.begin_browser_selection(
                profile.server.clone(),
                SelectionRequest::Restore,
                profile.selected.clone(),
            );
            profile
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self::load_native(server, native_store(), argument("--player-id").as_deref())
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    fn load_native(server: &str, store: Store, requested: Option<&str>) -> Self {
        let mut profile = Self::empty(server.trim_end_matches('/').into(), store);
        let read = profile.read_store().and_then(|value| {
            if let Some(value) = value {
                profile.decode_store(&value)?;
            }
            Ok(())
        });
        if let Err(error) = read {
            profile.error = Some(error);
            profile.read_failed = true;
            return profile;
        }
        let selected = requested
            .map(str::parse::<PlayerId>)
            .transpose()
            .map_err(|error| error.to_string())
            .and_then(|id| profile.select_id(id.unwrap_or_else(|| profile.selected_id())));
        // A busy sidecar lock or a failed write is retryable. Only an actual
        // unreadable source above poisons loading, so UI Save can retry safely.
        if let Err(error) = selected {
            profile.error = Some(error);
        }
        profile
    }
    #[cfg(any(not(target_arch = "wasm32"), test))]
    fn decode_store(&mut self, data: &[u8]) -> Result<(), String> {
        let document = decode_document(data)?;
        if let Some(saved) = document.profiles.iter().find(|profile| {
            profile.server == self.server && profile.credential.player() == document.selected
        }) {
            self.selected = saved.credential.clone();
        }
        self.profiles = document.profiles;
        Ok(())
    }
    fn select_id(&mut self, id: PlayerId) -> Result<SelectionStatus, String> {
        if self.locked {
            return Err("Disconnect before changing your Traveler ID.".into());
        }
        if self.is_pending() {
            return Err("Traveler profiles are being saved. Please wait.".into());
        }
        if self.read_failed {
            return Err(self
                .error
                .clone()
                .unwrap_or_else(|| "Saved Traveler profiles are unavailable.".into()));
        }
        #[cfg(target_arch = "wasm32")]
        if matches!(self.store, Store::Browser) {
            self.begin_browser_selection(
                self.server.clone(),
                SelectionRequest::Select(id),
                self.selected.clone(),
            );
            return Ok(SelectionStatus::Pending);
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _guard = match &self.store {
            Store::File(path) => Some(
                lock_profile_file(path)
                    .map_err(|_| "Traveler profiles are busy or unavailable. Try again.")?,
            ),
            Store::Memory => None,
        };
        let stored = self.read_store()?;
        let document = match stored {
            Some(bytes) => decode_document(&bytes)?,
            None => SavedDocument {
                selected: self.selected_id(),
                profiles: self.profiles.clone(),
            },
        };
        let prepared = prepare_selection(
            &self.server,
            document,
            None,
            SelectionRequest::Select(id),
            &self.selected,
            fresh_credential,
        )?;
        self.write_store(&prepared.bytes)?;
        self.commit(prepared);
        Ok(SelectionStatus::Ready)
    }
    fn commit(&mut self, prepared: PreparedSelection) {
        self.server = prepared.server;
        self.selected = prepared.selected;
        self.profiles = prepared.profiles;
        self.initialized = true;
        self.error = None;
        self.read_failed = false;
    }
    fn read_store(&self) -> Result<Option<Vec<u8>>, String> {
        match &self.store {
            Store::Memory => Ok(None),
            #[cfg(not(target_arch = "wasm32"))]
            Store::File(path) => {
                use std::io::Read;
                let file = match std::fs::File::open(path) {
                    Ok(file) => file,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(_) => return Err("Cannot read the saved Traveler profiles.".into()),
                };
                let mut bytes = Vec::new();
                file.take((MAX_STORE_BYTES + 1) as u64)
                    .read_to_end(&mut bytes)
                    .map_err(|_| "Cannot read the saved Traveler profiles.")?;
                if bytes.len() > MAX_STORE_BYTES {
                    return Err("Saved Traveler profiles exceed the storage limit.".into());
                }
                Ok(Some(bytes))
            }
            #[cfg(target_arch = "wasm32")]
            Store::Browser => Err("Browser profiles require coordinated persistence.".into()),
        }
    }
    fn write_store(&self, _bytes: &[u8]) -> Result<(), String> {
        match &self.store {
            Store::Memory => Ok(()),
            #[cfg(not(target_arch = "wasm32"))]
            Store::File(path) => write_private_file(path, _bytes).map_err(|_| {
                "Cannot save your Traveler ID. The game will keep the current identity.".into()
            }),
            #[cfg(target_arch = "wasm32")]
            Store::Browser => Err("Browser profiles require coordinated persistence.".into()),
        }
    }
}

fn validate_server(server: &str) -> Result<(), String> {
    if server.is_empty() || server.len() > 512 {
        Err("Invalid server address.".into())
    } else {
        Ok(())
    }
}
fn decode_document(data: &[u8]) -> Result<SavedDocument, String> {
    if data.len() > MAX_STORE_BYTES {
        return Err("Saved Traveler profiles exceed the storage limit.".into());
    }
    let value: serde_json::Value = serde_json::from_slice(data).map_err(|_| "Saved Traveler profiles are unreadable. Keep the original file to recover its credentials.")?;
    if value.get("version").and_then(|value| value.as_u64()) != Some(1) {
        return Err("Unsupported saved Traveler profile version.".into());
    }
    let entries = value
        .get("profiles")
        .and_then(|value| value.as_array())
        .ok_or("Invalid saved Traveler profiles.")?;
    if entries.len() > MAX_PROFILES {
        return Err("Too many saved Traveler profiles.".into());
    }
    let mut profiles: Vec<SavedProfile> = Vec::with_capacity(entries.len());
    for entry in entries {
        let server = entry
            .get("server")
            .and_then(|value| value.as_str())
            .ok_or("Invalid saved server address.")?;
        validate_server(server)?;
        let credential = PlayerCredential::parse(
            entry
                .get("credential")
                .and_then(|value| value.as_str())
                .ok_or("Invalid saved Traveler credential.")?,
        )
        .map_err(|error| error.to_string())?;
        if profiles.iter().any(|profile| {
            profile.server == server && profile.credential.player() == credential.player()
        }) {
            return Err("Duplicate saved Traveler profile.".into());
        }
        profiles.push(SavedProfile {
            server: server.into(),
            credential,
        });
    }
    let selected = value
        .get("selected")
        .and_then(|value| value.as_str())
        .ok_or("Invalid selected Traveler ID.")?
        .parse::<PlayerId>()
        .map_err(|error| error.to_string())?;
    Ok(SavedDocument { selected, profiles })
}

/// Pure transaction planning. The browser calls this *inside* its exclusive Web
/// Lock; native callers hold the stable sidecar file lock. Generate credentials
/// only after the latest store lookup, and commit memory only after persistence.
fn prepare_selection(
    server: &str,
    mut document: SavedDocument,
    tab_selected: Option<&str>,
    request: SelectionRequest,
    fallback: &PlayerCredential,
    fresh: impl FnOnce(PlayerId) -> PlayerCredential,
) -> Result<PreparedSelection, String> {
    validate_server(server)?;
    let selected_id = match request {
        SelectionRequest::Select(id) => id,
        SelectionRequest::Restore => match tab_selected {
            Some(text) => text
                .parse::<PlayerId>()
                .map_err(|error| error.to_string())?,
            None if document.profiles.iter().any(|profile| {
                profile.server == server && profile.credential.player() == document.selected
            }) =>
            {
                document.selected
            }
            None => fallback.player(),
        },
    };
    let selected = document
        .profiles
        .iter()
        .find(|profile| profile.server == server && profile.credential.player() == selected_id)
        .map(|profile| profile.credential.clone())
        .unwrap_or_else(|| {
            if selected_id == fallback.player() {
                fallback.clone()
            } else {
                fresh(selected_id)
            }
        });
    if !document
        .profiles
        .iter()
        .any(|profile| profile.server == server && profile.credential.player() == selected_id)
    {
        if document.profiles.len() >= MAX_PROFILES {
            return Err("The saved Traveler profile limit has been reached.".into());
        }
        document.profiles.push(SavedProfile {
            server: server.into(),
            credential: selected.clone(),
        });
    }
    let value = serde_json::json!({ "version": 1, "selected": selected_id.to_string(), "profiles": document.profiles.iter().map(|profile| serde_json::json!({"server":profile.server, "credential":profile.credential.encode()})).collect::<Vec<_>>() });
    let bytes = serde_json::to_vec(&value).map_err(|_| "Could not encode Traveler profiles.")?;
    if bytes.len() > MAX_STORE_BYTES {
        return Err("Saved Traveler profiles exceed the storage limit.".into());
    }
    Ok(PreparedSelection {
        server: server.into(),
        selected,
        profiles: document.profiles,
        bytes,
    })
}

/// Apply asynchronous browser persistence before networking can use a credential.
/// A failed or canceled operation never publishes its staged credential in memory.
pub(crate) fn poll(world: &mut World) {
    let Some(mut profile) = world.get_resource_mut::<TravelerProfile>() else {
        return;
    };
    let result = profile
        .pending
        .as_ref()
        .and_then(|receiver| match receiver.lock() {
            Ok(receiver) => match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(
                    "Traveler profile save was interrupted. Try again.".into(),
                )),
            },
            Err(_) => Some(Err(
                "Traveler profile save was interrupted. Try again.".into()
            )),
        });
    if let Some(result) = result {
        profile.pending = None;
        match result {
            Ok(prepared) => profile.commit(prepared),
            Err(error) => profile.error = Some(error),
        }
    }
}
fn fresh_credential(id: PlayerId) -> PlayerCredential {
    loop {
        if let Ok(value) = PlayerCredential::new(id, renet_cross::generate_random_bytes()) {
            return value;
        }
    }
}
pub(crate) fn select(world: &mut World, text: &str) -> Result<SelectionStatus, String> {
    let id = text
        .parse::<PlayerId>()
        .map_err(|error| error.to_string())?;
    world.resource_mut::<TravelerProfile>().select_id(id)
}

#[cfg(target_arch = "wasm32")]
fn tab_selection_key(server: &str) -> String {
    use std::fmt::Write;
    let mut key = String::from("dreamwake.traveler-selection.v1:");
    for byte in server.bytes() {
        write!(&mut key, "{byte:02x}").unwrap();
    }
    key
}

#[cfg(target_arch = "wasm32")]
mod browser {
    use super::*;
    use std::{cell::RefCell, rc::Rc};
    use wasm_bindgen::{JsCast, JsValue, closure::Closure, prelude::wasm_bindgen};
    use wasm_bindgen_futures::{JsFuture, spawn_local};

    // JavaScript coordinates browser storage only. Credential validation,
    // lookup, generation, caps and complete serialization remain in Rust.
    #[wasm_bindgen(inline_js = r#"
export async function dreamwakeSaveTraveler(key, selectionKey, maximum, prepare) {
    if (!globalThis.navigator?.locks || !globalThis.AbortController) {
        throw 'This browser cannot safely save Traveler IDs across tabs.';
    }
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 5000);
    try {
        return await navigator.locks.request(key, {mode: 'exclusive', signal: controller.signal}, () => {
            let stored, selected;
            try {
                stored = localStorage.getItem(key);
                selected = sessionStorage.getItem(selectionKey);
            } catch (_) { throw 'Cannot read this browser\'s saved Traveler profiles.'; }
            if (stored !== null && stored.length > maximum) throw 'Saved Traveler profiles exceed the storage limit.';
            if (selected !== null && selected.length > 19) throw 'Invalid selected Traveler ID.';
            const update = prepare(stored, selected);
            if (!Array.isArray(update) || update.length !== 2 || typeof update[0] !== 'string' || typeof update[1] !== 'string' || update[0].length > maximum || update[1].length > 19) {
                throw 'Could not prepare saved Traveler profiles.';
            }
            try {
                localStorage.setItem(key, update[0]);
                sessionStorage.setItem(selectionKey, update[1]);
            } catch (_) { throw 'Cannot save your Traveler ID in this browser. Try again.'; }
            return true;
        });
    } catch (error) {
        if (error?.name === 'AbortError') throw 'Traveler profiles are busy. Try again.';
        if (typeof error === 'string') throw error;
        throw 'Cannot coordinate saved Traveler profiles in this browser.';
    } finally { clearTimeout(timeout); }
}
"#)]
    extern "C" {
        #[wasm_bindgen(js_name = dreamwakeSaveTraveler)]
        fn save_traveler(
            key: &str,
            selection_key: &str,
            maximum: usize,
            prepare: &js_sys::Function,
        ) -> js_sys::Promise;
    }
    impl TravelerProfile {
        pub(super) fn begin_browser_selection(
            &mut self,
            server: String,
            request: SelectionRequest,
            fallback: PlayerCredential,
        ) {
            let (sender, receiver) = mpsc::channel();
            self.pending = Some(Mutex::new(receiver));
            self.error = None;
            spawn_local(async move {
                let prepared = Rc::new(RefCell::new(None));
                let output = prepared.clone();
                let selection_key = tab_selection_key(&server);
                let callback = Closure::wrap(Box::new(
                    move |stored: JsValue, tab_selected: JsValue| -> Result<JsValue, JsValue> {
                        let stored = stored.as_string();
                        let tab_selected = tab_selected.as_string();
                        let document = match stored {
                            Some(stored) => decode_document(stored.as_bytes()),
                            None => Ok(SavedDocument {
                                selected: fallback.player(),
                                profiles: Vec::new(),
                            }),
                        }
                        .map_err(|error| JsValue::from_str(&error))?;
                        let selection = prepare_selection(
                            &server,
                            document,
                            tab_selected.as_deref(),
                            request,
                            &fallback,
                            fresh_credential,
                        )
                        .map_err(|error| JsValue::from_str(&error))?;
                        let update = js_sys::Array::new();
                        update.push(&JsValue::from_str(
                            std::str::from_utf8(&selection.bytes).map_err(|_| {
                                JsValue::from_str("Invalid Traveler profile encoding.")
                            })?,
                        ));
                        update.push(&JsValue::from_str(&selection.selected.player().to_string()));
                        *output.borrow_mut() = Some(selection);
                        Ok(update.into())
                    },
                )
                    as Box<dyn FnMut(JsValue, JsValue) -> Result<JsValue, JsValue>>);
                let result = JsFuture::from(save_traveler(
                    STORAGE_KEY,
                    &selection_key,
                    MAX_STORE_BYTES,
                    callback.as_ref().unchecked_ref(),
                ))
                .await;
                let result = match result {
                    Ok(_) => prepared
                        .borrow_mut()
                        .take()
                        .ok_or_else(|| "Traveler profile save did not complete. Try again.".into()),
                    Err(error) => Err(error.as_string().unwrap_or_else(|| {
                        "Traveler profile save did not complete. Try again.".into()
                    })),
                };
                let _ = sender.send(result);
                // callback stays alive until request resolves or aborts.
                drop(callback);
            });
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn argument(name: &str) -> Option<String> {
    let mut args = std::env::args();
    while let Some(arg) = args.next() {
        if arg == name {
            return args.next();
        }
    }
    None
}
#[cfg(not(target_arch = "wasm32"))]
fn native_store() -> Store {
    use std::path::PathBuf;
    if let Some(path) = argument("--profile-file") {
        return Store::File(path.into());
    }
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|p| PathBuf::from(p).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
    };
    Store::File(
        base.unwrap_or_else(|| PathBuf::from("."))
            .join("Dreamwake/travelers.json"),
    )
}
#[cfg(not(target_arch = "wasm32"))]
fn write_private_file(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent().filter(|value| !value.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let nonce: [u8; 8] = renet_cross::generate_random_bytes();
    let temporary = path.with_extension(format!("tmp-{:016x}", u64::from_le_bytes(nonce)));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    let result = file
        .write_all(bytes)
        .and_then(|_| file.sync_all())
        .and_then(|_| std::fs::rename(&temporary, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}
#[cfg(not(target_arch = "wasm32"))]
fn lock_profile_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent().filter(|value| !value.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut lock_path = path.as_os_str().to_owned();
    lock_path.push(".lock");
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(std::path::PathBuf::from(lock_path))?;
    file.try_lock().map_err(std::io::Error::other)?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn switching_numeric_profiles_reuses_secret_and_locked_identity_cannot_change() {
        let mut profile = TravelerProfile::default();
        profile.select_id(PlayerId::new(101).unwrap()).unwrap();
        let first = profile.lock().unwrap();
        assert!(profile.select_id(PlayerId::new(202).unwrap()).is_err());
        profile.unlock();
        profile.select_id(PlayerId::new(202).unwrap()).unwrap();
        assert_ne!(profile.selected.encode(), first);
        profile.select_id(PlayerId::new(101).unwrap()).unwrap();
        assert_eq!(profile.lock().unwrap(), first);
    }
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn file_profile_roundtrip_preserves_large_integer_and_secret() {
        let nonce: [u8; 8] = renet_cross::generate_random_bytes();
        let path = std::env::temp_dir().join(format!(
            "dreamwake-profile-{:016x}.json",
            u64::from_le_bytes(nonce)
        ));
        let mut profile = TravelerProfile::empty("local".into(), Store::File(path.clone()));
        profile
            .select_id(PlayerId::new(PlayerId::MAX).unwrap())
            .unwrap();
        let credential = profile.selected.encode();
        let mut restored = TravelerProfile::empty("local".into(), Store::File(path.clone()));
        restored
            .decode_store(&restored.read_store().unwrap().unwrap())
            .unwrap();
        assert_eq!(restored.selected.encode(), credential);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_file(path).unwrap();
    }
    fn credential(id: u64, byte: u8) -> PlayerCredential {
        PlayerCredential::new(PlayerId::new(id).unwrap(), [byte; 32]).unwrap()
    }
    fn empty_document() -> SavedDocument {
        SavedDocument {
            selected: PlayerId::new(101).unwrap(),
            profiles: Vec::new(),
        }
    }
    #[test]
    fn serialized_tab_transactions_preserve_both_ids_and_reuse_same_id_secret() {
        let fallback = credential(101, 1);
        let first = prepare_selection(
            "server",
            empty_document(),
            None,
            SelectionRequest::Select(fallback.player()),
            &fallback,
            |_| panic!("reuse selected fallback"),
        )
        .unwrap();
        // Tab B reads after acquiring the same lock, including A's new record.
        let second = prepare_selection(
            "server",
            decode_document(&first.bytes).unwrap(),
            None,
            SelectionRequest::Select(PlayerId::new(202).unwrap()),
            &credential(999, 9),
            |id| credential(id.get(), 2),
        )
        .unwrap();
        assert_eq!(second.profiles.len(), 2);
        let same_id = prepare_selection(
            "server",
            decode_document(&second.bytes).unwrap(),
            None,
            SelectionRequest::Select(fallback.player()),
            &credential(888, 8),
            |_| panic!("an existing ID must not mint a secret"),
        )
        .unwrap();
        assert_eq!(same_id.selected.encode(), first.selected.encode());
        assert_eq!(same_id.profiles.len(), 2);
    }
    #[test]
    fn tab_restore_uses_its_own_server_selection_instead_of_other_tabs_last_id() {
        let first = prepare_selection(
            "server-a",
            empty_document(),
            None,
            SelectionRequest::Select(PlayerId::new(101).unwrap()),
            &credential(101, 1),
            |_| unreachable!(),
        )
        .unwrap();
        let second = prepare_selection(
            "server-a",
            decode_document(&first.bytes).unwrap(),
            None,
            SelectionRequest::Select(PlayerId::new(202).unwrap()),
            &credential(202, 2),
            |_| unreachable!(),
        )
        .unwrap();
        let restored_a = prepare_selection(
            "server-a",
            decode_document(&second.bytes).unwrap(),
            Some("101"),
            SelectionRequest::Restore,
            &credential(999, 9),
            |_| panic!("retain saved secret"),
        )
        .unwrap();
        assert_eq!(restored_a.selected.encode(), first.selected.encode());
        let other_server = prepare_selection(
            "server-b",
            decode_document(&restored_a.bytes).unwrap(),
            Some("101"),
            SelectionRequest::Restore,
            &credential(101, 3),
            |_| unreachable!(),
        )
        .unwrap();
        assert_eq!(other_server.selected.encode(), credential(101, 3).encode());
        assert_ne!(other_server.selected.encode(), restored_a.selected.encode());
    }
    #[test]
    fn pending_result_cannot_supply_a_credential_until_persistence_completes() {
        let mut world = World::new();
        let mut profile = TravelerProfile::default();
        profile.select_id(PlayerId::new(101).unwrap()).unwrap();
        let (sender, receiver) = mpsc::channel();
        profile.pending = Some(Mutex::new(receiver));
        world.insert_resource(profile);
        assert!(!world.resource::<TravelerProfile>().is_ready());
        assert!(world.resource_mut::<TravelerProfile>().lock().is_err());
        poll(&mut world);
        assert!(world.resource::<TravelerProfile>().is_pending());
        let prepared = prepare_selection(
            "server",
            empty_document(),
            None,
            SelectionRequest::Select(PlayerId::new(202).unwrap()),
            &credential(202, 2),
            |_| unreachable!(),
        )
        .unwrap();
        sender.send(Ok(prepared)).ok().unwrap();
        assert_eq!(world.resource::<TravelerProfile>().selected_id().get(), 101);
        poll(&mut world);
        assert_eq!(world.resource::<TravelerProfile>().selected_id().get(), 202);
        assert!(world.resource::<TravelerProfile>().is_ready());
        assert!(world.resource_mut::<TravelerProfile>().lock().is_ok());
    }
    #[test]
    fn failed_pending_write_keeps_the_previous_identity_and_allows_explicit_retry() {
        let mut world = World::new();
        let mut profile = TravelerProfile::default();
        profile.select_id(PlayerId::new(101).unwrap()).unwrap();
        let original = profile.selected.encode();
        let (sender, receiver) = mpsc::channel();
        profile.pending = Some(Mutex::new(receiver));
        world.insert_resource(profile);
        assert!(
            sender
                .send(Err("Traveler profiles are busy. Try again.".into()))
                .is_ok()
        );
        poll(&mut world);
        assert_eq!(
            world.resource::<TravelerProfile>().selected.encode(),
            original
        );
        assert!(!world.resource::<TravelerProfile>().is_pending());
        assert!(!world.resource::<TravelerProfile>().is_ready());
        select(&mut world, "101").unwrap();
        assert!(world.resource::<TravelerProfile>().is_ready());
        assert_eq!(
            world.resource::<TravelerProfile>().selected.encode(),
            original
        );
    }
    #[test]
    fn invalid_or_oversized_store_never_produces_a_replacement_write() {
        assert!(decode_document(&vec![b' '; MAX_STORE_BYTES + 1]).is_err());
        assert!(decode_document(br#"{"version":1,"selected":"101","profiles":[{"server":"server","credential":"secret"}]}"#).is_err());
        assert!(
            prepare_selection(
                "server",
                empty_document(),
                Some("001"),
                SelectionRequest::Restore,
                &credential(101, 1),
                |_| unreachable!()
            )
            .is_err()
        );
    }
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_startup_busy_lock_is_retryable_after_other_process_releases_it() {
        let nonce: [u8; 8] = renet_cross::generate_random_bytes();
        let directory = std::env::temp_dir().join(format!(
            "dreamwake-profile-lock-{:016x}",
            u64::from_le_bytes(nonce)
        ));
        let path = directory.join("profiles.json");
        let guard = lock_profile_file(&path).unwrap();
        let mut profile =
            TravelerProfile::load_native("server", Store::File(path.clone()), Some("101"));
        assert!(profile.storage_error().unwrap().contains("busy"));
        assert!(!profile.read_failed);
        assert!(profile.lock().is_err());
        drop(guard);
        assert_eq!(
            profile.select_id(PlayerId::new(101).unwrap()).unwrap(),
            SelectionStatus::Ready
        );
        assert!(profile.is_ready());
        assert_eq!(profile.selected_id().get(), 101);
        let saved = profile.selected.encode();
        let restored = TravelerProfile::load_native("server", Store::File(path), Some("101"));
        assert_eq!(restored.selected.encode(), saved);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
