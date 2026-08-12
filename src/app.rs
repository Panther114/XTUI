use crate::{api::Api, media, model::*};
use std::{collections::VecDeque, sync::Arc};

const MAX_RETAINED_POSTS: usize = 400;

#[derive(Clone, Debug)]
pub enum Screen {
    Home,
    Explore,
    Mentions,
    Bookmarks,
    Lists,
    ListFeed(Box<XList>),
    Profile(Box<User>),
    Likes(Box<User>),
    Thread(Box<Post>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Search,
}

pub struct ViewState {
    screen: Screen,
    posts: Vec<Post>,
    lists: Vec<XList>,
    selected: usize,
    mode: InputMode,
    query: String,
    search_cursor: usize,
    status: String,
    error: Option<String>,
    thread_expanded: bool,
    next_token: Option<String>,
}

pub struct App {
    pub api: Arc<dyn Api>,
    pub screen: Screen,
    pub history: Vec<ViewState>,
    pub posts: Vec<Post>,
    pub lists: Vec<XList>,
    pub selected: usize,
    pub me: Option<User>,
    pub mode: InputMode,
    pub query: String,
    pub search_cursor: usize,
    pub status: String,
    pub error: Option<String>,
    pub help: bool,
    pub demo: bool,
    pub browser_mode: bool,
    pub should_quit: bool,
    pub thread_expanded: bool,
    pub media_preview: Option<(String, Vec<String>)>,
    pub next_token: Option<String>,
    pub nav_focused: bool,
    pub nav_selected: usize,
    media_cache: VecDeque<(String, Vec<String>)>,
}

impl App {
    pub fn new(api: Arc<dyn Api>, demo: bool) -> Self {
        Self {
            api,
            screen: Screen::Home,
            history: vec![],
            posts: vec![],
            lists: vec![],
            selected: 0,
            me: None,
            mode: InputMode::Normal,
            query: String::new(),
            search_cursor: 0,
            status: "Loading your timeline…".into(),
            error: None,
            help: false,
            demo,
            browser_mode: false,
            should_quit: false,
            thread_expanded: true,
            media_preview: None,
            next_token: None,
            nav_focused: false,
            nav_selected: 0,
            media_cache: VecDeque::new(),
        }
    }

    pub async fn bootstrap(&mut self) {
        self.me = self.api.me().await.map_err(|e| self.set_error(e)).ok();
        self.load_screen().await;
    }

    pub fn with_browser_mode(mut self) -> Self {
        self.browser_mode = true;
        self
    }

    pub fn selected_post(&self) -> Option<&Post> {
        self.posts.get(self.selected)
    }
    pub fn selected_list(&self) -> Option<&XList> {
        self.lists.get(self.selected)
    }

    /// The screen at the top of the back stack, or the current screen when at a
    /// root. Threads, profiles, and likes are sub-screens of a section; the UI
    /// uses this to keep the section rail highlighting stable inside them.
    pub fn origin_screen(&self) -> &Screen {
        self.history
            .last()
            .map(|view| &view.screen)
            .unwrap_or(&self.screen)
    }

    /// The rail section that `origin_screen` belongs to (0..=4).
    pub fn nav_index(&self) -> usize {
        match self.origin_screen() {
            Screen::Home => 0,
            Screen::Explore => 1,
            Screen::Mentions => 2,
            Screen::Bookmarks => 3,
            Screen::Lists | Screen::ListFeed(_) => 4,
            _ => 0,
        }
    }

    /// The rail item at `index` maps to this screen.
    pub fn nav_screen(index: usize) -> Screen {
        match index {
            0 => Screen::Home,
            1 => Screen::Explore,
            2 => Screen::Mentions,
            3 => Screen::Bookmarks,
            _ => Screen::Lists,
        }
    }

    /// Enter or leave sidebar navigation. While focused, the left rail owns the
    /// arrow keys and Enter switches sections.
    pub fn toggle_nav_focus(&mut self) {
        self.nav_focused = !self.nav_focused;
        if self.nav_focused {
            self.nav_selected = self.nav_index();
            self.status = "Sidebar · ↑↓ choose · → open · Esc back".into();
        }
    }

    pub fn nav_move(&mut self, delta: isize) {
        self.nav_selected = (self.nav_selected as isize + delta).clamp(0, 4) as usize;
    }

