use crate::{config::Config, model::*};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::{Client, Url};
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

#[async_trait]
pub trait Api: Send + Sync {
    async fn me(&self) -> Result<User>;
    async fn home(&self, next: Option<&str>) -> Result<Page<Post>>;
    /// Select which Home timeline to serve. Only the browser extension can
    /// honor this; the official API and demo fall back to Following.
    async fn set_feed(&self, _feed: FeedKind) {}
    /// Release non-home browser transports while preserving their session
    /// caches. Browser-backed implementations may override this.
    async fn release_secondary(&self) {}
    async fn search(&self, query: &str, next: Option<&str>) -> Result<Page<Post>>;
    async fn thread(&self, conversation_id: &str) -> Result<Vec<Post>>;
    async fn bookmarks(&self, next: Option<&str>) -> Result<Page<Post>>;
    async fn likes(&self, user_id: &str, next: Option<&str>) -> Result<Page<Post>>;
    async fn mentions(&self, next: Option<&str>) -> Result<Page<Post>>;
    async fn user_by_username(&self, username: &str) -> Result<User>;
    async fn user_posts(&self, user_id: &str, next: Option<&str>) -> Result<Page<Post>>;
    async fn lists(&self) -> Result<Vec<XList>>;
    async fn list_posts(&self, list_id: &str, next: Option<&str>) -> Result<Page<Post>>;
}

pub struct XApi {
    client: Client,
    token: tokio::sync::RwLock<String>,
    client_id: Option<String>,
    refresh_token: tokio::sync::RwLock<Option<String>>,
    refresh_lock: tokio::sync::Mutex<()>,
    user_id: tokio::sync::RwLock<Option<String>>,
}

impl XApi {
    pub fn new(config: Config) -> Result<Self> {
        Ok(Self {
            client: Client::builder()
                .user_agent("XTUI/0.1")
                .connect_timeout(Duration::from_secs(10))
                // The TUI awaits every request inline; a stalled connection
                // would otherwise freeze the whole interface.
                .timeout(Duration::from_secs(60))
                .build()?,
            token: tokio::sync::RwLock::new(
                config
                    .access_token()
                    .context("XTUI is not logged in")?
                    .to_owned(),
            ),
            client_id: config.client_id,
            refresh_token: tokio::sync::RwLock::new(config.refresh_token),
            refresh_lock: tokio::sync::Mutex::new(()),
            user_id: tokio::sync::RwLock::new(config.user_id),
        })
    }

