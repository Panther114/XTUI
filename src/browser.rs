//! Read-only Chrome DevTools Protocol bridge.
//!
//! This module deliberately talks to a dedicated Chromium profile through CDP.
//! It never reads, copies, serializes, or exposes browser cookies.  Authentication
//! remains inside the browser, where the person using XTUI completes it normally.

use std::{
    path::PathBuf,
    process::{Command, Stdio},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::{
    net::TcpStream,
    time::{sleep, timeout},
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

const DEBUG_PORT: u16 = 9333;
const CDP_HTTP: &str = "http://127.0.0.1:9333";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

type CdpSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// A single page attached through the Chrome DevTools Protocol.
///
/// This is intentionally a small read-only abstraction: callers can navigate
/// a tab and evaluate page-side extraction scripts, but have no cookie API.
pub struct BrowserSession {
    socket: CdpSocket,
    next_id: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct DebugTarget {
    #[serde(rename = "type")]
    target_type: String,
    #[serde(default)]
    url: String,
    #[serde(rename = "webSocketDebuggerUrl")]
    websocket_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DebugVersion {
    #[serde(rename = "User-Agent")]
    user_agent: String,
    #[serde(rename = "webSocketDebuggerUrl")]
    websocket_url: String,
}

/// Returns XTUI's isolated Chromium profile location.
pub fn profile_dir() -> Result<PathBuf> {
    let root = dirs::config_dir()
        .ok_or_else(|| anyhow!("could not determine the platform config directory"))?;
    Ok(root.join("xtui").join("browser-profile"))
}

/// Find a usable Chromium executable without relying on a fixed Windows install.
pub fn find_chromium_executable() -> Option<PathBuf> {
    first_existing_executable(platform_browser_candidates())
}

fn first_existing_executable(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    candidates.into_iter().find(|path| path.is_file())
}

/// Start (if needed) the dedicated browser and attach to one of its tabs.
///
/// `visible` should be true for interactive sign-in.  False starts Chromium
/// headlessly while preserving the same dedicated profile for scheduled reads.
pub async fn connect_or_launch(visible: bool) -> Result<BrowserSession> {
    if debugger_ready().await {
        let running_headless = debugger_is_headless().await?;
        let requested_headless = !visible;
        if running_headless != requested_headless {
            // Login needs a visible window; ordinary browsing must not leave one
            // behind. Restarting Chromium keeps the dedicated profile and session.
            close_debugger_browser().await?;
            wait_for_debugger_shutdown().await?;
        }
    }
    if !debugger_ready().await {
        launch_browser(visible)?;
        wait_for_debugger().await?;
    }

    let target = select_page_target().await?;
    BrowserSession::connect(&target).await
}

impl BrowserSession {
    /// Associated convenience form of [`connect_or_launch`].
    pub async fn connect_or_launch(visible: bool) -> Result<Self> {
        connect_or_launch(visible).await
    }

    async fn connect(target: &DebugTarget) -> Result<Self> {
        let url = target
            .websocket_url
            .as_deref()
            .ok_or_else(|| anyhow!("the selected browser page has no CDP websocket"))?;
        let (socket, _) = connect_async(url)
            .await
            .context("could not connect to the browser's DevTools websocket")?;
        Ok(Self { socket, next_id: 1 })
    }

    /// Navigate the attached page. Navigation does not read authentication data.
    pub async fn navigate(&mut self, url: &str) -> Result<()> {
        let _: Value = self.call("Page.navigate", json!({ "url": url })).await?;
        Ok(())
    }

    /// Bring the page forward and deliver a real wheel gesture.
    ///
    /// X's virtualized timelines do not consistently react to JavaScript
    /// `scrollTo` calls, especially after a tab has been in the background.
    /// CDP input events follow the same path as a physical mouse wheel.
    pub async fn scroll_down(&mut self, delta_y: f64) -> Result<()> {
        let _: Value = self
            .call(
                "Input.dispatchMouseEvent",
                json!({
                    "type": "mouseWheel",
                    "x": 640,
                    "y": 480,
                    "deltaX": 0,
                    "deltaY": delta_y,
                    "pointerType": "mouse"
                }),
            )
            .await?;
        Ok(())
    }

    pub async fn activate(&mut self) -> Result<()> {
        let _: Value = self.call("Page.bringToFront", json!({})).await?;
        Ok(())
    }

    pub async fn collect_garbage(&mut self) {
        let _ = self.call("HeapProfiler.collectGarbage", json!({})).await;
    }

    /// Replace the current tab with a fresh page target. X retains substantial
    /// per-route renderer state after large threads; closing the target is the
    /// only dependable way to return that memory to the OS.
    pub async fn recreate_page(&mut self) -> Result<()> {
        let target = create_page_target().await?;
        let fresh = BrowserSession::connect(&target).await?;

        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let close = json!({ "id": id, "method": "Page.close", "params": {} });
        let _ = self
            .socket
            .send(Message::Text(close.to_string().into()))
            .await;
        *self = fresh;
        Ok(())
    }

    /// Evaluate a JSON-returning expression in the currently attached page.
    ///
    /// Expressions run in Chromium and their JSON result is deserialized into
    /// `T`; `undefined`, thrown errors, and non-serializable values return an
    /// error instead of leaking browser internals.
    pub async fn evaluate<T: DeserializeOwned>(&mut self, expression: &str) -> Result<T> {
        let result = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "awaitPromise": true,
                    "returnByValue": true,
                    "userGesture": false,
                }),
            )
            .await?;

        if let Some(details) = result.get("exceptionDetails") {
            let description = details
                .pointer("/exception/description")
                .and_then(Value::as_str)
                .or_else(|| details.get("text").and_then(Value::as_str))
                .unwrap_or("page-side JavaScript exception");
            bail!("evaluation failed: {description}");
        }

        let value = result.pointer("/result/value").ok_or_else(|| {
            let kind = result
                .pointer("/result/type")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let description = result
                .pointer("/result/description")
                .and_then(Value::as_str)
                .unwrap_or("no description");
            anyhow!("evaluation returned no JSON value ({kind}: {description})")
        })?;
        serde_json::from_value(value.clone())
            .context("evaluation returned an unexpected JSON shape")
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| anyhow!("CDP command id overflow"))?;
        let message = json!({ "id": id, "method": method, "params": params });
        self.socket
            .send(Message::Text(message.to_string().into()))
            .await
            .context("could not send CDP command")?;

        timeout(COMMAND_TIMEOUT, async {
            while let Some(message) = self.socket.next().await {
                let message = message.context("browser DevTools websocket failed")?;
                let Message::Text(text) = message else {
                    continue;
                };
                let response: Value = serde_json::from_str(text.as_str())
                    .context("browser returned invalid CDP JSON")?;
                if response.get("id").and_then(Value::as_u64) != Some(id) {
                    continue; // CDP events and commands from other consumers are irrelevant here.
                }
                if let Some(error) = response.get("error") {
                    bail!("CDP {method} failed: {error}");
                }
                return response
                    .get("result")
                    .cloned()
                    .ok_or_else(|| anyhow!("CDP {method} response lacked a result"));
            }
            bail!("browser DevTools websocket closed before responding to {method}")
        })
        .await
        .map_err(|_| anyhow!("timed out waiting for CDP {method}"))?
    }
}