    pub async fn nav_activate(&mut self) {
        let screen = Self::nav_screen(self.nav_selected);
        self.nav_focused = false;
        self.root(screen).await;
    }

    pub fn move_selection(&mut self, delta: isize) {
        let len = if matches!(self.screen, Screen::Lists) {
            self.lists.len()
        } else if matches!(self.screen, Screen::Thread(_)) && !self.thread_expanded {
            1
        } else {
            self.posts.len()
        };
        if len == 0 {
            self.selected = 0;
            return;
        }
        self.selected = (self.selected as isize + delta).clamp(0, len as isize - 1) as usize;
    }

    pub async fn advance(&mut self, delta: isize) {
        if self.will_load_more(delta) {
            self.load_more().await;
        }
        self.move_selection(delta);
    }

    /// Whether a forward move of `delta` would need another page from the API.
    /// Shared by the UI so it can show a busy state before the fetch blocks.
    pub fn will_load_more(&self, delta: isize) -> bool {
        delta > 0
            && !matches!(self.screen, Screen::Lists | Screen::Thread(_))
            && self.next_token.is_some()
            && self.selected.saturating_add(delta as usize) >= self.posts.len().saturating_sub(2)
    }

    pub fn begin_search(&mut self) {
        if !matches!(self.screen, Screen::Explore) {
            self.push_view(Screen::Explore);
        }
        self.mode = InputMode::Search;
        self.query.clear();
        self.search_cursor = 0;
        self.posts.clear();
        self.selected = 0;
        self.error = None;
        self.status = "Type a search and press Enter".into();
    }

    pub fn move_search_cursor(&mut self, delta: isize) {
        let length = self.query.chars().count();
        self.search_cursor =
            (self.search_cursor as isize + delta).clamp(0, length as isize) as usize;
    }

    pub fn insert_query_char(&mut self, character: char) {
        let byte = self
            .query
            .char_indices()
            .nth(self.search_cursor)
            .map(|(index, _)| index)
            .unwrap_or(self.query.len());
        self.query.insert(byte, character);
        self.search_cursor += 1;
    }

    pub fn backspace_query(&mut self) {
        if self.search_cursor == 0 {
            return;
        }
        let start = self
            .query
            .char_indices()
            .nth(self.search_cursor - 1)
            .map(|(index, _)| index)
            .unwrap_or(0);
        let end = self
            .query
            .char_indices()
            .nth(self.search_cursor)
            .map(|(index, _)| index)
            .unwrap_or(self.query.len());
        self.query.replace_range(start..end, "");
        self.search_cursor -= 1;
    }

    pub fn clear_query(&mut self) {
        self.query.clear();
        self.search_cursor = 0;
    }

    pub fn search_display(&self) -> String {
        let mut display = String::with_capacity(self.query.len() + 3);
        for (index, character) in self.query.chars().enumerate() {
            if index == self.search_cursor {
                display.push('▌');
            }
            display.push(character);
        }
        if self.search_cursor == self.query.chars().count() {
            display.push('▌');
        }
        display
    }

    pub async fn navigate(&mut self, screen: Screen) {
        self.push_view(screen);
        self.load_screen().await;
    }

    pub async fn root(&mut self, screen: Screen) {
        self.history.clear();
        self.screen = screen;
        self.posts.clear();
        self.lists.clear();
        self.selected = 0;
        self.error = None;
        self.load_screen().await;
    }

    pub fn back(&mut self) {
        if self.media_preview.take().is_some() || self.help {
            self.help = false;
            return;
        }
        if let Some(view) = self.history.pop() {
            self.screen = view.screen;
            self.posts = view.posts;
            self.lists = view.lists;
            self.selected = view.selected;
            self.mode = view.mode;
            self.query = view.query;
            self.search_cursor = view.search_cursor;
            self.status = view.status;
            self.error = view.error;
            self.thread_expanded = view.thread_expanded;
            self.next_token = view.next_token;
        } else {
            self.status = "Already at the top level · press Q to quit".into();
        }
    }