    async fn get<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<T> {
        let url = Url::parse(&format!("https://api.x.com/2/{path}"))?;
        let mut response = self
            .client
            .get(url.clone())
            .bearer_auth(self.token.read().await.as_str())
            .query(params)
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            && self.refresh_token.read().await.is_some()
        {
            self.refresh_access_token().await?;
            response = self
                .client
                .get(url)
                .bearer_auth(self.token.read().await.as_str())
                .query(params)
                .send()
                .await?;
        }
        let status = response.status();
        let body = response.bytes().await?;
        if !status.is_success() {
            let detail = serde_json::from_slice::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v.get("detail").and_then(|v| v.as_str()).map(str::to_owned))
                .unwrap_or_else(|| String::from_utf8_lossy(&body).chars().take(240).collect());
            bail!("X API returned {status}: {detail}");
        }
        serde_json::from_slice(&body).context("X returned an unexpected response")
    }

    async fn refresh_access_token(&self) -> Result<()> {
        #[derive(Deserialize)]
        struct Refreshed {
            access_token: String,
            refresh_token: Option<String>,
            expires_in: Option<i64>,
        }
        let _guard = self.refresh_lock.lock().await;
        let refresh = self
            .refresh_token
            .read()
            .await
            .clone()
            .context("the X session expired; run `xtui login` again")?;
        let client_id = self
            .client_id
            .as_deref()
            .context("cannot refresh without XTUI_CLIENT_ID")?;
        let token: Refreshed = self
            .client
            .post("https://api.x.com/2/oauth2/token")
            .form(&[
                ("refresh_token", refresh.as_str()),
                ("grant_type", "refresh_token"),
                ("client_id", client_id),
            ])
            .send()
            .await?
            .error_for_status()
            .context("X rejected the refresh token; run `xtui login` again")?
            .json()
            .await?;
        *self.token.write().await = token.access_token.clone();
        if let Some(rotated) = token.refresh_token.clone() {
            *self.refresh_token.write().await = Some(rotated);
        }
        let mut config = Config::load()?;
        config.access_token = Some(token.access_token);
        if token.refresh_token.is_some() {
            config.refresh_token = token.refresh_token;
        }
        config.expires_at = token.expires_in.map(|s| Utc::now().timestamp() + s);
        config.save()?;
        Ok(())
    }

    async fn my_id(&self) -> Result<String> {
        if let Some(id) = self.user_id.read().await.clone() {
            return Ok(id);
        }
        let me = self.me().await?;
        *self.user_id.write().await = Some(me.id.clone());
        Ok(me.id)
    }

    async fn posts(
        &self,
        path: &str,
        mut params: Vec<(&str, String)>,
        next: Option<&str>,
    ) -> Result<Page<Post>> {
        params.extend(post_fields());
        if let Some(token) = next {
            params.push(("pagination_token", token.to_owned()));
        }
        let response: ApiResponse<Vec<RawPost>> = self.get(path, &params).await?;
        Ok(response.into_posts())
    }
}

#[async_trait]
impl Api for XApi {
    async fn me(&self) -> Result<User> {
        let response: ApiResponse<RawUser> = self.get("users/me", &user_fields()).await?;
        response
            .data
            .context("X did not return the authenticated user")
            .map(Into::into)
    }
    async fn home(&self, next: Option<&str>) -> Result<Page<Post>> {
        self.posts(
            &format!(
                "users/{}/timelines/reverse_chronological",
                self.my_id().await?
            ),
            vec![("max_results", "20".into())],
            next,
        )
        .await
    }
    async fn search(&self, query: &str, next: Option<&str>) -> Result<Page<Post>> {
        self.posts(
            "tweets/search/recent",
            vec![
                ("query", format!("{query} -is:retweet")),
                ("max_results", "20".into()),
            ],
            next,
        )
        .await
    }
    async fn thread(&self, conversation_id: &str) -> Result<Vec<Post>> {
        let mut page = self
            .search(&format!("conversation_id:{conversation_id}"), None)
            .await?;
        page.items.sort_by_key(|post| post.created_at);
        Ok(page.items)
    }
    async fn bookmarks(&self, next: Option<&str>) -> Result<Page<Post>> {
        self.posts(
            &format!("users/{}/bookmarks", self.my_id().await?),
            vec![("max_results", "20".into())],
            next,
        )
        .await
    }
    async fn likes(&self, user_id: &str, next: Option<&str>) -> Result<Page<Post>> {
        self.posts(
            &format!("users/{user_id}/liked_tweets"),
            vec![("max_results", "20".into())],
            next,
        )
        .await
    }
    async fn mentions(&self, next: Option<&str>) -> Result<Page<Post>> {
        self.posts(
            &format!("users/{}/mentions", self.my_id().await?),
            vec![("max_results", "20".into())],
            next,
        )
        .await
    }
    async fn user_by_username(&self, username: &str) -> Result<User> {
        let response: ApiResponse<RawUser> = self
            .get(
                &format!("users/by/username/{}", username.trim_start_matches('@')),
                &user_fields(),
            )
            .await?;
        response.data.context("user not found").map(Into::into)
    }
    async fn user_posts(&self, user_id: &str, next: Option<&str>) -> Result<Page<Post>> {
        self.posts(
            &format!("users/{user_id}/tweets"),
            vec![
                ("max_results", "20".into()),
                ("exclude", "retweets,replies".into()),
            ],
            next,
        )
        .await
    }
    async fn lists(&self) -> Result<Vec<XList>> {
        let response: ApiResponse<Vec<RawList>> = self
            .get(
                &format!("users/{}/owned_lists", self.my_id().await?),
                &[(
                    "list.fields",
                    "description,follower_count,member_count,private".into(),
                )],
            )
            .await?;
        Ok(response
            .data
            .unwrap_or_default()
            .into_iter()
            .map(Into::into)
            .collect())
    }
    async fn list_posts(&self, list_id: &str, next: Option<&str>) -> Result<Page<Post>> {
        self.posts(
            &format!("lists/{list_id}/tweets"),
            vec![("max_results", "20".into())],
            next,
        )
        .await
    }
}

