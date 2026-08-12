use crate::{api::Api, browser::BrowserSession, model::*};
use anyhow::{Result, bail};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use tokio::sync::{Mutex, RwLock};
use url::Url;

pub struct ScrapeApi {
    browser: Mutex<BrowserSession>,
    me: RwLock<Option<User>>,
    authors: RwLock<HashMap<String, String>>,
}

impl ScrapeApi {
    pub async fn connect() -> Result<Self> {
        Ok(Self {
            browser: Mutex::new(BrowserSession::connect_or_launch(false).await?),
            me: RwLock::new(None),
            authors: RwLock::new(HashMap::new()),
        })
    }

    pub async fn open_login() -> Result<()> {
        let mut browser = BrowserSession::connect_or_launch(true).await?;
        browser.navigate("https://x.com/i/flow/login").await
    }

    async fn page(&self, url: &str) -> Result<Vec<Post>> {
        let mut browser = self.browser.lock().await;
        browser.navigate(url).await?;
        wait_for_timeline(&mut browser).await?;
        let raw: Vec<ScrapedPost> = browser.evaluate(EXTRACT_POSTS_JS).await?;
        let mut seen = HashSet::new();
        let posts: Vec<_> = raw
            .into_iter()
            .filter_map(ScrapedPost::into_post)
            .filter(|post| seen.insert(post.id.clone()))
            .collect();
        drop(browser);
        self.remember(&posts).await;
        Ok(posts)
    }

    async fn current_user(&self) -> Result<User> {
        if let Some(user) = self.me.read().await.clone() {
            return Ok(user);
        }
        let mut browser = self.browser.lock().await;
        browser.navigate("https://x.com/home").await?;
        for _ in 0..40 {
            let links: u64 = browser
                .evaluate("document.querySelectorAll('a[href]').length")
                .await
                .unwrap_or(0);
            if links > 4 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        let raw: ScrapedSelf = browser.evaluate(EXTRACT_SELF_JS).await?;
        if raw.username.is_empty() {
            let links: Vec<String> = browser
                .evaluate("[...document.querySelectorAll('a[href]')].slice(0,30).map(a=>a.getAttribute('href'))")
                .await
                .unwrap_or_default();
            bail!(
                "the XTUI browser profile is not signed in or X did not finish rendering; run `xtui browser-login` (visible links: {})",
                links.join(", ")
            );
        }
        let user = User {
            id: raw.username.clone(),
            name: if raw.name.is_empty() {
                raw.username.clone()
            } else {
                raw.name
            },
            username: raw.username,
            ..Default::default()
        };
        *self.me.write().await = Some(user.clone());
        Ok(user)
    }

    async fn remember(&self, posts: &[Post]) {
        let mut authors = self.authors.write().await;
        if authors.len() > 2048 {
            authors.clear();
        }
        for post in posts {
            authors.insert(post.id.clone(), post.author.username.clone());
        }
    }
}

#[async_trait]
impl Api for ScrapeApi {
    async fn me(&self) -> Result<User> {
        self.current_user().await
    }

    async fn home(&self, next: Option<&str>) -> Result<Page<Post>> {
        if next.is_some() {
            let items = self.scroll_posts().await?;
            return Ok(Page {
                next_token: (!items.is_empty()).then(|| "scroll".into()),
                items,
            });
        }
        let mut browser = self.browser.lock().await;
        let already_home: bool = browser
            .evaluate("location.pathname === '/home'")
            .await
            .unwrap_or(false);
        if !already_home {
            browser.navigate("https://x.com/home").await?;
        }
        wait_for_timeline(&mut browser).await?;
        let _: bool = browser
            .evaluate("window.scrollTo(0,0); true")
            .await
            .unwrap_or(false);
        // XTUI's Home label is Following. Select it explicitly instead of silently showing For You.
        let switched: bool = browser.evaluate(CLICK_FOLLOWING_JS).await.unwrap_or(false);
        if switched {
            tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        }
        let raw: Vec<ScrapedPost> = browser.evaluate(EXTRACT_POSTS_JS).await?;
        let items: Vec<Post> = raw.into_iter().filter_map(ScrapedPost::into_post).collect();
        drop(browser);
        self.remember(&items).await;
        Ok(Page {
            items,
            next_token: Some("scroll".into()),
        })
    }

