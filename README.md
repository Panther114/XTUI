# XTUI

**X, without leaving your terminal.** XTUI is a read-focused, keyboard-first X client with a
responsive, strictly black-and-white interface. Its browser extension uses the X session already
signed in to Edge or Chrome, so XTUI never copies cookies or asks for a second browser login.

## Highlights

- Following and For You timelines, search, mentions, bookmarks, profiles, likes, Lists, and threads
- Continuous background pagination with a 144-post reservoir, deduplication, and a bounded
  800-post memory window
- Route-isolated, inactive muted X transport tabs keep timeline harvesting alive while a thread,
  profile, or search is open
- Responsive wide, medium, compact, and tiny terminal layouts
- Keyboard, mouse wheel, click selection, bracketed paste, configurable keys, and grayscale themes
- Non-blocking fetches and event-driven redraws; idle XTUI sleeps instead of polling
- Deterministic offline demo and optional official X OAuth/API mode

## Build and install

```powershell
cargo build --release
Copy-Item .\target\release\xtui.exe .\xtui.exe -Force
npm link
xtui --version
```

`npm link` creates the user-level `xtui` command without publishing anything. Tagged GitHub installs
also work through `npx --yes github:panther114/xtui`; the launcher downloads the release binary to a
stable per-user cache.

## Install the Edge extension

```powershell
xtui extension install --edge
xtui extension path
```

Then complete Edge's one required consent step:

1. Open `edge://extensions`.
2. Enable **Developer mode**.
3. Select **Load unpacked**.
4. Select the folder printed by `xtui extension path`.

XTUI uses the deterministic development extension ID `iepklfmnjidigljfaegfjlbeghpjejka`. Sign in to
X normally in Edge if you are not already signed in, then verify:

```powershell
xtui extension status --edge
xtui extension check --edge
xtui
```

For Chrome, replace `--edge` with `--chrome` and load the same folder from
`chrome://extensions`. Published store builds use the same source package, but store submission is
separate from this repository build.

## How the bridge works

```text
XTUI TUI <-> loopback framed channel <-> native messaging host
                                            ^
                                            |
                              MV3 service worker <-> inactive x.com route tabs
```

- The extension is limited to `https://x.com/*`, `nativeMessaging`, alarms, and tab management.
- It has no cookie permission, telemetry, X write actions, or arbitrary browser automation.
- Transport tabs are never focused and never replace user tabs. Videos are paused and animations
  disabled in those tabs. Old routes are bounded and evicted.
- Clean XTUI exit closes the tabs immediately. A lost connection closes them through a 30-second
  watchdog.
- X can change its rendered page structure; `xtui extension check` distinguishes connection,
  sign-in, rendering, and pagination failures.

## Keys

| Keys | Action |
|---|---|
| `j` / `↓` | Move down and prefetch near the end |
| `k` / `↑` | Move up |
| `g` / `G` | Top / bottom |
| `Enter` / `→` | Open post or list |
| `Esc` / `←` | Back |
| `Tab` | Focus navigation rail |
| `/` | Search |
| `1`–`5` | Home, Explore, Mentions, Bookmarks, Lists |
| `f` | Following / For You |
| `M` / `V` / `O` | Preview media / open media / open post |
| `?` | Contextual help |
| `Q` | Quit |

## Configuration

XTUI reads `%APPDATA%\xtui\config.json` on Windows and `~/.config/xtui/config.json` on Linux.
Legacy `"source": "browser"` values are treated as `"extension"` during migration.

```json
{
  "source": "extension",
  "auto_refresh_secs": 300,
  "theme": {
    "accent": "#FFFFFF",
    "gray": "#9C9C9C",
    "background": "#000000"
  },
  "keys": {
    "move_down": ["j", "down"],
    "move_up": ["k", "up"],
    "search": "/",
    "quit": "q"
  }
}
```

Every configured color is converted to grayscale. Set `auto_refresh_secs` to `0` to disable silent
Home refresh.

## Optional official API mode

Run `xtui login YOUR_CLIENT_ID` for OAuth 2.0 PKCE after configuring the native-app callback
`http://127.0.0.1:17171/callback` in the X Developer Console. This route can require paid API
credits. `XTUI_CLIENT_ID` and `XTUI_ACCESS_TOKEN` remain supported.

## Development and validation

```powershell
node --test .\extension\tests\manifest.test.js
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --lib
cargo test
cargo build --release --locked
```

The official Grok Build source is cloned only as a local landing-page reference under
`.grok-build-reference/` and is ignored by Git. XTUI's implementation remains native Ratatui code.

## Scope

XTUI is read-only. Posting, replying, liking, reposting, following, and direct messages are not
implemented. Normal production media/permalink handoff remains available; automated tests replace
the OS opener with a recorder so validation cannot launch a browser or file.
