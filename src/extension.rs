use crate::{api::Api, config::Config, model::*};
use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream as StdTcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc as StdArc, Mutex as StdMutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::{Mutex, RwLock},
    time::timeout,
};

pub const EXTENSION_ID: &str = "iepklfmnjidigljfaegfjlbeghpjejka";
/// Older unpacked builds that still speak the current bridge protocol.
/// Reload is optional while one of these is running.
const COMPATIBLE_EXTENSION_VERSIONS: &[&str] = &["0.3.2", "0.3.3", "0.3.4", "0.3.5", "0.3.6"];

fn extension_version_supported(running: &str) -> bool {
    running == env!("CARGO_PKG_VERSION") || COMPATIBLE_EXTENSION_VERSIONS.contains(&running)
}
const HOST_NAME: &str = "com.xtui.bridge";
const BRIDGE_ADDRESS: &str = "127.0.0.1:17471";
const MAX_MESSAGE_BYTES: usize = 512 * 1024;
const CALL_TIMEOUT: Duration = Duration::from_secs(35);

const EXTENSION_FILES: &[(&str, &str)] = &[
    ("manifest.json", include_str!("../extension/manifest.json")),
    ("background.js", include_str!("../extension/background.js")),
    ("timeline.js", include_str!("../extension/timeline.js")),
    (
        "interceptor.js",
        include_str!("../extension/interceptor.js"),
    ),
    ("content.js", include_str!("../extension/content.js")),
    ("popup.html", include_str!("../extension/popup.html")),
    ("popup.css", include_str!("../extension/popup.css")),
    ("popup.js", include_str!("../extension/popup.js")),
    ("PRIVACY.md", include_str!("../extension/PRIVACY.md")),
    (
        "STORE_LISTING.md",
        include_str!("../extension/STORE_LISTING.md"),
    ),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserTarget {
    Edge,
    Chrome,
}

impl BrowserTarget {
    pub fn parse(value: Option<&str>) -> Result<Self> {
        match value.unwrap_or("--edge") {
            "edge" | "--edge" => Ok(Self::Edge),
            "chrome" | "--chrome" => Ok(Self::Chrome),
            other => bail!("unknown browser `{other}`; use --edge or --chrome"),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Edge => "Microsoft Edge",
            Self::Chrome => "Google Chrome",
        }
    }
}

pub fn extension_root() -> Result<PathBuf> {
    Ok(Config::path()?
        .parent()
        .context("XTUI config path had no parent")?
        .join("extension"))
}

pub fn prepare_extension() -> Result<PathBuf> {
    let root = extension_root()?;
    fs::create_dir_all(&root).with_context(|| format!("could not create {}", root.display()))?;
    for (relative, contents) in EXTENSION_FILES {
        let path = root.join(relative);
        fs::write(&path, contents)
            .with_context(|| format!("could not write {}", path.display()))?;
    }
    Ok(root)
}

pub fn install_extension(target: BrowserTarget) -> Result<PathBuf> {
    let root = prepare_extension()?;
    let executable = std::env::current_exe().context("could not locate the XTUI executable")?;
    register_native_host(target, &executable)?;
    let mut config = Config::load()?;
    config.source = Some("extension".into());
    config.save()?;
    println!(
        "XTUI prepared the {} extension at {}",
        target.label(),
        root.display()
    );
    println!(
        "Native host registered for extension {EXTENSION_ID}. Load this folder once from the browser's Extensions page."
    );
    Ok(root)
}

fn host_manifest_path(target: BrowserTarget) -> Result<PathBuf> {
    let suffix = match target {
        BrowserTarget::Edge => "edge",
        BrowserTarget::Chrome => "chrome",
    };
    Ok(extension_root()?.join(format!("native-host-{suffix}.json")))
}

fn write_host_manifest(target: BrowserTarget, executable: &Path) -> Result<PathBuf> {
    let path = host_manifest_path(target)?;
    let manifest = json!({
        "name": HOST_NAME,
        "description": "XTUI local browser bridge",
        "path": executable,
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{EXTENSION_ID}/")]
    });
    fs::write(&path, serde_json::to_vec_pretty(&manifest)?)
        .with_context(|| format!("could not write {}", path.display()))?;
    Ok(path)
}

#[cfg(windows)]
fn register_native_host(target: BrowserTarget, executable: &Path) -> Result<()> {
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};
    let manifest = write_host_manifest(target, executable)?;
    let browser = match target {
        BrowserTarget::Edge => "Microsoft\\Edge",
        BrowserTarget::Chrome => "Google\\Chrome",
    };
    let key_path = format!("Software\\{browser}\\NativeMessagingHosts\\{HOST_NAME}");
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu
        .create_subkey(&key_path)
        .with_context(|| format!("could not create HKCU\\{key_path}"))?;
    key.set_value("", &manifest.to_string_lossy().as_ref())
        .with_context(|| format!("could not register HKCU\\{key_path}"))?;
    Ok(())
}

