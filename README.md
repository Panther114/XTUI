# XTUI

**X, without leaving your terminal.** XTUI is a read-focused, keyboard-first X client with a responsive black-and-white terminal interface. It can read x.com through an isolated browser companion without paid API credits.

![Rust](https://img.shields.io/badge/Rust-1.85%2B-black) ![License](https://img.shields.io/badge/license-MIT-white)

## What works

- Reverse-chronological Home / Following timeline
- Root-post feed with thread drill-down and retractable replies
- Pinned reading position: the selected post stays at the top with compact next-post previews
- Recent search, mentions, bookmarks, profiles, user posts, likes, and Lists
- Reposts, quoted posts, metrics, verification badges, and responsive cards
- OAuth 2.0 Authorization Code + PKCE login with automatic token refresh
- Photo previews rendered as monochrome Unicode in any modern terminal
- Highest-bitrate MP4 handoff for video/GIF media, plus browser permalink handoff
- Wide three-column, medium two-column, and compact single-column layouts
- Mouse-wheel scrolling, resize support, errors, empty states, and contextual help
- A deterministic demo account for exploring every screen without credentials

## Quick start

```powershell
cargo build --release
.\target\release\xtui.exe browser-login
# Sign in to X in the browser window, then:
.\target\release\xtui.exe browser
```

XTUI remembers browser mode after setup, so subsequent launches need only `xtui.exe`.

For a completely offline tour, use `xtui.exe demo`.

### Install the local `xtui` command (Windows)

After building the release binary, link this repository as a local npm package:

```powershell
cargo build --release
Copy-Item .\target\release\xtui.exe .\xtui.exe
npm link
xtui --help
```

`npm link` does not publish anything. It creates a user-level command shim (normally `%APPDATA%\npm\xtui.cmd`), so new terminals can run `xtui`, `xtui demo`, or `xtui browser` directly. Re-run `npm link` after moving the repository or rebuilding from a different checkout.

Inside XTUI, the complete shortcut dock stays visible at the bottom-right on wide layouts. The essentials are `↑`/`↓` to move, `→` to open, `←` to go back, `Tab` to drive the left sidebar (then `↑`/`↓` to choose a section and `→` to open it), `/` to search, `1`–`5` for sections, `M` for a media preview, and `Q` to quit. Search input supports `←`/`→` cursor editing, `Home`/`End` jumps, and `Ctrl+U` to clear.

## Free browser-companion mode

Browser mode launches Microsoft Edge or Google Chrome with an isolated profile at `%APPDATA%\xtui\browser-profile` and a loopback-only debugging port. Sign into X in the visible login window once. Normal XTUI sessions then restart that profile headlessly and read the same rendered post cards you see on x.com—no browser window needs to remain open.

- XTUI never copies or exports cookies.
- The bridge offers navigation and rendered-page extraction only.
- Requests are initiated only by navigation, refresh, or explicit next-page actions.
- Closing the headless companion process is safe; XTUI reopens the same isolated profile.
- The renderer suppresses image decoding and autoplay while retaining media URLs for on-demand previews.
- X can change its page structure at any time, so this mode is less stable than an official API.
- Automated access may be restricted by X's terms or anti-scraping rules. Use it only for your own account and at your own risk.

## Optional official API mode

The supported official API remains available when prepaid X API credits are acceptable.

1. Create a project/app in the [X Developer Console](https://developer.x.com/en/portal/dashboard).
2. Enable OAuth 2.0 and select **Native App / public client**.
3. Register this callback exactly: `http://127.0.0.1:17171/callback`.
4. Fund the app's API credits as required by X.
5. Run `cargo run --release -- login YOUR_CLIENT_ID`.
6. Run `cargo run --release`.

You can alternatively provide `XTUI_CLIENT_ID` and `XTUI_ACCESS_TOKEN`. Saved credentials live in the OS config directory (`%APPDATA%\xtui\config.json` on Windows, `~/.config/xtui/config.json` on Linux). Treat that file as a secret. Use the environment variable route when plaintext local token storage is inappropriate.

Requested OAuth scopes are read-only: `tweet.read`, `users.read`, `follows.read`, `bookmark.read`, `like.read`, `list.read`, and `offline.access`. XTUI does not request write or DM access.

## X API realities

The official API provides a reverse-chronological Following timeline, not X's algorithmic **For You** feed. It also lacks a consolidated pull-style Notifications endpoint, so XTUI calls that screen **Mentions**. Recent-search-backed threads can omit replies older than the API's search window.

X currently bills API reads per resource. XTUI uses pages of 50 only on explicit navigation and does not speculatively prefetch, but a live feed can still consume paid credits. Demo mode never contacts X.

## Layout

```text
┌───────────────┬────────────────────────────────────────┬──────────────────────┐
│       𝕏       │ Home                                   │ Search               │
│  1  Home      │                 Following              │ What's happening     │
│  2  Explore   ├────────────────────────────────────────┤                      │
│  3  Mentions  │ Ada  @ada_codes · 4m                   │ Selected             │
│  4  Bookmarks │ I replaced my browser scroll with…     │ @ada_codes           │
│  5  Lists     │ ◯ 38    ↻ 226    ♡ 2.4K               │ 38 replies           │
│               ├────────────────────────────────────────┤                      │
└───────────────┴────────────────────────────────────────┴──────────────────────┘
```

At widths below 120 columns the context rail disappears; below 86 columns navigation becomes a bottom tab bar.

## Development

```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

The `Api` trait isolates X transport from application and UI state. `DemoApi` is the reproducible acceptance backend; `XApi` normalizes X's expansion-heavy responses into the same domain model.

## Scope

XTUI is intentionally read-focused. Posting, replying, liking, reposting, following, and direct messages are not implemented. Rich X cards, live Spaces, algorithmic ranking, and native terminal video playback are outside the official API/portable-terminal envelope; the original post or media opens with one key.
