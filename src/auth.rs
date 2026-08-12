use crate::config::Config;
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use url::Url;

const REDIRECT_URI: &str = "http://127.0.0.1:17171/callback";

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
}

pub async fn login(client_id: Option<&str>) -> Result<()> {
    let client_id = client_id
        .map(str::to_owned)
        .or_else(|| Config::load().ok()?.client_id)
        .context(
            "a client ID is required; run `xtui login YOUR_CLIENT_ID` or set XTUI_CLIENT_ID",
        )?;

    let verifier = random_token(64);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_token(32);
    let scopes =
        "tweet.read users.read follows.read bookmark.read like.read list.read offline.access";
    let mut authorization = Url::parse("https://x.com/i/oauth2/authorize")?;
    authorization
        .query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &client_id)
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("scope", scopes)
        .append_pair("state", &state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");

    let listener = TcpListener::bind("127.0.0.1:17171")
        .await
        .context("could not listen on localhost:17171 for the OAuth callback")?;
    println!("Opening X authorization in your browser…");
    println!("If it does not open, visit:\n{authorization}\n");
    let _ = open::that(authorization.as_str());

    let (mut socket, _) = listener.accept().await?;
    let mut request = vec![0; 8192];
    let read = socket.read(&mut request).await?;
    let first_line = String::from_utf8_lossy(&request[..read])
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();
    let target = first_line
        .split_whitespace()
        .nth(1)
        .context("invalid OAuth callback")?;
    let callback = Url::parse(&format!("http://localhost{target}"))?;
    let params: std::collections::HashMap<_, _> = callback.query_pairs().into_owned().collect();

    let response = if params.get("state") != Some(&state) {
        "OAuth state did not match. You can close this tab."
    } else if params.contains_key("code") {
        "XTUI is authorized. You can close this tab and return to the terminal."
    } else {
        "X did not authorize XTUI. You can close this tab."
    };
    let html = format!(
        "<html><body style='background:#000;color:#fff;font:20px sans-serif;padding:4rem'><h1>𝕏TUI</h1><p>{response}</p></body></html>"
    );
    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", html.len(), html).as_bytes()).await?;

    if params.get("state") != Some(&state) {
        bail!("OAuth state mismatch; login was cancelled for safety");
    }
    if let Some(error) = params.get("error") {
        bail!("X authorization failed: {error}");
    }
    let code = params
        .get("code")
        .context("X callback did not contain an authorization code")?;
    let token: TokenResponse = reqwest::Client::new()
        .post("https://api.x.com/2/oauth2/token")
        .form(&[
            ("code", code.as_str()),
            ("grant_type", "authorization_code"),
            ("client_id", client_id.as_str()),
            ("redirect_uri", REDIRECT_URI),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await?
        .error_for_status()
        .context("X rejected the OAuth token exchange")?
        .json()
        .await?;

    let mut config = Config::load()?;
    config.client_id = Some(client_id);
    config.access_token = Some(token.access_token);
    config.refresh_token = token.refresh_token;
    config.expires_at = token
        .expires_in
        .map(|seconds| chrono::Utc::now().timestamp() + seconds);
    config.save()?;
    println!("Login saved. Start XTUI with `xtui`.");
    Ok(())
}

pub fn logout() -> Result<()> {
    let path = Config::path()?;
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

fn random_token(bytes: usize) -> String {
    let mut raw = vec![0; bytes];
    rand::rng().fill_bytes(&mut raw);
    URL_SAFE_NO_PAD.encode(raw)
}