    async fn search(&self, query: &str, next: Option<&str>) -> Result<Page<Post>> {
        let mut url = Url::parse("https://x.com/search")?;
        url.query_pairs_mut()
            .append_pair("q", query)
            .append_pair("src", "typed_query")
            .append_pair("f", "live");
        let items = if next.is_some() {
            self.scroll_posts().await?
        } else {
            self.page(url.as_str()).await?
        };
        Ok(Page {
            next_token: (!items.is_empty()).then(|| "scroll".into()),
            items,
        })
    }

    async fn thread(&self, conversation_id: &str) -> Result<Vec<Post>> {
        let (previous_url, previous_scroll): (String, f64) = {
            let mut browser = self.browser.lock().await;
            let url = browser.evaluate("location.href").await.unwrap_or_default();
            let scroll = browser.evaluate("window.scrollY").await.unwrap_or(0.0);
            (url, scroll)
        };
        let author = self.authors.read().await.get(conversation_id).cloned();
        let url = author
            .map(|username| format!("https://x.com/{username}/status/{conversation_id}"))
            .unwrap_or_else(|| format!("https://x.com/i/web/status/{conversation_id}"));
        let mut posts = self.page(&url).await?;
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        {
            let mut browser = self.browser.lock().await;
            let settled: Vec<Post> = browser
                .evaluate::<Vec<ScrapedPost>>(EXTRACT_POSTS_JS)
                .await?
                .into_iter()
                .filter_map(ScrapedPost::into_post)
                .collect();
            drop(browser);
            let mut known: HashSet<_> = posts.iter().map(|post| post.id.clone()).collect();
            posts.extend(
                settled
                    .into_iter()
                    .filter(|post| known.insert(post.id.clone())),
            );
            self.remember(&posts).await;
        }
        let mut seen: HashSet<_> = posts.iter().map(|post| post.id.clone()).collect();
        for _ in 0..4 {
            let more = self.scroll_posts().await?;
            if more.is_empty() {
                break;
            }
            posts.extend(more.into_iter().filter(|post| seen.insert(post.id.clone())));
        }
        // Thread pages are by far X's largest renderer state. Return the
        // companion to the originating feed once replies have been extracted;
        // the TUI already owns the thread data and can resume paging the feed.
        if previous_url.starts_with("https://x.com/") {
            let mut browser = self.browser.lock().await;
            browser.recreate_page().await?;
            browser.navigate(&previous_url).await?;
            tokio::time::sleep(std::time::Duration::from_millis(600)).await;
            let expression = format!("window.scrollTo(0,{previous_scroll}); true");
            let _: bool = browser.evaluate(&expression).await.unwrap_or(false);
            browser.collect_garbage().await;
        }
        Ok(posts)
    }

    async fn bookmarks(&self, next: Option<&str>) -> Result<Page<Post>> {
        let items = if next.is_some() {
            self.scroll_posts().await?
        } else {
            self.page("https://x.com/i/bookmarks").await?
        };
        Ok(Page {
            next_token: (!items.is_empty()).then(|| "scroll".into()),
            items,
        })
    }

    async fn likes(&self, user_id: &str, next: Option<&str>) -> Result<Page<Post>> {
        let items = if next.is_some() {
            self.scroll_posts().await?
        } else {
            self.page(&format!(
                "https://x.com/{}/likes",
                user_id.trim_start_matches('@')
            ))
            .await?
        };
        Ok(Page {
            next_token: (!items.is_empty()).then(|| "scroll".into()),
            items,
        })
    }

    async fn mentions(&self, next: Option<&str>) -> Result<Page<Post>> {
        let items = if next.is_some() {
            self.scroll_posts().await?
        } else {
            self.page("https://x.com/notifications/mentions").await?
        };
        Ok(Page {
            next_token: (!items.is_empty()).then(|| "scroll".into()),
            items,
        })
    }

