use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct User {
    pub id: String,
    pub name: String,
    pub username: String,
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub description: String,
    pub profile_image_url: Option<String>,
    pub public_metrics: Option<UserMetrics>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct UserMetrics {
    pub followers_count: u64,
    pub following_count: u64,
    pub tweet_count: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct PostMetrics {
    pub reply_count: u64,
    pub retweet_count: u64,
    pub like_count: u64,
    pub quote_count: u64,
    pub bookmark_count: Option<u64>,
    pub impression_count: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Photo,
    Video,
    AnimatedGif,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Media {
    pub key: String,
    pub kind: MediaKind,
    pub url: Option<String>,
    pub preview_url: Option<String>,
    pub alt_text: Option<String>,
    #[serde(default)]
    pub variants: Vec<MediaVariant>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct MediaVariant {
    pub url: String,
    pub bit_rate: Option<u64>,
    pub content_type: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Post {
    pub id: String,
    pub text: String,
    pub author: User,
    pub created_at: DateTime<Utc>,
    pub conversation_id: String,
    pub in_reply_to_user_id: Option<String>,
    #[serde(default)]
    pub metrics: PostMetrics,
    #[serde(default)]
    pub media: Vec<Media>,
    pub quoted: Option<Box<Post>>,
    pub reposted: Option<Box<Post>>,
    pub language: Option<String>,
}

impl Post {
    pub fn permalink(&self) -> String {
        format!("https://x.com/{}/status/{}", self.author.username, self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn permalink_uses_author_and_id() {
        let post = Post {
            id: "42".into(),
            author: User {
                username: "ada".into(),
                ..Default::default()
            },
            text: String::new(),
            created_at: Utc::now(),
            conversation_id: "42".into(),
            in_reply_to_user_id: None,
            metrics: PostMetrics::default(),
            media: vec![],
            quoted: None,
            reposted: None,
            language: None,
        };
        assert_eq!(post.permalink(), "https://x.com/ada/status/42");
    }
}

#[derive(Clone, Debug, Default)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct XList {
    pub id: String,
    pub name: String,
    pub description: String,
    pub member_count: Option<u64>,
    pub follower_count: Option<u64>,
    pub private: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FeedKind {
    #[default]
    Following,
    ForYou,
}
