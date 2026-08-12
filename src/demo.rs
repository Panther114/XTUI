use crate::{api::Api, model::*};
use anyhow::{Result, bail};
use async_trait::async_trait;
use chrono::{Duration, TimeZone, Utc};

pub struct DemoApi {
    posts: Vec<Post>,
    me: User,
}

impl DemoApi {
    pub fn new() -> Self {
        let me = user(
            "0",
            "Terminal Human",
            "terminalhuman",
            "Building quieter software, one keystroke at a time.",
            284,
            311,
        );
        let ada = user(
            "1",
            "Ada",
            "ada_codes",
            "Systems, type theory, and good coffee.",
            48200,
            391,
        );
        let kai = user(
            "2",
            "Kai Nakamura",
            "kainaka",
            "Designer. I make complicated things feel inevitable.",
            126000,
            744,
        );
        let orbital = user(
            "3",
            "Orbital",
            "orbital",
            "Earth, from a little higher up.",
            9300000,
            42,
        );
        let sam = user(
            "4",
            "Sam Rivera",
            "sam_builds",
            "Indie developer. Shipping in public.",
            8900,
            512,
        );
        let base = Utc.with_ymd_and_hms(2026, 8, 10, 6, 0, 0).unwrap();

        let quote = post(
            "91",
            "The best interface is the one that leaves room for the thing you came to read.",
            kai.clone(),
            base - Duration::hours(4),
            214,
            3100,
            18700,
        );
        let mut p1 = post(
            "100",
            "I replaced my morning browser scroll with a terminal feed for a week. The surprise wasn't the speed — it was how much calmer plain text made everything feel.",
            ada.clone(),
            base,
            38,
            226,
            2401,
        );
        p1.media.push(Media {
            key: "m1".into(),
            kind: MediaKind::Photo,
            url: Some("https://images.unsplash.com/photo-1516321318423-f06f85e504b3?w=1200".into()),
            preview_url: Some("demo://terminal-workspace".into()),
            alt_text: Some("A laptop showing a dark terminal beside a cup of coffee".into()),
            variants: vec![],
        });
        let mut p2 = post(
            "101",
            "A tiny design rule that keeps paying rent:\n\nIf secondary information can be revealed on intent, don't make everyone parse it by default.",
            kai.clone(),
            base - Duration::minutes(17),
            91,
            804,
            9300,
        );
        p2.quoted = Some(Box::new(quote));
        let p3 = post(
            "102",
            "Sunrise crossing the terminator over the Pacific. No filter, just an absurdly beautiful planet.",
            orbital.clone(),
            base - Duration::minutes(43),
            1200,
            8600,
            79000,
        );
        let p4 = post(
            "103",
            "shipped: keyboard-only navigation, offline cache, and image previews that degrade gracefully all the way down to Unicode blocks.\n\nThe boring compatibility work is usually the feature.",
            sam.clone(),
            base - Duration::hours(1),
            46,
            119,
            1800,
        );
        let mut p5 = post(
            "104",
            "What are you building this week?",
            ada.clone(),
            base - Duration::hours(2),
            412,
            97,
            1200,
        );
        p5.reposted = Some(Box::new(post(
            "88",
            "Public roadmaps are useful. Public momentum is better.",
            sam.clone(),
            base - Duration::days(1),
            62,
            344,
            4000,
        )));
        let p6 = post(
            "105",
            "Hot take: software can be both powerful and quiet.",
            kai.clone(),
            base - Duration::hours(3),
            188,
            1600,
            20100,
        );

        Self {
            posts: vec![p1, p2, p3, p4, p5, p6],
            me,
        }
    }