    async fn user_by_username(&self, username: &str) -> Result<User> {
        let username = username.trim_start_matches('@');
        let posts = self.page(&format!("https://x.com/{username}")).await?;
        Ok(posts.first().map(|p| p.author.clone()).unwrap_or(User {
            id: username.into(),
            name: username.into(),
            username: username.into(),
            ..Default::default()
        }))
    }

    async fn user_posts(&self, user_id: &str, next: Option<&str>) -> Result<Page<Post>> {
        let items = if next.is_some() {
            self.scroll_posts().await?
        } else {
            self.page(&format!(
                "https://x.com/{}",
                user_id.trim_start_matches('@')
            ))
            .await?
        };
        Ok(Page {
            next_token: (!items.is_empty()).then(|| "scroll".into()),
            items,
        })
    }

    async fn lists(&self) -> Result<Vec<XList>> {
        let me = self.current_user().await?;
        let mut browser = self.browser.lock().await;
        browser
            .navigate(&format!("https://x.com/{}/lists", me.username))
            .await?;
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        browser.evaluate(EXTRACT_LISTS_JS).await
    }

    async fn list_posts(&self, list_id: &str, next: Option<&str>) -> Result<Page<Post>> {
        let items = if next.is_some() {
            self.scroll_posts().await?
        } else {
            self.page(&format!("https://x.com/i/lists/{list_id}"))
                .await?
        };
        Ok(Page {
            next_token: (!items.is_empty()).then(|| "scroll".into()),
            items,
        })
    }
}

impl ScrapeApi {
    async fn scroll_posts(&self) -> Result<Vec<Post>> {
        let mut browser = self.browser.lock().await;
        let before: HashSet<String> = browser
            .evaluate::<Vec<ScrapedPost>>(EXTRACT_POSTS_JS)
            .await?
            .into_iter()
            .map(|post| post.id)
            .collect();
        let mut found = Vec::new();
        let mut found_ids = HashSet::new();
        let mut idle_rounds = 0;
        browser.activate().await?;
        for _ in 0..10 {
            browser.scroll_down(2200.0).await?;
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let visible: Vec<Post> = browser
                .evaluate::<Vec<ScrapedPost>>(EXTRACT_POSTS_JS)
                .await?
                .into_iter()
                .filter(|post| !before.contains(&post.id))
                .filter_map(ScrapedPost::into_post)
                .collect();

            let old_len = found.len();
            found.extend(
                visible
                    .into_iter()
                    .filter(|post| found_ids.insert(post.id.clone())),
            );
            if found.len() == old_len {
                idle_rounds += 1;
            } else {
                idle_rounds = 0;
            }
            // Once X has yielded a useful page, allow one quiet sample for
            // delayed hydration before returning it to the TUI.
            if !found.is_empty() && idle_rounds >= 1 {
                break;
            }
        }
        drop(browser);
        self.remember(&found).await;
        Ok(found)
    }
}

async fn wait_for_timeline(browser: &mut BrowserSession) -> Result<()> {
    let mut last_count = 0;
    let mut stable_samples = 0;
    for _ in 0..80 {
        let count: u64 = browser
            .evaluate("document.querySelectorAll('article[data-testid=\"tweet\"]').length")
            .await
            .unwrap_or(0);
        if count > 0 {
            if count == last_count {
                stable_samples += 1;
            } else {
                stable_samples = 0;
                last_count = count;
            }
            if stable_samples >= 3 {
                return Ok(());
            }
        }
        let url: String = browser.evaluate("location.href").await.unwrap_or_default();
        if url.contains("/login") {
            bail!("the XTUI browser profile is not signed in; run `xtui browser-login`");
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
    bail!("X did not render any posts; press r to retry")
}

#[derive(Deserialize)]
struct ScrapedSelf {
    #[serde(default)]
    name: String,
    #[serde(default)]
    username: String,
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
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc))
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
            .map(|(i, m)| Media {
                key: format!("{}-{i}", self.id),
                kind: if m.kind == "video" {
                    MediaKind::Video
                } else {
                    MediaKind::Photo
                },
                url: Some(m.url),
                preview_url: None,
                alt_text: m.alt,
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
            quoted: self.quoted.and_then(|q| q.into_post()).map(Box::new),
            reposted: None,
            language: None,
        })
    }
}