    fn push_view(&mut self, screen: Screen) {
        let previous = ViewState {
            screen: std::mem::replace(&mut self.screen, screen),
            posts: std::mem::take(&mut self.posts),
            lists: std::mem::take(&mut self.lists),
            selected: std::mem::take(&mut self.selected),
            mode: std::mem::replace(&mut self.mode, InputMode::Normal),
            query: std::mem::take(&mut self.query),
            search_cursor: std::mem::take(&mut self.search_cursor),
            status: std::mem::take(&mut self.status),
            error: self.error.take(),
            thread_expanded: std::mem::replace(&mut self.thread_expanded, true),
            next_token: self.next_token.take(),
        };
        if self.history.len() == 16 {
            self.history.remove(0);
        }
        self.history.push(previous);
    }

    pub async fn activate(&mut self) {
        match &self.screen {
            Screen::Lists => {
                if let Some(list) = self.selected_list().cloned() {
                    self.navigate(Screen::ListFeed(Box::new(list))).await
                }
            }
            _ => {
                if let Some(post) = self.selected_post().cloned() {
                    self.navigate(Screen::Thread(Box::new(post))).await
                }
            }
        }
    }

    pub async fn open_profile(&mut self) {
        if let Some(user) = self
            .selected_post()
            .map(|p| p.author.clone())
            .or_else(|| self.me.clone())
        {
            self.navigate(Screen::Profile(Box::new(user))).await;
        }
    }

    pub async fn open_likes(&mut self) {
        let user = match &self.screen {
            Screen::Profile(u) => Some((**u).clone()),
            _ => self.selected_post().map(|p| p.author.clone()),
        };
        if let Some(user) = user {
            self.navigate(Screen::Likes(Box::new(user))).await;
        }
    }

    pub async fn submit_search(&mut self) {
        self.mode = InputMode::Normal;
        let query = self.query.trim().to_owned();
        if query.is_empty() {
            return;
        }
        self.query = query.clone();
        self.screen = Screen::Explore;
        self.selected = 0;
        self.error = None;
        self.status = format!("Searching for “{query}”…");
        match self.api.search(&query, None).await {
            Ok(page) => {
                self.posts = page.items;
                self.next_token = page.next_token;
                self.status = format!("{} results", self.posts.len());
            }
            Err(e) => self.set_error(e),
        }
    }

    pub async fn refresh(&mut self) {
        self.error = None;
        self.load_screen().await;
    }

    pub async fn load_more(&mut self) {
        self.status = "Loading more posts…".into();
        let Some(token) = self.next_token.clone() else {
            self.status = "You’re all caught up.".into();
            return;
        };
        let result = match &self.screen {
            Screen::Home => self.api.home(Some(&token)).await,
            Screen::Explore => self.api.search(self.query.trim(), Some(&token)).await,
            Screen::Mentions => self.api.mentions(Some(&token)).await,
            Screen::Bookmarks => self.api.bookmarks(Some(&token)).await,
            Screen::ListFeed(list) => self.api.list_posts(&list.id, Some(&token)).await,
            Screen::Profile(user) => self.api.user_posts(&user.id, Some(&token)).await,
            Screen::Likes(user) => self.api.likes(&user.id, Some(&token)).await,
            Screen::Lists | Screen::Thread(_) => {
                self.status = "No more items are available on this screen.".into();
                return;
            }
        };
        match result {
            Ok(page) => {
                let fresh: Vec<_> = {
                    let existing: std::collections::HashSet<_> =
                        self.posts.iter().map(|post| post.id.as_str()).collect();
                    page.items
                        .into_iter()
                        .filter(|post| !existing.contains(post.id.as_str()))
                        .collect()
                };
                let added = fresh.len();
                self.posts.extend(fresh);
                self.trim_feed();
                self.next_token = if added == 0 { None } else { page.next_token };
                self.status = if added == 0 {
                    "You’re all caught up.".into()
                } else {
                    format!("Loaded {added} more posts")
                };
            }
            Err(error) => self.set_error(error),
        }
    }