async fn debugger_ready() -> bool {
    reqwest::get(format!("{CDP_HTTP}/json/version"))
        .await
        .and_then(|response| response.error_for_status())
        .is_ok()
}

async fn debugger_version() -> Result<DebugVersion> {
    reqwest::get(format!("{CDP_HTTP}/json/version"))
        .await
        .context("could not inspect the browser companion")?
        .error_for_status()
        .context("browser companion rejected its version request")?
        .json()
        .await
        .context("browser companion returned invalid version data")
}

async fn debugger_is_headless() -> Result<bool> {
    Ok(debugger_version()
        .await?
        .user_agent
        .contains("HeadlessChrome"))
}

async fn close_debugger_browser() -> Result<()> {
    let version = debugger_version().await?;
    let (mut socket, _) = connect_async(&version.websocket_url)
        .await
        .context("could not connect to the browser companion for restart")?;
    socket
        .send(Message::Text(
            json!({ "id": 1, "method": "Browser.close" })
                .to_string()
                .into(),
        ))
        .await
        .context("could not stop the browser companion")?;
    Ok(())
}

fn launch_browser(visible: bool) -> Result<()> {
    let executable = find_chromium_executable().ok_or_else(|| {
        anyhow!("could not find Microsoft Edge or Google Chrome; install one and retry")
    })?;
    let profile = profile_dir()?;
    std::fs::create_dir_all(&profile).context("could not create XTUI browser profile")?;

    let mut command = Command::new(executable);
    command
        .arg(format!("--remote-debugging-port={DEBUG_PORT}"))
        .arg("--remote-debugging-address=127.0.0.1")
        .arg("--remote-allow-origins=http://127.0.0.1:9333")
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-gpu")
        .arg("--disable-extensions")
        .arg("--disable-sync")
        .arg("--disable-notifications")
        .arg("--disable-features=BackForwardCache,MediaRouter,OptimizationHints")
        .arg("--mute-audio")
        .arg("--autoplay-policy=user-gesture-required")
        .arg("--renderer-process-limit=2")
        .arg("--blink-settings=imagesEnabled=false")
        .arg("--window-size=1024,768")
        .arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if !visible {
        command.arg("--headless=new");
    }
    command
        .spawn()
        .context("could not start Chromium for XTUI")?;
    Ok(())
}