fn post_fields() -> Vec<(&'static str, String)> {
    vec![
        ("tweet.fields", "id,text,author_id,created_at,conversation_id,in_reply_to_user_id,lang,public_metrics,attachments,referenced_tweets".into()),
        ("expansions", "author_id,attachments.media_keys,referenced_tweets.id,referenced_tweets.id.author_id".into()),
        ("user.fields", "id,name,username,verified,description,profile_image_url,public_metrics".into()),
        ("media.fields", "media_key,type,url,preview_image_url,alt_text,variants".into()),
    ]
}
fn user_fields() -> Vec<(&'static str, String)> {
    vec![(
        "user.fields",
        "id,name,username,verified,description,profile_image_url,public_metrics".into(),
    )]
}

#[derive(Deserialize, Default)]
struct ApiResponse<T> {
    data: Option<T>,
    includes: Option<Includes>,
    meta: Option<Meta>,
}
#[derive(Deserialize, Default)]
struct Includes {
    #[serde(default)]
    users: Vec<RawUser>,
    #[serde(default)]
    media: Vec<RawMedia>,
    #[serde(default)]
    tweets: Vec<RawPost>,
}
#[derive(Deserialize, Default)]
struct Meta {
    next_token: Option<String>,
}

impl ApiResponse<Vec<RawPost>> {
    fn into_posts(self) -> Page<Post> {
        let includes = self.includes.unwrap_or_default();
        let users: HashMap<_, _> = includes
            .users
            .into_iter()
            .map(|u| (u.id.clone(), User::from(u)))
            .collect();
        let media: HashMap<_, _> = includes
            .media
            .into_iter()
            .map(|m| (m.media_key.clone(), Media::from(m)))
            .collect();
        let referenced: HashMap<_, _> = includes
            .tweets
            .into_iter()
            .map(|p| (p.id.clone(), p))
            .collect();
        let items = self
            .data
            .unwrap_or_default()
            .into_iter()
            .map(|raw| convert_post(raw, &users, &media, &referenced))
            .collect();
        Page {
            items,
            next_token: self.meta.and_then(|m| m.next_token),
        }
    }
}

#[derive(Clone, Deserialize, Default)]
struct RawPost {
    id: String,
    text: String,
    author_id: Option<String>,
    created_at: Option<DateTime<Utc>>,
    conversation_id: Option<String>,
    in_reply_to_user_id: Option<String>,
    lang: Option<String>,
    #[serde(default)]
    public_metrics: PostMetrics,
    attachments: Option<Attachments>,
    #[serde(default)]
    referenced_tweets: Vec<Reference>,
}
#[derive(Clone, Deserialize, Default)]
struct Attachments {
    #[serde(default)]
    media_keys: Vec<String>,
}
#[derive(Clone, Deserialize)]
struct Reference {
    #[serde(rename = "type")]
    kind: String,
    id: String,
}
#[derive(Clone, Deserialize, Default)]
struct RawUser {
    id: String,
    name: String,
    username: String,
    #[serde(default)]
    verified: bool,
    #[serde(default)]
    description: String,
    profile_image_url: Option<String>,
    public_metrics: Option<UserMetrics>,
}
#[derive(Clone, Deserialize)]
struct RawMedia {
    media_key: String,
    #[serde(rename = "type")]
    kind: MediaKind,
    url: Option<String>,
    preview_image_url: Option<String>,
    alt_text: Option<String>,
    #[serde(default)]
    variants: Vec<MediaVariant>,
}
#[derive(Deserialize)]
struct RawList {
    id: String,
    name: String,
    #[serde(default)]
    description: String,
    member_count: Option<u64>,
    follower_count: Option<u64>,
    #[serde(default)]
    private: bool,
}