    pub async fn preview_media(&mut self) {
        let Some(item) = self.selected_post().and_then(|p| p.media.first()).cloned() else {
            self.status = "This post has no attached media.".into();
            return;
        };
        let url = item.preview_url.as_ref().or(item.url.as_ref()).cloned();
        let Some(url) = url else {
            self.status = "No preview is available for this media.".into();
            return;
        };
        if let Some((_, lines)) = self.media_cache.iter().find(|(key, _)| key == &url) {
            self.media_preview = Some((
                item.alt_text.unwrap_or_else(|| format!("{:?}", item.kind)),
                lines.clone(),
            ));
            self.status = "Media preview · cached".into();
            return;
        }
        self.status = "Rendering media preview…".into();
        match media::download_preview(&url, 68, 24).await {
            Ok(lines) => {
                self.media_cache.push_front((url, lines.clone()));
                self.media_cache.truncate(8);
                self.media_preview = Some((
                    item.alt_text.unwrap_or_else(|| format!("{:?}", item.kind)),
                    lines,
                ))
            }
            Err(e) => self.set_error(e),
        }
    }

    pub fn open_external(&mut self, media_only: bool) {
        let Some(post) = self.selected_post() else {
            return;
        };
        let url = if media_only {
            post.media
                .first()
                .and_then(media::best_external_url)
                .unwrap_or_else(|| post.permalink())
        } else {
            post.permalink()
        };
        if let Err(e) = open::that(&url) {
            self.error = Some(format!("Could not open browser: {e}"));
        }
    }

    async fn load_screen(&mut self) {
        self.status = "Loading…".into();
        self.next_token = None;
        let result = match &self.screen {
            Screen::Home => self.api.home(None).await.map(|p| {
                self.posts = p.items;
                self.next_token = p.next_token;
            }),
            Screen::Explore => {
                self.mode = InputMode::Search;
                self.query.clear();
                self.search_cursor = 0;
                self.posts.clear();
                Ok(())
            }
            Screen::Mentions => self.api.mentions(None).await.map(|p| {
                self.posts = p.items;
                self.next_token = p.next_token;
            }),
            Screen::Bookmarks => self.api.bookmarks(None).await.map(|p| {
                self.posts = p.items;
                self.next_token = p.next_token;
            }),
            Screen::Lists => self.api.lists().await.map(|l| {
                self.lists = l;
            }),
            Screen::ListFeed(list) => self.api.list_posts(&list.id, None).await.map(|p| {
                self.posts = p.items;
                self.next_token = p.next_token;
            }),
            Screen::Profile(user) => self.api.user_posts(&user.id, None).await.map(|p| {
                self.posts = p.items;
                self.next_token = p.next_token;
            }),
            Screen::Likes(user) => self.api.likes(&user.id, None).await.map(|p| {
                self.posts = p.items;
                self.next_token = p.next_token;
            }),
            Screen::Thread(post) => {
                self.api
                    .thread(&post.conversation_id)
                    .await
                    .map(|mut posts| {
                        if !posts.iter().any(|p| p.id == post.id) {
                            posts.insert(0, (**post).clone());
                        }
                        self.posts = posts;
                        self.thread_expanded = true;
                    })
            }
        };
        match result {
            Ok(()) => {
                self.status = if self.posts.is_empty()
                    && !matches!(self.screen, Screen::Lists | Screen::Explore)
                {
                    "Nothing here yet.".into()
                } else if self.demo {
                    "DEMO · no network or account required".into()
                } else if self.browser_mode {
                    "LIVE · browser companion · no API credits".into()
                } else {
                    "Live · X API".into()
                }
            }
            Err(e) => self.set_error(e),
        }
    }

    fn set_error(&mut self, error: impl std::fmt::Display) {
        self.error = Some(error.to_string());
        self.status = "Request failed — press r to retry".into();
    }

    fn trim_feed(&mut self) {
        let excess = self.posts.len().saturating_sub(MAX_RETAINED_POSTS);
        if excess > 0 {
            self.posts.drain(..excess);
            self.selected = self.selected.saturating_sub(excess);
        }
    }