const CLICK_FOLLOWING_JS: &str = r#"(()=>{const tab=[...document.querySelectorAll('[role=tab]')].find(e=>e.textContent.trim()==='Following');if(tab&&tab.getAttribute('aria-selected')!=='true'){tab.click();return true}return false})()"#;
const EXTRACT_SELF_JS: &str = r#"(()=>{const p=document.querySelector('a[data-testid="AppTabBar_Profile_Link"]')||document.querySelector('nav[aria-label="Primary"] a[aria-label="Profile"]')||[...document.querySelectorAll('header nav a,header a')].find(a=>/^\/[A-Za-z0-9_]+$/.test(a.getAttribute('href')||'')&&!['home','explore','notifications','messages','i'].includes((a.getAttribute('href')||'').slice(1)));const href=p?.getAttribute('href')||'';const username=href.split('/').filter(Boolean)[0]||'';const name=document.querySelector('[data-testid="SideNav_AccountSwitcher_Button"] img')?.getAttribute('alt')||document.querySelector('button[aria-label="Account menu"] img')?.getAttribute('alt')||username;return {name,username}})()"#;
const EXTRACT_LISTS_JS: &str = r#"(()=>{const seen=new Set;return [...document.querySelectorAll('a[href*="/lists/"]')].map(a=>{const m=a.getAttribute('href').match(/\/lists\/(\d+)/);if(!m||seen.has(m[1]))return null;seen.add(m[1]);const text=a.innerText.trim().split('\n');return {id:m[1],name:text[0]||'List',description:text.slice(1).join(' '),member_count:null,follower_count:null,private:false}}).filter(Boolean)})()"#;
const EXTRACT_POSTS_JS: &str = r#"(()=>{
const number=s=>{const m=(s||'').replaceAll(',','').match(/([0-9.]+)\s*([KMB])?/i);if(!m)return 0;const n=Number(m[1]);return Math.round(n*({K:1e3,M:1e6,B:1e9}[m[2]?.toUpperCase()]||1))};
const one=a=>{const time=a.querySelector('time');const link=time?.closest('a')?.getAttribute('href')||[...a.querySelectorAll('a[href*="/status/"]')].map(x=>x.getAttribute('href')).find(x=>/\/status\/\d+/.test(x))||'';const match=link.match(/\/([^/]+)\/status\/(\d+)/);const user=a.querySelector('[data-testid="User-Name"]');const username=[...user?.querySelectorAll('span')||[]].map(x=>x.textContent.trim()).find(x=>x.startsWith('@'))?.slice(1)||match?.[1]||'';const names=[...user?.querySelectorAll('span')||[]].map(x=>x.textContent.trim()).filter(x=>x&&!x.startsWith('@')&&x!=='·');const text=a.querySelector('[data-testid="tweetText"]')?.innerText||'';const metric=t=>number(a.querySelector(`[data-testid="${t}"]`)?.getAttribute('aria-label'));const media=[...a.querySelectorAll('[data-testid="tweetPhoto"] img')].map(img=>({kind:'photo',url:img.currentSrc||img.src,alt:img.alt||null}));const video=a.querySelector('video');if(video?.poster)media.push({kind:'video',url:video.poster,alt:'Video preview'});const quote=a.querySelector('[data-testid="quoteTweet"]');return {id:match?.[2]||'',text,name:names[0]||username,username,verified:!!user?.querySelector('[data-testid="icon-verified"],img[alt="Verified account"],svg[aria-label="Verified account"]'),created_at:time?.dateTime||null,replies:metric('reply'),reposts:metric('retweet'),likes:metric('like'),views:number(a.querySelector('a[href$="/analytics"]')?.getAttribute('aria-label')),media,quoted:quote?one(quote):null}};
return [...document.querySelectorAll('article[data-testid="tweet"]')].map(one).filter(x=>x.id&&x.username)
})()"#;

#[cfg(test)]
mod tests {
    use super::*;
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