#[cfg(not(windows))]
fn register_native_host(target: BrowserTarget, executable: &Path) -> Result<()> {
    let manifest = write_host_manifest(target, executable)?;
    let home = dirs::home_dir().context("could not locate the home directory")?;
    let directory = match (std::env::consts::OS, target) {
        ("macos", BrowserTarget::Edge) => {
            home.join("Library/Application Support/Microsoft Edge/NativeMessagingHosts")
        }
        ("macos", BrowserTarget::Chrome) => {
            home.join("Library/Application Support/Google/Chrome/NativeMessagingHosts")
        }
        (_, BrowserTarget::Edge) => home.join(".config/microsoft-edge/NativeMessagingHosts"),
        (_, BrowserTarget::Chrome) => home.join(".config/google-chrome/NativeMessagingHosts"),
    };
    fs::create_dir_all(&directory)?;
    fs::copy(manifest, directory.join(format!("{HOST_NAME}.json")))?;
    Ok(())
}

pub fn installation_status(target: BrowserTarget) -> Result<Value> {
    let root = extension_root()?;
    let manifest = host_manifest_path(target)?;
    Ok(json!({
        "browser": target.label(),
        "extension_id": EXTENSION_ID,
        "extension_path": root,
        "extension_prepared": root.join("manifest.json").is_file(),
        "native_host_manifest": manifest,
        "native_host_registered": native_host_registered(target, &manifest),
        "bridge_listening": StdTcpStream::connect_timeout(&BRIDGE_ADDRESS.parse()?, Duration::from_millis(250)).is_ok()
    }))
}

#[cfg(windows)]
fn native_host_registered(target: BrowserTarget, manifest: &Path) -> bool {
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};
    let browser = match target {
        BrowserTarget::Edge => "Microsoft\\Edge",
        BrowserTarget::Chrome => "Google\\Chrome",
    };
    let path = format!("Software\\{browser}\\NativeMessagingHosts\\{HOST_NAME}");
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(path)
        .and_then(|key| key.get_value::<String, _>(""))
        .is_ok_and(|value| Path::new(&value) == manifest)
}

#[cfg(not(windows))]
fn native_host_registered(_target: BrowserTarget, manifest: &Path) -> bool {
    manifest.is_file()
}