    pub fn title(&self) -> String {
        match &self.screen {
            Screen::Home => "Home".into(),
            Screen::Explore => "Explore".into(),
            Screen::Mentions => "Mentions".into(),
            Screen::Bookmarks => "Bookmarks".into(),
            Screen::Lists => "Lists".into(),
            Screen::ListFeed(l) => l.name.clone(),
            Screen::Profile(u) => u.name.clone(),
            Screen::Likes(u) => format!("Posts liked by {}", u.name),
            Screen::Thread(_) => "Post".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::DemoApi;
    #[tokio::test]
    async fn navigation_preserves_a_back_stack() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap().await;
        app.selected = 1;
        app.query = "remember me".into();
        app.next_token = Some("cursor".into());
        let original_ids: Vec<_> = app.posts.iter().map(|post| post.id.clone()).collect();
        let selected = app.selected_post().unwrap().clone();
        app.navigate(Screen::Thread(Box::new(selected))).await;
        assert_eq!(app.title(), "Post");
        assert!(app.posts.len() > 1);
        app.back();
        assert_eq!(app.title(), "Home");
        assert_eq!(app.selected, 1);
        assert_eq!(app.query, "remember me");
        assert_eq!(app.next_token.as_deref(), Some("cursor"));
        assert_eq!(
            app.posts
                .iter()
                .map(|post| post.id.clone())
                .collect::<Vec<_>>(),
            original_ids
        );
    }
    #[tokio::test]
    async fn roots_show_only_mother_posts_until_opened() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap().await;
        assert!(app.posts.iter().all(|p| p.in_reply_to_user_id.is_none()));
        app.activate().await;
        assert!(
            app.posts
                .iter()
                .skip(1)
                .any(|p| p.in_reply_to_user_id.is_some())
        );
        app.thread_expanded = false;
        app.move_selection(10);
        assert_eq!(app.selected, 0);
    }

    #[tokio::test]
    async fn search_entry_is_visible_and_preserves_back_navigation() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap().await;
        app.begin_search();
        assert!(matches!(app.screen, Screen::Explore));
        assert_eq!(app.mode, InputMode::Search);
        assert!(matches!(
            app.history.last(),
            Some(view) if matches!(view.screen, Screen::Home)
        ));
    }

    #[tokio::test]
    async fn back_at_a_root_does_not_quit() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap().await;
        app.back();
        assert!(!app.should_quit);
        assert!(app.status.contains("top level"));
    }

    #[test]
    fn search_cursor_supports_arrow_editing_and_unicode() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.insert_query_char('a');
        app.insert_query_char('界');
        app.move_search_cursor(-1);
        app.insert_query_char('b');
        assert_eq!(app.query, "ab界");
        assert_eq!(app.search_display(), "ab▌界");
        app.backspace_query();
        assert_eq!(app.query, "a界");
        app.clear_query();
        assert_eq!(app.query, "");
        assert_eq!(app.search_cursor, 0);
    }

    #[tokio::test]
    async fn advancing_near_the_end_requests_and_deduplicates_more_posts() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap().await;
        let original = app.posts.len();
        app.next_token = Some("next".into());
        app.selected = original.saturating_sub(2);
        app.advance(1).await;
        assert_eq!(app.posts.len(), original);
        assert_eq!(app.status, "You’re all caught up.");
    }

    #[tokio::test]
    async fn will_load_more_is_true_only_near_the_end_of_a_pageable_feed() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap().await;
        assert!(!app.will_load_more(1), "no next page, no fetch");
        app.next_token = Some("next".into());
        app.selected = 0;
        assert!(!app.will_load_more(1));
        app.selected = app.posts.len().saturating_sub(2);
        assert!(app.will_load_more(1));
        app.root(Screen::Lists).await;
        assert!(!app.will_load_more(1), "lists never paginate");
        app.activate().await;
        assert!(
            !app.will_load_more(1),
            "threads never paginate through the feed"
        );
    }

    #[tokio::test]
    async fn submitted_search_is_trimmed() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap().await;
        app.begin_search();
        for character in "  terminal  ".chars() {
            app.insert_query_char(character);
        }
        app.submit_search().await;
        assert_eq!(app.query, "terminal");
        assert!(!app.posts.is_empty());
    }

    #[tokio::test]
    async fn sidebar_focus_moves_with_arrows_and_activates_sections() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap().await;
        app.toggle_nav_focus();
        assert!(app.nav_focused);
        assert_eq!(app.nav_selected, 0, "focus starts on the current section");
        app.nav_move(2);
        assert_eq!(app.nav_selected, 2);
        app.nav_activate().await;
        assert!(!app.nav_focused, "activating leaves the sidebar");
        assert!(matches!(app.screen, Screen::Mentions));
        app.toggle_nav_focus();
        app.nav_move(5);
        assert_eq!(app.nav_selected, 4, "rail clamps at Lists");
        app.nav_move(-9);
        assert_eq!(app.nav_selected, 0, "rail clamps at Home");
    }
}