    fn replies(&self, id: &str) -> Vec<Post> {
        let root = self.posts.iter().find(|p| p.id == id).cloned();
        let Some(root) = root else { return vec![] };
        let base = root.created_at;
        let mut a = post(
            &format!("{id}1"),
            "This is exactly it. Removing the sidebars changed what I actually chose to read.",
            user("5", "Mina", "minamakes", "Product engineer", 3200, 201),
            base + Duration::minutes(8),
            4,
            9,
            138,
        );
        a.conversation_id = root.conversation_id.clone();
        a.in_reply_to_user_id = Some(root.author.id.clone());
        let mut b = post(
            &format!("{id}2"),
            "How are you handling media? That's the part that always sends terminal clients back to the browser.",
            user("6", "Drew", "drewcli", "Terminal enthusiast", 760, 180),
            base + Duration::minutes(14),
            7,
            3,
            64,
        );
        b.conversation_id = root.conversation_id.clone();
        b.in_reply_to_user_id = Some(root.author.id.clone());
        let mut c = post(
            &format!("{id}3"),
            "Unicode preview in every terminal, native protocols when supported, and one key to open the original. Graceful layers.",
            root.author.clone(),
            base + Duration::minutes(21),
            3,
            8,
            91,
        );
        c.conversation_id = root.conversation_id.clone();
        c.in_reply_to_user_id = Some(b.author.id.clone());
        vec![root, a, b, c]
    }
}
impl Default for DemoApi {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Api for DemoApi {
    async fn me(&self) -> Result<User> {
        Ok(self.me.clone())
    }
    async fn home(&self, _: Option<&str>) -> Result<Page<Post>> {
        Ok(Page {
            items: self.posts.clone(),
            next_token: None,
        })
    }
    async fn search(&self, query: &str, _: Option<&str>) -> Result<Page<Post>> {
        let q = query.to_lowercase();
        let items = self
            .posts
            .iter()
            .filter(|p| {
                p.text.to_lowercase().contains(&q)
                    || p.author.name.to_lowercase().contains(&q)
                    || p.author.username.to_lowercase().contains(&q)
            })
            .cloned()
            .collect();
        Ok(Page {
            items,
            next_token: None,
        })
    }
    async fn thread(&self, id: &str) -> Result<Vec<Post>> {
        Ok(self.replies(id))
    }
    async fn bookmarks(&self, _: Option<&str>) -> Result<Page<Post>> {
        Ok(Page {
            items: self.posts.iter().skip(1).step_by(2).cloned().collect(),
            next_token: None,
        })
    }
    async fn likes(&self, _: &str, _: Option<&str>) -> Result<Page<Post>> {
        Ok(Page {
            items: self.posts.iter().rev().take(4).cloned().collect(),
            next_token: None,
        })
    }
    async fn mentions(&self, _: Option<&str>) -> Result<Page<Post>> {
        Ok(Page {
            items: self.replies("100").into_iter().skip(1).collect(),
            next_token: None,
        })
    }
    async fn user_by_username(&self, username: &str) -> Result<User> {
        self.posts
            .iter()
            .map(|p| &p.author)
            .find(|u| {
                u.username
                    .eq_ignore_ascii_case(username.trim_start_matches('@'))
            })
            .cloned()
            .or_else(|| (self.me.username == username).then(|| self.me.clone()))
            .ok_or_else(|| anyhow::anyhow!("user not found"))
    }
    async fn user_posts(&self, user_id: &str, _: Option<&str>) -> Result<Page<Post>> {
        Ok(Page {
            items: self
                .posts
                .iter()
                .filter(|p| p.author.id == user_id)
                .cloned()
                .collect(),
            next_token: None,
        })
    }
    async fn lists(&self) -> Result<Vec<XList>> {
        Ok(vec![
            XList {
                id: "l1".into(),
                name: "Design & craft".into(),
                description: "People sweating the details.".into(),
                member_count: Some(42),
                follower_count: Some(812),
                private: false,
            },
            XList {
                id: "l2".into(),
                name: "Terminal people".into(),
                description: "CLIs, TUIs, shells and systems.".into(),
                member_count: Some(28),
                follower_count: Some(309),
                private: false,
            },
        ])
    }
    async fn list_posts(&self, list_id: &str, _: Option<&str>) -> Result<Page<Post>> {
        if !["l1", "l2"].contains(&list_id) {
            bail!("list not found");
        }
        Ok(Page {
            items: self
                .posts
                .iter()
                .skip(if list_id == "l1" { 1 } else { 0 })
                .step_by(2)
                .cloned()
                .collect(),
            next_token: None,
        })
    }
}

fn user(
    id: &str,
    name: &str,
    username: &str,
    description: &str,
    followers: u64,
    following: u64,
) -> User {
    User {
        id: id.into(),
        name: name.into(),
        username: username.into(),
        verified: followers > 100_000,
        description: description.into(),
        profile_image_url: None,
        public_metrics: Some(UserMetrics {
            followers_count: followers,
            following_count: following,
            tweet_count: 1240,
        }),
    }
}
fn post(
    id: &str,
    text: &str,
    author: User,
    created_at: chrono::DateTime<Utc>,
    replies: u64,
    reposts: u64,
    likes: u64,
) -> Post {
    Post {
        id: id.into(),
        text: text.into(),
        author,
        created_at,
        conversation_id: id.into(),
        in_reply_to_user_id: None,
        metrics: PostMetrics {
            reply_count: replies,
            retweet_count: reposts,
            like_count: likes,
            quote_count: 0,
            bookmark_count: Some(likes / 9),
            impression_count: Some(likes * 21),
        },
        media: vec![],
        quoted: None,
        reposted: None,
        language: Some("en".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn demo_supports_core_browsing_flows() {
        let api = DemoApi::new();
        assert!(api.home(None).await.unwrap().items.len() >= 6);
        assert!(!api.search("terminal", None).await.unwrap().items.is_empty());
        let thread = api.thread("100").await.unwrap();
        assert_eq!(thread.first().unwrap().id, "100");
        assert!(thread.len() > 1);
        assert_eq!(api.lists().await.unwrap().len(), 2);
    }
}