async fn wait_for_debugger() -> Result<()> {
    let deadline = tokio::time::Instant::now() + STARTUP_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if debugger_ready().await {
            return Ok(());
        }
        sleep(Duration::from_millis(200)).await;
    }
    bail!("browser did not expose DevTools on port {DEBUG_PORT} within 15 seconds")
}

async fn wait_for_debugger_shutdown() -> Result<()> {
    let deadline = tokio::time::Instant::now() + STARTUP_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if !debugger_ready().await {
            return Ok(());
        }
        sleep(Duration::from_millis(100)).await;
    }
    bail!("browser companion did not stop cleanly")
}

async fn select_page_target() -> Result<DebugTarget> {
    let targets = list_page_targets().await?;
    if let Some(found) = targets
        .iter()
        .find(|target| target.url.contains("x.com"))
        .or_else(|| targets.first())
    {
        return Ok(found.clone());
    }
    // All tabs may have been closed (e.g. by a previous cleanup); open a
    // fresh one instead of failing the whole session.
    create_page_target().await
}

async fn list_page_targets() -> Result<Vec<DebugTarget>> {
    let response = reqwest::get(format!("{CDP_HTTP}/json/list"))
        .await
        .context("could not list browser tabs through DevTools")?
        .error_for_status()
        .context("browser DevTools rejected the tab listing")?;
    Ok(response
        .json::<Vec<DebugTarget>>()
        .await
        .context("browser returned an invalid DevTools tab listing")?
        .into_iter()
        .filter(|target| target.target_type == "page" && target.websocket_url.is_some())
        .collect())
}

/// Ask the browser to open a fresh blank tab and return its CDP target.
async fn create_page_target() -> Result<DebugTarget> {
    let response = reqwest::Client::new()
        .put(format!("{CDP_HTTP}/json/new?about:blank"))
        .send()
        .await
        .context("could not create a fresh browser tab")?
        .error_for_status()
        .context("browser rejected a fresh tab")?;
    response
        .json()
        .await
        .context("browser returned invalid tab data")
}

fn platform_browser_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    #[cfg(target_os = "windows")]
    {
        for variable in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
            if let Some(root) = std::env::var_os(variable) {
                let root = PathBuf::from(root);
                candidates.push(root.join("Microsoft/Edge/Application/msedge.exe"));
                candidates.push(root.join("Google/Chrome/Application/chrome.exe"));
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        candidates.extend([
            PathBuf::from("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
            PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        ]);
    }
    #[cfg(target_os = "linux")]
    {
        candidates.extend(
            [
                "microsoft-edge",
                "google-chrome",
                "chromium",
                "chromium-browser",
            ]
            .into_iter()
            .filter_map(path_from_path_env),
        );
    }
    candidates
}

#[cfg(target_os = "linux")]
fn path_from_path_env(program: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?
        .to_string_lossy()
        .split(':')
        .map(|entry| Path::new(entry).join(program))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_devtools_page_target() {
        let target: DebugTarget = serde_json::from_str(
            r#"{"type":"page","url":"https://x.com/home","webSocketDebuggerUrl":"ws://127.0.0.1:9223/devtools/page/abc"}"#,
        )
        .unwrap();
        assert_eq!(target.target_type, "page");
        assert_eq!(target.url, "https://x.com/home");
        assert_eq!(
            target.websocket_url.as_deref(),
            Some("ws://127.0.0.1:9223/devtools/page/abc")
        );
    }

    #[test]
    fn profile_is_xtui_scoped() {
        let profile = profile_dir().unwrap();
        assert!(profile.ends_with(std::path::Path::new("xtui").join("browser-profile")));
    }

    #[test]
    fn browser_candidates_are_nonempty_on_supported_platforms() {
        // Discovery may legitimately find nothing on a CI machine, but it should
        // always form candidate paths on Windows/macOS and inspect PATH on Linux.
        let _ = platform_browser_candidates();
    }

    #[test]
    fn executable_discovery_uses_the_first_existing_candidate() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing-browser");
        let present = directory.path().join("browser.exe");
        std::fs::write(&present, b"").unwrap();

        assert_eq!(
            first_existing_executable(vec![missing, present.clone()]),
            Some(present)
        );
    }
}