pub fn run_native_host() -> Result<()> {
    validate_native_origin()?;
    let listener = TcpListener::bind(BRIDGE_ADDRESS)
        .with_context(|| format!("could not bind XTUI native bridge at {BRIDGE_ADDRESS}"))?;
    let active = StdArc::new(StdMutex::new(None::<NativeClient>));
    let generation = StdArc::new(AtomicU64::new(1));
    let stdout_gate = StdArc::new(StdMutex::new(()));
    let accept_active = active.clone();
    let accept_generation = generation.clone();
    let accept_stdout = stdout_gate.clone();
    std::thread::spawn(move || {
        for incoming in listener.incoming() {
            let Ok(client) = incoming else { continue };
            let _ = client.set_nodelay(true);
            let Ok(mut reader) = client.try_clone() else {
                continue;
            };
            let writer = client;
            let reader_active = accept_active.clone();
            let reader_generation = accept_generation.clone();
            let reader_stdout = accept_stdout.clone();
            std::thread::spawn(move || {
                // A connect with no framed request is a health probe or a
                // half-open socket. Promoting it would shut down the live TUI.
                let Ok(first) = read_frame(&mut reader, MAX_MESSAGE_BYTES) else {
                    return;
                };
                let id = reader_generation.fetch_add(1, Ordering::Relaxed);
                replace_native_client(&reader_active, id, writer);
                if !forward_to_native(&first, id, &reader_active, &reader_stdout) {
                    clear_native_client(&reader_active, id);
                    return;
                }
                relay_client_to_native(reader, id, &reader_active, &reader_stdout);
            });
        }
    });

    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    while let Ok(payload) = read_frame(&mut input, MAX_MESSAGE_BYTES) {
        let target = active.lock().ok().and_then(|slot| {
            slot.as_ref().and_then(|client| {
                client
                    .stream
                    .try_clone()
                    .ok()
                    .map(|stream| (client.generation, stream))
            })
        });
        if let Some((generation, mut client)) = target
            && write_frame(&mut client, &payload).is_err()
            && let Ok(mut slot) = active.lock()
            && slot
                .as_ref()
                .is_some_and(|client| client.generation == generation)
        {
            *slot = None;
        }
    }
    Ok(())
}

struct NativeClient {
    generation: u64,
    stream: StdTcpStream,
}

fn replace_native_client(
    active: &StdArc<StdMutex<Option<NativeClient>>>,
    generation: u64,
    stream: StdTcpStream,
) {
    if let Ok(mut slot) = active.lock()
        && let Some(previous) = slot.replace(NativeClient { generation, stream })
    {
        let _ = previous.stream.shutdown(std::net::Shutdown::Both);
    }
}

fn validate_native_origin() -> Result<()> {
    let expected = format!("chrome-extension://{EXTENSION_ID}/");
    let origin = std::env::args().nth(1);
    if origin.as_deref() != Some(expected.as_str()) {
        bail!("native host rejected an unexpected extension origin")
    }
    Ok(())
}

fn forward_to_native(
    payload: &[u8],
    generation: u64,
    active: &StdArc<StdMutex<Option<NativeClient>>>,
    stdout_gate: &StdArc<StdMutex<()>>,
) -> bool {
    let is_current = active
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().map(|client| client.generation == generation))
        .unwrap_or(false);
    if !is_current {
        return false;
    }
    let Ok(_gate) = stdout_gate.lock() else {
        return false;
    };
    let stdout = std::io::stdout();
    write_frame(&mut stdout.lock(), payload).is_ok()
}

fn clear_native_client(active: &StdArc<StdMutex<Option<NativeClient>>>, generation: u64) {
    if let Ok(mut slot) = active.lock()
        && slot
            .as_ref()
            .is_some_and(|client| client.generation == generation)
    {
        *slot = None;
    }
}

fn relay_client_to_native(
    mut socket: StdTcpStream,
    generation: u64,
    active: &StdArc<StdMutex<Option<NativeClient>>>,
    stdout_gate: &StdArc<StdMutex<()>>,
) {
    while let Ok(payload) = read_frame(&mut socket, MAX_MESSAGE_BYTES) {
        if !forward_to_native(&payload, generation, active, stdout_gate) {
            break;
        }
    }
    clear_native_client(active, generation);
}

fn read_frame(reader: &mut impl Read, maximum: usize) -> Result<Vec<u8>> {
    let mut length = [0u8; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    if length > maximum {
        bail!("native message exceeded {maximum} bytes")
    }
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload)?;
    Ok(payload)
}

fn write_frame(writer: &mut impl Write, payload: &[u8]) -> Result<()> {
    if payload.len() > MAX_MESSAGE_BYTES {
        bail!("native message exceeded {MAX_MESSAGE_BYTES} bytes")
    }
    writer.write_all(&(payload.len() as u32).to_le_bytes())?;
    writer.write_all(payload)?;
    writer.flush()?;
    Ok(())
}

pub struct ExtensionApi {
    _lock: BridgeLock,
    stream: Mutex<TcpStream>,
    request_id: AtomicU64,
    me: RwLock<Option<User>>,
    authors: RwLock<HashMap<String, (String, u64)>>,
    feed: RwLock<FeedKind>,
}