impl From<RawUser> for User {
    fn from(u: RawUser) -> Self {
        Self {
            id: u.id,
            name: u.name,
            username: u.username,
            verified: u.verified,
            description: u.description,
            profile_image_url: u.profile_image_url,
            public_metrics: u.public_metrics,
        }
    }
}
impl From<RawMedia> for Media {
    fn from(m: RawMedia) -> Self {
        Self {
            key: m.media_key,
            kind: m.kind,
            url: m.url,
            preview_url: m.preview_image_url,
            alt_text: m.alt_text,
            variants: m.variants,
        }
    }
}
impl From<RawList> for XList {
    fn from(l: RawList) -> Self {
        Self {
            id: l.id,
            name: l.name,
            description: l.description,
            member_count: l.member_count,
            follower_count: l.follower_count,
            private: l.private,
        }
    }
}

fn convert_post(
    raw: RawPost,
    users: &HashMap<String, User>,
    media: &HashMap<String, Media>,
    referenced: &HashMap<String, RawPost>,
) -> Post {
    let fallback = User {
        id: raw.author_id.clone().unwrap_or_default(),
        name: "Unknown".into(),
        username: "unknown".into(),
        ..Default::default()
    };
    let author = raw
        .author_id
        .as_ref()
        .and_then(|id| users.get(id))
        .cloned()
        .unwrap_or(fallback);
    let attachments = raw
        .attachments
        .as_ref()
        .map(|a| {
            a.media_keys
                .iter()
                .filter_map(|k| media.get(k).cloned())
                .collect()
        })
        .unwrap_or_default();
    let nested = |kind: &str| {
        raw.referenced_tweets
            .iter()
            .find(|r| r.kind == kind)
            .and_then(|r| referenced.get(&r.id))
            .cloned()
            .map(|p| Box::new(convert_post(p, users, media, &HashMap::new())))
    };
    Post {
        id: raw.id.clone(),
        text: raw.text,
        author,
        created_at: raw.created_at.unwrap_or_else(Utc::now),
        conversation_id: raw.conversation_id.unwrap_or(raw.id),
        in_reply_to_user_id: raw.in_reply_to_user_id,
        metrics: raw.public_metrics,
        media: attachments,
        quoted: nested("quoted"),
        reposted: nested("retweeted"),
        language: raw.lang,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expansion_response_is_normalized_into_post_cards() {
        let json = r#"{
          "data":[{"id":"10","text":"hello","author_id":"u1","created_at":"2026-08-10T00:00:00Z","conversation_id":"10","attachments":{"media_keys":["m1"]},"public_metrics":{"reply_count":1,"retweet_count":2,"like_count":3,"quote_count":0}}],
          "includes":{"users":[{"id":"u1","name":"Ada","username":"ada","verified":true}],"media":[{"media_key":"m1","type":"photo","url":"https://example.test/image.jpg"}]},
          "meta":{"next_token":"next"}
        }"#;
        let response: ApiResponse<Vec<RawPost>> = serde_json::from_str(json).unwrap();
        let page = response.into_posts();
        assert_eq!(page.next_token.as_deref(), Some("next"));
        assert_eq!(page.items[0].author.username, "ada");
        assert!(page.items[0].author.verified);
        assert_eq!(page.items[0].media.len(), 1);
        assert_eq!(page.items[0].metrics.like_count, 3);
    }
}