/// Keeps a process-wide exclusive file open so a second TUI cannot attach to
/// the same native-host socket and abort the first session.
struct BridgeLock {
    _file: fs::File,
}

fn acquire_bridge_lock() -> Result<BridgeLock> {
    let path = Config::path()?
        .parent()
        .context("XTUI config path had no parent")?
        .join("bridge.lock");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let mut file = open_exclusive_lock(&path).with_context(
        || "another XTUI is already using the browser bridge; quit that terminal session first",
    )?;
    file.set_len(0)?;
    writeln!(file, "{}", std::process::id())?;
    file.flush()?;
    Ok(BridgeLock { _file: file })
}

#[cfg(windows)]
fn open_exclusive_lock(path: &Path) -> Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    Ok(fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .share_mode(0)
        .open(path)?)
}

#[cfg(not(windows))]
fn open_exclusive_lock(path: &Path) -> Result<fs::File> {
    if let Ok(existing) = fs::read_to_string(path)
        && let Ok(pid) = existing.trim().parse::<i32>()
        && process_is_alive(pid)
    {
        bail!("bridge lock is held by pid {pid}");
    }
    Ok(fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(true)
        .open(path)?)
}

#[cfg(not(windows))]
fn process_is_alive(pid: i32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

fn reregister_native_host() {
    if let Ok(executable) = std::env::current_exe() {
        let _ = register_native_host(BrowserTarget::Edge, &executable);
        let _ = register_native_host(BrowserTarget::Chrome, &executable);
    }
}

impl ExtensionApi {
    pub async fn connect() -> Result<Self> {
        let lock = acquire_bridge_lock()?;
        reregister_native_host();
        let stream = connect_stream().await?;
        let api = Self {
            _lock: lock,
            stream: Mutex::new(stream),
            request_id: AtomicU64::new(1),
            me: RwLock::new(None),
            authors: RwLock::new(HashMap::new()),
            feed: RwLock::new(FeedKind::Following),
        };
        let status = timeout(
            Duration::from_secs(5),
            api.call::<Value>(json!({ "op": "status" })),
        )
        .await
        .map_err(|_| anyhow!("the XTUI native bridge accepted a socket but did not respond"))??;
        let running_version = status
            .get("extension_version")
            .and_then(Value::as_str)
            .unwrap_or("older than 0.2.4");
        if !extension_version_supported(running_version) {
            bail!(
                "Edge is running XTUI extension {running_version}, but the CLI is {}; open edge://extensions and click Reload on XTUI",
                env!("CARGO_PKG_VERSION")
            )
        }
        Ok(api)
    }

    async fn call<T: DeserializeOwned>(&self, mut request: Value) -> Result<T> {
        let id = self.request_id.fetch_add(1, Ordering::Relaxed);
        request["id"] = json!(id);
        let payload = serde_json::to_vec(&request)?;
        if payload.len() > MAX_MESSAGE_BYTES {
            bail!("extension request exceeded {MAX_MESSAGE_BYTES} bytes")
        }
        let mut stream = self.stream.lock().await;
        let mut first_error = None;
        for attempt in 0..3 {
            match exchange(&mut stream, &payload, id).await {
                Ok(response) => return Ok(response),
                Err(error) => {
                    let retryable = bridge_error_is_retryable(&error);
                    if first_error.is_none() {
                        first_error = Some(error.to_string());
                    }
                    if !retryable || attempt + 1 == 3 {
                        return Err(error).with_context(|| {
                            if attempt == 0 {
                                "browser bridge request failed".to_owned()
                            } else {
                                format!(
                                    "browser bridge request failed after reconnect (first failure: {})",
                                    first_error.as_deref().unwrap_or("none")
                                )
                            }
                        });
                    }
                    tokio::time::sleep(Duration::from_millis(120 * (attempt as u64 + 1))).await;
                    *stream = connect_stream().await?;
                }
            }
        }
        unreachable!("bridge retry loop always returns")
    }

    async fn page(&self, op: &str, cursor: Option<&str>, fields: Value) -> Result<Page<Post>> {
        let mut request = json!({ "op": op, "cursor": cursor });
        if let (Some(target), Some(source)) = (request.as_object_mut(), fields.as_object()) {
            target.extend(source.clone());
        }
        let raw: RawPage = self.call(request).await?;
        let posts: Vec<_> = raw
            .items
            .into_iter()
            .filter_map(ScrapedPost::into_post)
            .collect();
        self.remember(&posts).await;
        Ok(Page {
            items: posts,
            next_token: raw.next_token,
        })
    }

    async fn remember(&self, posts: &[Post]) {
        let mut authors = self.authors.write().await;
        if authors.len() > 2048 {
            authors.clear();
        }
        for post in posts {
            authors.insert(
                post.id.clone(),
                (post.author.username.clone(), post.metrics.reply_count),
            );
        }
    }

    pub async fn shutdown(&self) {
        let _: Result<Value> = self.call(json!({ "op": "shutdown" })).await;
    }
}

async fn connect_stream() -> Result<TcpStream> {
    let mut last = None;
    for attempt in 0..4 {
        match timeout(Duration::from_secs(3), TcpStream::connect(BRIDGE_ADDRESS)).await {
            Ok(Ok(stream)) => {
                stream.set_nodelay(true)?;
                return Ok(stream);
            }
            Ok(Err(error)) => last = Some(anyhow!(error)),
            Err(_) => {
                last = Some(anyhow!(
                    "timed out connecting to the XTUI browser extension"
                ));
            }
        }
        tokio::time::sleep(Duration::from_millis(150 * (attempt as u64 + 1))).await;
    }
    Err(last
        .unwrap_or_else(|| {
            anyhow!(
                "the XTUI browser extension is not connected; load it and keep the browser running"
            )
        })
        .context(
            "the XTUI browser extension is not connected; load it and keep the browser running",
        ))
}

fn bridge_error_is_retryable(error: &anyhow::Error) -> bool {
    let text = error.to_string().to_ascii_lowercase();
    text.contains("aborted")
        || text.contains("connection reset")
        || text.contains("broken pipe")
        || text.contains("os error 10053")
        || text.contains("os error 10054")
        || text.contains("os error 104")
        || text.contains("forcibly closed")
}

async fn exchange<T: DeserializeOwned>(
    stream: &mut TcpStream,
    payload: &[u8],
    id: u64,
) -> Result<T> {
    timeout(CALL_TIMEOUT, async {
        stream
            .write_all(&(payload.len() as u32).to_le_bytes())
            .await?;
        stream.write_all(payload).await?;
        stream.flush().await?;
        let mut length = [0u8; 4];
        stream.read_exact(&mut length).await?;
        let length = u32::from_le_bytes(length) as usize;
        if length > MAX_MESSAGE_BYTES {
            bail!("extension response exceeded {MAX_MESSAGE_BYTES} bytes")
        }
        let mut response = vec![0; length];
        stream.read_exact(&mut response).await?;
        let envelope: ExtensionResponse<T> =
            serde_json::from_slice(&response).context("extension returned invalid JSON")?;
        if envelope.id != id {
            bail!("extension response id did not match request")
        }
        if !envelope.ok {
            bail!(
                "{}",
                envelope
                    .error
                    .unwrap_or_else(|| "extension request failed".into())
            )
        }
        envelope.result.context("extension response had no result")
    })
    .await
    .map_err(|_| anyhow!("the browser extension did not respond within 35 seconds"))?
}

#[derive(Deserialize)]
struct ExtensionResponse<T> {
    id: u64,
    ok: bool,
    result: Option<T>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct RawPage {
    items: Vec<ScrapedPost>,
    next_token: Option<String>,
}

#[derive(Deserialize)]
struct ScrapedPost {
    id: String,
    text: String,
    name: String,
    username: String,
    #[serde(default)]
    verified: bool,
    created_at: Option<String>,
    #[serde(default)]
    replies: u64,
    #[serde(default)]
    reposts: u64,
    #[serde(default)]
    likes: u64,
    #[serde(default)]
    views: u64,
    #[serde(default)]
    media: Vec<ScrapedMedia>,
    quoted: Option<Box<ScrapedPost>>,
}

#[derive(Deserialize)]
struct ScrapedMedia {
    kind: String,
    url: String,
    alt: Option<String>,
}

impl ScrapedPost {
    fn into_post(self) -> Option<Post> {
        if self.id.is_empty() || self.username.is_empty() {
            return None;
        }
        let created_at = self
            .created_at
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|date| date.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);
        let author = User {
            id: self.username.clone(),
            name: self.name,
            username: self.username,
            verified: self.verified,
            ..Default::default()
        };
        let media = self
            .media
            .into_iter()
            .enumerate()
            .map(|(index, media)| Media {
                key: format!("{}-{index}", self.id),
                kind: if media.kind == "video" {
                    MediaKind::Video
                } else {
                    MediaKind::Photo
                },
                url: Some(media.url),
                preview_url: None,
                alt_text: media.alt,
                variants: vec![],
            })
            .collect();
        Some(Post {
            id: self.id.clone(),
            text: self.text,
            author,
            created_at,
            conversation_id: self.id,
            in_reply_to_user_id: None,
            metrics: PostMetrics {
                reply_count: self.replies,
                retweet_count: self.reposts,
                like_count: self.likes,
                impression_count: Some(self.views),
                ..Default::default()
            },
            media,
            quoted: self.quoted.and_then(|post| post.into_post()).map(Box::new),
            reposted: None,
            language: None,
        })
    }
}

#[async_trait]
impl Api for ExtensionApi {
    async fn me(&self) -> Result<User> {
        if let Some(user) = self.me.read().await.clone() {
            return Ok(user);
        }
        let user: User = self.call(json!({ "op": "me" })).await?;
        if user.username.is_empty() {
            bail!("X is not signed in in this browser profile")
        }
        *self.me.write().await = Some(user.clone());
        Ok(user)
    }

    async fn set_feed(&self, feed: FeedKind) {
        *self.feed.write().await = feed;
    }

    async fn release_secondary(&self) {
        let _: Result<Value> = self.call(json!({ "op": "release_secondary" })).await;
    }

    async fn home(&self, next: Option<&str>) -> Result<Page<Post>> {
        let feed = match *self.feed.read().await {
            FeedKind::Following => "following",
            FeedKind::ForYou => "for_you",
        };
        self.page("home", next, json!({ "feed": feed })).await
    }

    async fn search(&self, query: &str, next: Option<&str>) -> Result<Page<Post>> {
        self.page("search", next, json!({ "query": query })).await
    }

    async fn thread(&self, conversation_id: &str) -> Result<Vec<Post>> {
        let route = self.authors.read().await.get(conversation_id).cloned();
        let (author, reply_count) = route
            .map(|(author, reply_count)| (Some(author), reply_count))
            .unwrap_or((None, 0));
        let raw: Vec<ScrapedPost> = self
            .call(json!({
                "op": "thread",
                "conversation_id": conversation_id,
                "author": author,
                "reply_count": reply_count
            }))
            .await?;
        let posts: Vec<_> = raw.into_iter().filter_map(ScrapedPost::into_post).collect();
        self.remember(&posts).await;
        Ok(posts)
    }

    async fn bookmarks(&self, next: Option<&str>) -> Result<Page<Post>> {
        self.page("bookmarks", next, json!({})).await
    }

    async fn likes(&self, user_id: &str, next: Option<&str>) -> Result<Page<Post>> {
        self.page("likes", next, json!({ "user_id": user_id }))
            .await
    }

    async fn mentions(&self, next: Option<&str>) -> Result<Page<Post>> {
        self.page("mentions", next, json!({})).await
    }

    async fn user_by_username(&self, username: &str) -> Result<User> {
        self.call(json!({ "op": "user", "user_id": username.trim_start_matches('@') }))
            .await
    }

    async fn user_posts(&self, user_id: &str, next: Option<&str>) -> Result<Page<Post>> {
        self.page("user_posts", next, json!({ "user_id": user_id }))
            .await
    }

    async fn lists(&self) -> Result<Vec<XList>> {
        let me = self.me().await?;
        self.call(json!({ "op": "lists", "user_id": me.username }))
            .await
    }

    async fn list_posts(&self, list_id: &str, next: Option<&str>) -> Result<Page<Post>> {
        self.page("list_posts", next, json!({ "list_id": list_id }))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_and_previous_extension_builds_are_accepted() {
        assert!(extension_version_supported(env!("CARGO_PKG_VERSION")));
        assert!(extension_version_supported("0.3.2"));
        assert!(extension_version_supported("0.3.3"));
        assert!(extension_version_supported("0.3.4"));
        assert!(extension_version_supported("0.3.5"));
        assert!(extension_version_supported("0.3.6"));
        assert!(!extension_version_supported("0.2.4"));
    }

    #[test]
    fn dropped_sockets_are_retried_but_application_errors_are_not() {
        assert!(bridge_error_is_retryable(&anyhow::anyhow!(
            "An established connection was aborted by the software in your host machine"
        )));
        assert!(bridge_error_is_retryable(&anyhow::anyhow!(
            "connection reset by peer"
        )));
        assert!(!bridge_error_is_retryable(&anyhow::anyhow!(
            "unsupported operation: home"
        )));
    }

    #[test]
    fn embedded_manifest_has_stable_extension_identity() {
        let manifest: Value =
            serde_json::from_str(include_str!("../extension/manifest.json")).unwrap();
        assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(EXTENSION_ID.len(), 32);
        assert_eq!(manifest["host_permissions"], json!(["https://x.com/*"]));
        assert!(
            EXTENSION_FILES
                .iter()
                .any(|(path, _)| *path == "interceptor.js"),
            "the installed extension must include the main-world timeline interceptor"
        );
    }

    #[test]
    fn framing_rejects_oversized_messages() {
        let length = ((MAX_MESSAGE_BYTES + 1) as u32).to_le_bytes();
        let error = read_frame(&mut length.as_slice(), MAX_MESSAGE_BYTES).unwrap_err();
        assert!(error.to_string().contains("exceeded"));
    }

    #[test]
    fn a_connect_without_a_request_does_not_displace_the_live_client() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let first_client = StdTcpStream::connect(address).unwrap();
        let (first_server, _) = listener.accept().unwrap();
        let active = StdArc::new(StdMutex::new(None));
        replace_native_client(&active, 7, first_server);

        let probe = StdTcpStream::connect(address).unwrap();
        drop(probe);
        assert_eq!(
            active
                .lock()
                .unwrap()
                .as_ref()
                .map(|client| client.generation),
            Some(7),
            "an empty connect must not take the live session"
        );
        drop(first_client);
    }

    #[test]
    fn a_new_tui_connection_replaces_and_closes_the_previous_one() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let mut first_client = StdTcpStream::connect(address).unwrap();
        let (first_server, _) = listener.accept().unwrap();
        let active = StdArc::new(StdMutex::new(None));
        replace_native_client(&active, 1, first_server);

        let _second_client = StdTcpStream::connect(address).unwrap();
        let (second_server, _) = listener.accept().unwrap();
        replace_native_client(&active, 2, second_server);

        assert_eq!(
            active
                .lock()
                .unwrap()
                .as_ref()
                .map(|client| client.generation),
            Some(2)
        );
        first_client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut byte = [0u8; 1];
        assert_eq!(
            first_client.read(&mut byte).unwrap_or(0),
            0,
            "the displaced TUI socket must not remain half-open"
        );
    }

    #[test]
    fn scraped_post_normalizes() {
        let raw = ScrapedPost {
            id: "42".into(),
            text: "hello".into(),
            name: "Ada".into(),
            username: "ada".into(),
            verified: true,
            created_at: Some("2026-08-10T00:00:00Z".into()),
            replies: 1,
            reposts: 2,
            likes: 3,
            views: 4,
            media: vec![],
            quoted: None,
        };
        let post = raw.into_post().unwrap();
        assert_eq!(post.permalink(), "https://x.com/ada/status/42");
        assert_eq!(post.metrics.like_count, 3);
    }
}
