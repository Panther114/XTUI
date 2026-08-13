use crate::{api::Api, config::Config, keys::KeyBindings, media, model::*};
use anyhow::{Result, bail};
use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

const MAX_RETAINED_POSTS: usize = 800;
pub(crate) const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
type ExternalOpener = dyn Fn(&str) -> Result<()> + Send + Sync;

#[derive(Clone, Debug)]
pub enum Screen {
    Landing,
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

/// What a landing-page menu entry does when activated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LandingAction {
    Start,
    SignIn,
    Verify,
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitAction {
    Landing(usize),
    Nav(usize),
    Card(usize),
    Back,
    Search,
    ToggleFeed,
    Help,
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HitRegion {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub action: HitAction,
}

impl HitRegion {
    pub fn contains(self, column: u16, row: u16) -> bool {
        column >= self.x
            && column < self.x.saturating_add(self.width)
            && row >= self.y
            && row < self.y.saturating_add(self.height)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Search,
}

/// What a screen load produced. The UI applies one of these to the feed.
#[derive(Clone, Debug)]
pub enum ScreenPayload {
    Page(Page<Post>),
    Thread(Vec<Post>),
    Lists(Vec<XList>),
}

/// Results from background fetch tasks. Every request carries a generation
/// counter so a slow response can never overwrite newer screen state.
#[derive(Debug)]
pub enum UiMsg {
    Bootstrap {
        me: Result<User>,
    },
    ScreenLoaded {
        generation: u32,
        silent: bool,
        result: Result<ScreenPayload>,
    },
    SearchDone {
        generation: u32,
        result: Result<Page<Post>>,
    },
    MoreLoaded {
        generation: u32,
        silent: bool,
        result: Result<Page<Post>>,
    },
    MediaDone {
        url: String,
        open: bool,
        counted: bool,
        result: Result<media::PreviewImage>,
    },
    SignInOpened {
        result: Result<()>,
    },
    SignInChecked {
        result: Result<User>,
    },
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
    body_scroll: usize,
}

#[derive(Clone)]
struct HomeCache {
    feed: FeedKind,
    posts: Vec<Post>,
    selected: usize,
    status: String,
    error: Option<String>,
    next_token: Option<String>,
    body_scroll: usize,
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
    pub media_preview: Option<(String, media::PreviewImage)>,
    pub next_token: Option<String>,
    pub body_scroll: usize,
    pub body_scroll_max: usize,
    pub nav_focused: bool,
    pub nav_selected: usize,
    pub feed_kind: FeedKind,
    pub keys: KeyBindings,
    pub auto_refresh_secs: u64,
    pub landing_selected: usize,
    pub login_pending: bool,
    /// (top row, bottom row, post/list index) for the cards painted on the
    /// last frame; the mouse click handler maps rows back to selections.
    pub card_rows: Vec<(u16, u16, usize)>,
    pub hit_regions: Vec<HitRegion>,
    pub hovered: Option<HitAction>,
    tx: mpsc::UnboundedSender<UiMsg>,
    rx: mpsc::UnboundedReceiver<UiMsg>,
    pending_ops: usize,
    pending_since: Option<Instant>,
    generation: u32,
    media_cache: VecDeque<(String, media::PreviewImage)>,
    media_inflight: HashSet<String>,
    open_media_on_arrival: HashSet<String>,
    pub image_engine: Option<media::ImageEngine>,
    external_opener: Arc<ExternalOpener>,
    next_background_fetch: Instant,
    thread_sync_attempts: usize,
    home_cache: Option<HomeCache>,
}

impl App {
    pub fn new(api: Arc<dyn Api>, demo: bool) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            api,
            screen: Screen::Landing,
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
            body_scroll: 0,
            body_scroll_max: 0,
            nav_focused: false,
            nav_selected: 0,
            feed_kind: FeedKind::Following,
            keys: KeyBindings::default(),
            auto_refresh_secs: 300,
            landing_selected: 0,
            login_pending: false,
            card_rows: vec![],
            hit_regions: vec![],
            hovered: None,
            tx,
            rx,
            pending_ops: 0,
            pending_since: None,
            generation: 0,
            media_cache: VecDeque::new(),
            media_inflight: HashSet::new(),
            open_media_on_arrival: HashSet::new(),
            image_engine: None,
            external_opener: Arc::new(|target| open::that(target).map_err(anyhow::Error::from)),
            next_background_fetch: Instant::now(),
            thread_sync_attempts: 0,
            home_cache: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_external_opener(mut self, opener: Arc<ExternalOpener>) -> Self {
        self.external_opener = opener;
        self
    }

    pub fn with_config(mut self, config: &Config) -> Self {
        self.keys = KeyBindings::from_config(config.keys.as_ref().map(|k| &k.0));
        self.auto_refresh_secs = config.auto_refresh_secs.unwrap_or(300);
        self
    }

    pub fn with_browser_mode(mut self) -> Self {
        self.browser_mode = true;
        self
    }

    pub fn register_hit(&mut self, x: u16, y: u16, width: u16, height: u16, action: HitAction) {
        if width > 0 && height > 0 {
            self.hit_regions.push(HitRegion {
                x,
                y,
                width,
                height,
                action,
            });
        }
    }

    pub fn hit_at(&self, column: u16, row: u16) -> Option<HitAction> {
        self.hit_regions
            .iter()
            .rev()
            .find(|region| region.contains(column, row))
            .map(|region| region.action)
    }

    pub fn hover_at(&mut self, column: u16, row: u16) -> bool {
        let next = self.hit_at(column, row);
        if next == self.hovered {
            return false;
        }
        self.hovered = next;
        true
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

    pub fn nav_activate(&mut self) {
        let screen = Self::nav_screen(self.nav_selected);
        self.nav_focused = false;
        self.root(screen);
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
        let next = (self.selected as isize + delta).clamp(0, len as isize - 1) as usize;
        if next != self.selected {
            self.selected = next;
            self.body_scroll = 0;
            self.body_scroll_max = 0;
        }
    }

    pub fn advance(&mut self, delta: isize) {
        if matches!(self.screen, Screen::Thread(_)) {
            if delta > 0 && self.body_scroll < self.body_scroll_max {
                self.body_scroll = (self.body_scroll + delta as usize).min(self.body_scroll_max);
                return;
            }
            if delta < 0 && self.body_scroll > 0 {
                self.body_scroll = self.body_scroll.saturating_sub(delta.unsigned_abs());
                return;
            }
        }
        if self.will_load_more(delta) {
            self.request_more();
        }
        self.move_selection(delta);
    }

    /// Whether a forward move of `delta` would need another page from the API.
    pub fn will_load_more(&self, delta: isize) -> bool {
        let prefetch = 12.min(self.posts.len().saturating_sub(2));
        delta > 0
            && !matches!(self.screen, Screen::Lists | Screen::Thread(_))
            && !self.has_pending()
            && self.next_token.is_some()
            && self.selected.saturating_add(delta as usize)
                >= self.posts.len().saturating_sub(prefetch)
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

    pub fn insert_query_text(&mut self, text: &str) {
        let byte = self
            .query
            .char_indices()
            .nth(self.search_cursor)
            .map(|(index, _)| index)
            .unwrap_or(self.query.len());
        self.query.insert_str(byte, text);
        self.search_cursor += text.chars().count();
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

    /// Push the current view onto the back stack and start loading `screen`.
    /// The fetch runs in the background; the spinner animates meanwhile.
    pub fn navigate(&mut self, screen: Screen) {
        self.push_view(screen);
        self.request_screen(self.screen.clone(), false);
    }

    pub fn root(&mut self, screen: Screen) {
        self.remember_home();
        self.history.clear();
        self.screen = screen;
        if matches!(self.screen, Screen::Home)
            && let Some(cached) = self
                .home_cache
                .as_ref()
                .filter(|cached| cached.feed == self.feed_kind)
                .cloned()
        {
            self.posts = cached.posts;
            self.selected = cached.selected.min(self.posts.len().saturating_sub(1));
            self.status = cached.status;
            self.error = cached.error;
            self.next_token = cached.next_token;
            self.body_scroll = cached.body_scroll;
            self.lists.clear();
            self.request_screen(Screen::Home, true);
            return;
        }
        self.posts.clear();
        self.lists.clear();
        self.selected = 0;
        self.error = None;
        self.request_screen(self.screen.clone(), false);
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
            self.body_scroll = view.body_scroll;
            self.body_scroll_max = 0;
            self.next_background_fetch = Instant::now();
            if self.browser_mode && matches!(self.screen, Screen::Home) {
                let api = self.api.clone();
                tokio::spawn(async move { api.release_secondary().await });
            }
        } else {
            self.status = "Already at the top level · press Q to quit".into();
        }
    }

    fn push_view(&mut self, screen: Screen) {
        self.remember_home();
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
            body_scroll: std::mem::take(&mut self.body_scroll),
        };
        if self.history.len() == 16 {
            self.history.remove(0);
        }
        self.history.push(previous);
    }

    fn remember_home(&mut self) {
        if !matches!(self.screen, Screen::Home) || self.posts.is_empty() {
            return;
        }
        self.home_cache = Some(HomeCache {
            feed: self.feed_kind,
            posts: self.posts.clone(),
            selected: self.selected,
            status: self.status.clone(),
            error: self.error.clone(),
            next_token: self.next_token.clone(),
            body_scroll: self.body_scroll,
        });
    }

    pub fn activate(&mut self) {
        match &self.screen {
            Screen::Lists => {
                if let Some(list) = self.selected_list().cloned() {
                    self.navigate(Screen::ListFeed(Box::new(list)))
                }
            }
            _ => {
                if let Some(post) = self.selected_post().cloned() {
                    self.navigate(Screen::Thread(Box::new(post.clone())));
                    // The selected root is already in memory. Paint it on the
                    // first thread frame instead of showing an empty state
                    // while the extension gathers replies.
                    self.posts.push(post);
                }
            }
        }
    }

    pub fn open_profile(&mut self) {
        if let Some(user) = self
            .selected_post()
            .map(|p| p.author.clone())
            .or_else(|| self.me.clone())
        {
            self.navigate(Screen::Profile(Box::new(user)));
        }
    }

    pub fn open_likes(&mut self) {
        let user = match &self.screen {
            Screen::Profile(u) => Some((**u).clone()),
            _ => self.selected_post().map(|p| p.author.clone()),
        };
        if let Some(user) = user {
            self.navigate(Screen::Likes(Box::new(user)));
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
        if let Err(e) = (self.external_opener)(&url) {
            self.error = Some(format!("Could not open browser: {e}"));
        }
    }

    pub fn submit_search(&mut self) {
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
        self.generation += 1;
        self.begin_pending();
        let api = self.api.clone();
        let tx = self.tx.clone();
        let generation = self.generation;
        tokio::spawn(async move {
            let result = api.search(&query, None).await;
            let _ = tx.send(UiMsg::SearchDone { generation, result });
        });
    }

    pub fn refresh(&mut self) {
        self.error = None;
        self.request_screen(self.screen.clone(), false);
    }

    pub fn toggle_feed(&mut self) {
        if !self.browser_mode {
            self.status = "For You is only available in browser mode".into();
            return;
        }
        self.feed_kind = match self.feed_kind {
            FeedKind::Following => FeedKind::ForYou,
            FeedKind::ForYou => FeedKind::Following,
        };
        self.status = match self.feed_kind {
            FeedKind::Following => "Switched to the Following timeline".into(),
            FeedKind::ForYou => "Switched to the algorithmic For You timeline".into(),
        };
        if matches!(self.screen, Screen::Home) {
            self.request_screen(Screen::Home, false);
        }
    }

    /// The landing page menu, in display order.
    pub fn landing_items(&self) -> Vec<(String, LandingAction)> {
        let mut items = vec![];
        if self.demo {
            items.push(("Start (demo — no account)".into(), LandingAction::Start));
        } else {
            let mode = if self.browser_mode {
                "live · browser extension"
            } else {
                "live · X API"
            };
            items.push((format!("Start ({mode})"), LandingAction::Start));
        }
        if !self.browser_mode {
            items.push(("Connect browser extension".into(), LandingAction::SignIn));
        }
        if self.login_pending {
            items.push((
                "I finished signing in — verify".into(),
                LandingAction::Verify,
            ));
        }
        items.push(("Quit".into(), LandingAction::Quit));
        items
    }

    pub fn activate_landing(&mut self) {
        let items = self.landing_items();
        let Some((_, action)) = items.get(self.landing_selected) else {
            return;
        };
        match action {
            LandingAction::Start => self.start_reading(),
            LandingAction::SignIn => self.request_browser_login(),
            LandingAction::Verify => self.request_sign_in_check(),
            LandingAction::Quit => self.should_quit = true,
        }
    }

    /// Enter the Home feed. The landing page stays on the back stack, so Esc
    /// returns to it.
    pub fn start_reading(&mut self) {
        self.landing_selected = 0;
        self.navigate(Screen::Home);
    }

    /// Prepare the extension files without opening an external browser. The
    /// browser's own extension page remains an explicit user-controlled step.
    pub fn request_browser_login(&mut self) {
        self.status = "Preparing the browser extension…".into();
        self.begin_pending();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = crate::extension::prepare_extension().map(|_| ());
            let _ = tx.send(UiMsg::SignInOpened { result });
        });
    }

    /// Connect to the extension and confirm the browser's existing X session.
    pub fn request_sign_in_check(&mut self) {
        self.status = "Checking the browser session…".into();
        self.begin_pending();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = async {
                let api = crate::extension::ExtensionApi::connect().await?;
                api.me().await
            }
            .await;
            let _ = tx.send(UiMsg::SignInChecked { result });
        });
    }

    // ------------------------------------------------------------------
    // Background request/apply plumbing. Every network operation spawns a
    // task that reports back through the channel; the event loop applies the
    // results with `process_messages`. The UI never blocks on I/O.
    // ------------------------------------------------------------------

    pub fn request_bootstrap(&mut self) {
        self.status = "Checking your session…".into();
        self.begin_pending();
        let api = self.api.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let me = api.me().await;
            let _ = tx.send(UiMsg::Bootstrap { me });
        });
    }

    /// Begin loading `screen`. Explore is the search entry point and needs no
    /// network; everything else fetches in the background. `silent` refreshes
    /// in place (used by auto-refresh) without touching status or selection.
    pub fn request_screen(&mut self, screen: Screen, silent: bool) {
        self.screen = screen;
        if !silent {
            self.body_scroll = 0;
            self.body_scroll_max = 0;
            self.thread_sync_attempts = 0;
            self.next_background_fetch = Instant::now();
        }
        if matches!(self.screen, Screen::Explore) {
            self.mode = InputMode::Search;
            self.query.clear();
            self.search_cursor = 0;
            self.posts.clear();
            self.selected = 0;
            self.next_token = None;
            if !silent {
                self.status = "Type a search and press Enter".into();
            }
            return;
        }
        if !silent {
            self.status = "Loading…".into();
        }
        if !silent {
            self.next_token = None;
        }
        self.generation += 1;
        self.begin_pending();
        let api = self.api.clone();
        let tx = self.tx.clone();
        let generation = self.generation;
        let feed = self.feed_kind;
        let screen = self.screen.clone();
        tokio::spawn(async move {
            let result = fetch_screen(&*api, &screen, feed).await;
            let _ = tx.send(UiMsg::ScreenLoaded {
                generation,
                silent,
                result,
            });
        });
    }

    pub fn request_more(&mut self) {
        self.request_more_inner(false);
    }

    fn request_more_inner(&mut self, silent: bool) {
        let Some(token) = self.next_token.clone() else {
            self.status = "You’re all caught up.".into();
            return;
        };
        if matches!(self.screen, Screen::Lists | Screen::Thread(_)) {
            self.status = "No more items are available on this screen.".into();
            return;
        }
        self.status = "Loading more posts…".into();
        let generation = self.generation;
        self.begin_pending();
        let api = self.api.clone();
        let tx = self.tx.clone();
        let screen = self.screen.clone();
        let query = self.query.clone();
        tokio::spawn(async move {
            let result = fetch_more(&*api, &screen, &query, &token).await;
            let _ = tx.send(UiMsg::MoreLoaded {
                generation,
                silent,
                result,
            });
        });
    }

    /// Continuously fill the local timeline reservoir in browser mode. This is
    /// deliberately independent of the cursor: opening or dwelling on an early
    /// card must not stop the extension from fetching later pages.
    pub fn maintain_read_ahead(&mut self) -> bool {
        const MIN_POSTS_AHEAD: usize = 24;
        if self.has_pending() || Instant::now() < self.next_background_fetch {
            return false;
        }

        if matches!(self.screen, Screen::Thread(_)) && self.browser_mode {
            let expected = self
                .posts
                .first()
                .map(|post| post.metrics.reply_count as usize + 1)
                .unwrap_or(2)
                .clamp(2, 100);
            if self.thread_sync_attempts >= 16
                || (self.posts.len() >= expected && self.thread_sync_attempts >= 2)
            {
                return false;
            }
            self.thread_sync_attempts += 1;
            self.next_background_fetch = Instant::now() + Duration::from_millis(500);
            let status = self.status.clone();
            self.request_screen(self.screen.clone(), true);
            self.status = status;
            return true;
        }

        if self.next_token.is_none()
            || matches!(
                self.screen,
                Screen::Landing | Screen::Lists | Screen::Thread(_)
            )
            || (!self.browser_mode
                && self.posts.len().saturating_sub(self.selected) >= MIN_POSTS_AHEAD)
            || (self.browser_mode
                && self.posts.len() >= MAX_RETAINED_POSTS
                && self.posts.len().saturating_sub(self.selected) >= MIN_POSTS_AHEAD)
        {
            return false;
        }
        let status = self.status.clone();
        self.request_more_inner(true);
        self.status = status;
        true
    }

    pub fn cached_media(&self, url: &str) -> Option<&media::PreviewImage> {
        self.media_cache
            .iter()
            .find(|(key, _)| key == url)
            .map(|(_, image)| image)
    }

    pub fn selected_preview_url(&self) -> Option<String> {
        self.selected_post()
            .and_then(|post| post.media.first())
            .and_then(media::best_preview_url)
    }

    pub fn attach_image_engine(&mut self) {
        self.image_engine = Some(media::ImageEngine::detect());
    }

    pub fn activity_ms(&self) -> u128 {
        self.pending_since
            .map(|started| started.elapsed().as_millis())
            .unwrap_or(0)
    }

    /// Decode stills for every visible post so unselected cards also show
    /// native images once the bytes arrive.
    pub fn ensure_visible_media<I>(&mut self, urls: I)
    where
        I: IntoIterator<Item = String>,
    {
        for url in urls {
            self.start_media_fetch(url, false);
        }
    }

    pub fn request_media_preview(&mut self) {
        let Some(item) = self.selected_post().and_then(|p| p.media.first()).cloned() else {
            self.status = "This post has no attached media.".into();
            return;
        };
        let Some(url) = media::best_preview_url(&item) else {
            self.status = "No still preview is available — press V to open the media.".into();
            return;
        };
        let alt = item.alt_text.unwrap_or_else(|| format!("{:?}", item.kind));
        if let Some(image) = self.cached_media(&url).cloned() {
            self.media_preview = Some((alt, image));
            self.status = "Media preview · cached".into();
            return;
        }
        self.status = "Rendering media preview…".into();
        self.open_media_on_arrival.insert(url.clone());
        self.start_media_fetch(url, true);
    }

    fn start_media_fetch(&mut self, url: String, counted: bool) {
        if self.cached_media(&url).is_some() || !self.media_inflight.insert(url.clone()) {
            return;
        }
        if counted {
            self.begin_pending();
        }
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = media::download_preview(&url).await;
            let _ = tx.send(UiMsg::MediaDone {
                url,
                open: counted,
                counted,
                result,
            });
        });
    }

    /// Apply every completed background result. Returns true when state
    /// changed and the frame must be repainted.
    pub fn process_messages(&mut self) -> bool {
        let mut changed = false;
        while let Ok(message) = self.rx.try_recv() {
            changed = true;
            let counted = !matches!(&message, UiMsg::MediaDone { counted: false, .. });
            if counted {
                self.end_pending();
            }
            match message {
                UiMsg::Bootstrap { me } => match me {
                    Ok(me) => {
                        self.me = Some(me.clone());
                        self.status =
                            format!("Signed in as @{} — press Enter to start", me.username);
                    }
                    Err(_) => {
                        self.me = None;
                        self.status =
                            "Not connected — demo mode is ready, or connect the browser extension"
                                .into();
                    }
                },
                UiMsg::ScreenLoaded {
                    generation,
                    silent,
                    result,
                } => {
                    if generation != self.generation {
                        continue;
                    }
                    self.apply_screen_data(&self.screen.clone(), result, silent);
                }
                UiMsg::SearchDone { generation, result } => {
                    if generation != self.generation {
                        continue;
                    }
                    match result {
                        Ok(page) => {
                            self.posts = page.items;
                            self.next_token = page.next_token;
                            self.status = format!("{} results", self.posts.len());
                        }
                        Err(e) => self.set_error(e),
                    }
                }
                UiMsg::MoreLoaded {
                    generation,
                    silent,
                    result,
                } => {
                    if generation != self.generation {
                        continue;
                    }
                    self.apply_more(result, silent);
                }
                UiMsg::MediaDone {
                    url,
                    open,
                    counted: _,
                    result,
                } => {
                    self.media_inflight.remove(&url);
                    let should_open = open || self.open_media_on_arrival.remove(&url);
                    match result {
                        Ok(image) => {
                            if self.media_cache.len() >= 32 {
                                if let Some((evicted, _)) = self.media_cache.pop_back() {
                                    if let Some(engine) = self.image_engine.as_mut() {
                                        engine.drop_url(&evicted);
                                    }
                                }
                            }
                            self.media_cache.push_front((url.clone(), image.clone()));
                            if should_open {
                                let alt = self
                                    .selected_post()
                                    .and_then(|p| p.media.first())
                                    .and_then(|m| m.alt_text.clone())
                                    .unwrap_or_else(|| "Media".into());
                                self.media_preview = Some((alt, image));
                                self.status = "Media preview · rendered".into();
                            }
                        }
                        Err(e) => {
                            if should_open {
                                self.set_error(e);
                            }
                        }
                    }
                }
                UiMsg::SignInOpened { result } => match result {
                    Ok(()) => {
                        self.login_pending = true;
                        self.status =
                            "Extension prepared — load it in Edge, then press r to verify".into();
                    }
                    Err(e) => self.set_error(e),
                },
                UiMsg::SignInChecked { result } => match result {
                    Ok(user) if !user.username.is_empty() => {
                        self.me = Some(user.clone());
                        self.browser_mode = true;
                        self.demo = false;
                        self.login_pending = false;
                        if let Err(error) = crate::config::Config::enable_extension_mode() {
                            self.status = format!(
                                "Signed in as @{} — but saving the mode failed: {error}",
                                user.username
                            );
                        } else {
                            self.status = format!(
                                "Connected as @{} — restart XTUI to use the extension",
                                user.username
                            );
                        }
                    }
                    Ok(_) => self.set_error("X is not signed in in the browser profile"),
                    Err(e) => self.set_error(e),
                },
            }
        }
        changed
    }

    fn apply_more(&mut self, result: Result<Page<Post>>, silent: bool) {
        let prior_status = self.status.clone();
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
                self.next_token = page.next_token;
                self.next_background_fetch = Instant::now()
                    + if added == 0 {
                        Duration::from_millis(700)
                    } else {
                        Duration::from_millis(20)
                    };
                self.status = if added == 0 {
                    "You’re all caught up.".into()
                } else {
                    format!("Loaded {added} more posts")
                };
            }
            Err(error) => self.set_error(error),
        }
        if silent && self.error.is_none() {
            self.status = prior_status;
        }
    }

    fn apply_screen_data(&mut self, screen: &Screen, result: Result<ScreenPayload>, silent: bool) {
        match result {
            Ok(ScreenPayload::Page(page)) => {
                self.apply_page(page, silent);
                if !silent {
                    self.status = self.feed_status();
                }
            }
            Ok(ScreenPayload::Thread(mut posts)) => {
                if let Screen::Thread(post) = screen
                    && !posts.iter().any(|p| p.id == post.id)
                {
                    posts.insert(0, (**post).clone());
                }
                self.posts = posts;
                self.selected = self.selected.min(self.posts.len().saturating_sub(1));
                self.next_background_fetch = Instant::now() + Duration::from_millis(300);
                self.thread_expanded = true;
                if !silent {
                    self.status = self.feed_status();
                }
            }
            Ok(ScreenPayload::Lists(lists)) => {
                self.lists = lists;
                self.selected = self.selected.min(self.lists.len().saturating_sub(1));
                if !silent {
                    self.status = self.feed_status();
                }
            }
            Err(error) => self.set_error(error),
        }
    }

    fn apply_page(&mut self, page: Page<Post>, silent: bool) {
        let anchor = if silent {
            self.selected_post().map(|post| post.id.clone())
        } else {
            None
        };
        if silent {
            let mut merged = page.items;
            let incoming: std::collections::HashSet<_> =
                merged.iter().map(|post| post.id.clone()).collect();
            merged.extend(
                std::mem::take(&mut self.posts)
                    .into_iter()
                    .filter(|post| !incoming.contains(&post.id)),
            );
            self.posts = merged;
            self.trim_feed();
        } else {
            self.posts = page.items;
            self.next_token = page.next_token;
        }
        self.selected = self.selected.min(self.posts.len().saturating_sub(1));
        if let Some(anchor) = anchor {
            if let Some(position) = self.posts.iter().position(|post| post.id == anchor) {
                self.selected = position;
            }
            self.status = "Timeline refreshed".into();
        }
    }

    fn feed_status(&self) -> String {
        if self.posts.is_empty() && !matches!(self.screen, Screen::Lists | Screen::Explore) {
            "Nothing here yet.".into()
        } else if self.demo {
            "DEMO · no network or account required".into()
        } else if self.browser_mode {
            "LIVE · browser extension · no API credits".into()
        } else {
            "Live · X API".into()
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
            Screen::Landing => "XTUI".into(),
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

    // ------------------------------------------------------------------
    // Spinner / pending state
    // ------------------------------------------------------------------

    fn begin_pending(&mut self) {
        self.pending_ops += 1;
        if self.pending_since.is_none() {
            self.pending_since = Some(Instant::now());
        }
    }

    fn end_pending(&mut self) {
        self.pending_ops = self.pending_ops.saturating_sub(1);
        if self.pending_ops == 0 {
            self.pending_since = None;
        }
    }

    pub fn has_pending(&self) -> bool {
        self.pending_ops > 0
    }

    pub fn has_media_inflight(&self) -> bool {
        !self.media_inflight.is_empty()
    }

    /// The current spinner glyph, or None when idle.
    pub fn spinner_frame(&self) -> Option<char> {
        if self.pending_ops == 0 {
            return None;
        }
        let elapsed = self
            .pending_since
            .unwrap_or_else(Instant::now)
            .elapsed()
            .as_millis() as usize;
        Some(SPINNER_FRAMES[elapsed / 100 % SPINNER_FRAMES.len()])
    }

    /// Test helper: block until every background fetch has been applied.
    pub async fn drain(&mut self) {
        loop {
            if !self.has_pending() && !self.has_media_inflight() && self.rx.is_empty() {
                return;
            }
            self.process_messages();
            tokio::task::yield_now().await;
        }
    }
}

// ----------------------------------------------------------------------
// Fetch helpers shared by the background tasks and the tests.
// ----------------------------------------------------------------------

pub async fn fetch_screen(api: &dyn Api, screen: &Screen, feed: FeedKind) -> Result<ScreenPayload> {
    match screen {
        Screen::Landing => bail!("the landing page requires no fetch"),
        Screen::Home => {
            api.set_feed(feed).await;
            api.home(None).await.map(ScreenPayload::Page)
        }
        Screen::Explore => Ok(ScreenPayload::Page(Page {
            items: Vec::new(),
            next_token: None,
        })),
        Screen::Mentions => api.mentions(None).await.map(ScreenPayload::Page),
        Screen::Bookmarks => api.bookmarks(None).await.map(ScreenPayload::Page),
        Screen::Lists => api.lists().await.map(ScreenPayload::Lists),
        Screen::ListFeed(list) => api
            .list_posts(&list.id, None)
            .await
            .map(ScreenPayload::Page),
        Screen::Profile(user) => api
            .user_posts(&user.id, None)
            .await
            .map(ScreenPayload::Page),
        Screen::Likes(user) => api.likes(&user.id, None).await.map(ScreenPayload::Page),
        Screen::Thread(post) => api
            .thread(&post.conversation_id)
            .await
            .map(ScreenPayload::Thread),
    }
}

pub async fn fetch_more(
    api: &dyn Api,
    screen: &Screen,
    query: &str,
    token: &str,
) -> Result<Page<Post>> {
    match screen {
        Screen::Landing => bail!("the landing page requires no fetch"),
        Screen::Home => api.home(Some(token)).await,
        Screen::Explore => api.search(query, Some(token)).await,
        Screen::Mentions => api.mentions(Some(token)).await,
        Screen::Bookmarks => api.bookmarks(Some(token)).await,
        Screen::ListFeed(list) => api.list_posts(&list.id, Some(token)).await,
        Screen::Profile(user) => api.user_posts(&user.id, Some(token)).await,
        Screen::Likes(user) => api.likes(&user.id, Some(token)).await,
        Screen::Lists | Screen::Thread(_) => bail!("no more items on this screen"),
    }
}

/// Bootstrap using the direct fetch path (equivalent to the background task,
/// but deterministic for tests).
#[cfg(test)]
impl App {
    pub(crate) async fn bootstrap_for_test(&mut self) {
        if let Ok(me) = self.api.me().await {
            self.me = Some(me);
        }
        self.screen = Screen::Home;
        if let Ok(payload) = fetch_screen(&*self.api, &self.screen, self.feed_kind).await {
            self.apply_screen_data(&self.screen.clone(), Ok(payload), false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, KeyConfig};
    use crate::demo::DemoApi;
    use crossterm::event::{KeyCode, KeyEvent};
    use std::collections::HashMap;

    #[tokio::test]
    async fn navigation_preserves_a_back_stack() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap_for_test().await;
        app.selected = 1;
        app.query = "remember me".into();
        app.next_token = Some("cursor".into());
        let original_ids: Vec<_> = app.posts.iter().map(|post| post.id.clone()).collect();
        let selected = app.selected_post().unwrap().clone();
        app.navigate(Screen::Thread(Box::new(selected)));
        app.drain().await;
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
        app.bootstrap_for_test().await;
        assert!(app.posts.iter().all(|p| p.in_reply_to_user_id.is_none()));
        app.activate();
        app.drain().await;
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
        app.bootstrap_for_test().await;
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
        app.bootstrap_for_test().await;
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
    async fn pasted_text_inserts_at_the_cursor() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.insert_query_char('x');
        app.move_search_cursor(-1);
        app.insert_query_text("rust ");
        assert_eq!(app.query, "rust x", "paste lands at the cursor position");
        app.insert_query_text("y");
        assert_eq!(app.query, "rust yx");
    }

    #[tokio::test]
    async fn advancing_near_the_end_requests_and_deduplicates_more_posts() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap_for_test().await;
        let original = app.posts.len();
        app.next_token = Some("next".into());
        app.selected = original.saturating_sub(2);
        app.advance(1);
        app.drain().await;
        assert_eq!(app.posts.len(), original);
        assert_eq!(app.status, "You’re all caught up.");
    }

    #[tokio::test]
    async fn will_load_more_is_true_only_near_the_end_of_a_pageable_feed() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap_for_test().await;
        assert!(!app.will_load_more(1), "no next page, no fetch");
        app.next_token = Some("next".into());
        app.selected = 0;
        assert!(!app.will_load_more(1));
        app.selected = app.posts.len().saturating_sub(2);
        assert!(app.will_load_more(1));
        app.root(Screen::Lists);
        app.drain().await;
        assert!(!app.will_load_more(1), "lists never paginate");
        app.activate();
        app.drain().await;
        assert!(
            !app.will_load_more(1),
            "threads never paginate through the feed"
        );
    }

    #[tokio::test]
    async fn read_ahead_prefetches_before_the_cursor_reaches_the_page_end() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap_for_test().await;
        app.next_token = Some("next".into());
        app.selected = 0;
        let status = app.status.clone();
        assert!(
            app.maintain_read_ahead(),
            "a short first page starts background prefetch immediately"
        );
        assert_eq!(app.status, status, "prefetch stays visually silent");
        assert!(app.has_pending());
        app.drain().await;
        assert!(!app.has_pending());
    }

    #[tokio::test]
    async fn browser_mode_keeps_prefetching_while_reader_stays_at_the_top() {
        let mut app = App::new(Arc::new(DemoApi::new()), false).with_browser_mode();
        app.bootstrap_for_test().await;
        let seed = app.posts.clone();
        while app.posts.len() < 40 {
            app.posts.extend(seed.clone());
        }
        app.selected = 0;
        app.next_token = Some("next".into());
        assert!(
            app.posts.len().saturating_sub(app.selected) >= 24,
            "the ordinary near-end prefetch condition is not met"
        );
        assert!(
            app.maintain_read_ahead(),
            "browser mode fills its reservoir independently of reading position"
        );
        app.drain().await;
    }

    #[tokio::test]
    async fn thread_navigation_scrolls_long_body_before_moving_to_replies() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap_for_test().await;
        let root = app.posts[0].clone();
        let reply = app.posts[1].clone();
        app.screen = Screen::Thread(Box::new(root.clone()));
        app.posts = vec![root, reply];
        app.body_scroll_max = 12;
        app.advance(5);
        assert_eq!(app.selected, 0);
        assert_eq!(app.body_scroll, 5);
        app.advance(-2);
        assert_eq!(app.body_scroll, 3);
        app.body_scroll = app.body_scroll_max;
        app.advance(1);
        assert_eq!(
            app.selected, 1,
            "selection moves after the full body is read"
        );
        assert_eq!(app.body_scroll, 0);
    }

    #[tokio::test]
    async fn submitted_search_is_trimmed() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap_for_test().await;
        app.begin_search();
        for character in "  terminal  ".chars() {
            app.insert_query_char(character);
        }
        app.submit_search();
        app.drain().await;
        assert_eq!(app.query, "terminal");
        assert!(!app.posts.is_empty());
    }

    #[tokio::test]
    async fn sidebar_focus_moves_with_arrows_and_activates_sections() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap_for_test().await;
        app.toggle_nav_focus();
        assert!(app.nav_focused);
        assert_eq!(app.nav_selected, 0, "focus starts on the current section");
        app.nav_move(2);
        assert_eq!(app.nav_selected, 2);
        app.nav_activate();
        app.drain().await;
        assert!(!app.nav_focused, "activating leaves the sidebar");
        assert!(matches!(app.screen, Screen::Mentions));
        app.toggle_nav_focus();
        app.nav_move(5);
        assert_eq!(app.nav_selected, 4, "rail clamps at Lists");
        app.nav_move(-9);
        assert_eq!(app.nav_selected, 0, "rail clamps at Home");
    }

    #[tokio::test]
    async fn stale_screen_results_are_dropped() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.request_bootstrap();
        // The user navigates before bootstrap lands: a fresh screen request
        // bumps the generation, so the bootstrap result must be discarded.
        app.request_screen(Screen::Mentions, false);
        app.drain().await;
        assert!(matches!(app.screen, Screen::Mentions));
    }

    #[tokio::test]
    async fn spinner_animates_while_requests_are_pending() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        assert_eq!(app.spinner_frame(), None, "idle shows no spinner");
        app.request_bootstrap();
        assert!(app.has_pending());
        assert!(
            app.spinner_frame().is_some(),
            "a pending fetch must expose a spinner frame"
        );
        app.drain().await;
        assert!(!app.has_pending());
        assert_eq!(app.spinner_frame(), None);
    }

    #[tokio::test]
    async fn silent_refresh_preserves_the_selected_post() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap_for_test().await;
        app.selected = 2;
        let anchor = app.selected_post().unwrap().id.clone();
        app.request_screen(Screen::Home, true);
        app.drain().await;
        assert_eq!(
            app.selected_post().map(|post| post.id.clone()),
            Some(anchor),
            "silent refresh keeps the reading position"
        );
        assert_eq!(app.status, "Timeline refreshed");
    }

    #[tokio::test]
    async fn returning_to_home_never_replaces_the_retained_feed_with_a_small_page() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap_for_test().await;
        let mut retained_tail = app.posts.clone();
        for (index, post) in retained_tail.iter_mut().enumerate() {
            post.id = format!("cached-{index}");
        }
        app.posts.extend(retained_tail);
        app.selected = 4;
        let retained = app.posts.len();

        app.root(Screen::Explore);
        app.drain().await;
        app.root(Screen::Home);

        assert_eq!(app.posts.len(), retained);
        assert_eq!(app.selected, 4);
        app.drain().await;
        assert!(app.posts.len() >= retained);
    }

    #[tokio::test]
    async fn silent_refresh_prepends_fresh_posts_without_resetting_tail_pagination() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap_for_test().await;
        app.selected = 2;
        app.next_token = Some("tail-position".into());
        let anchor = app.selected_post().unwrap().id.clone();
        let mut fresh = app.posts[0].clone();
        fresh.id = "fresh-head-post".into();
        app.apply_page(
            Page {
                items: vec![fresh],
                next_token: Some("head-sample".into()),
            },
            true,
        );
        assert_eq!(app.posts[0].id, "fresh-head-post");
        assert_eq!(app.selected_post().map(|post| &post.id), Some(&anchor));
        assert_eq!(app.next_token.as_deref(), Some("tail-position"));
    }

    #[tokio::test]
    async fn feed_toggle_is_rejected_outside_browser_mode() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        app.bootstrap_for_test().await;
        assert_eq!(app.feed_kind, FeedKind::Following);
        app.toggle_feed();
        assert_eq!(app.feed_kind, FeedKind::Following, "demo has no For You");
        assert!(app.status.contains("browser mode"));
    }

    #[test]
    fn config_applies_bindings_refresh_interval_and_theme_settings() {
        let mut keys = HashMap::new();
        keys.insert("move_down".to_owned(), vec!["s".to_owned()]);
        let config = Config {
            auto_refresh_secs: Some(42),
            keys: Some(KeyConfig(keys)),
            ..Config::default()
        };
        let app = App::new(Arc::new(DemoApi::new()), true).with_config(&config);
        assert_eq!(app.auto_refresh_secs, 42);
        assert_eq!(
            app.keys.action_for(&KeyEvent::from(KeyCode::Char('s'))),
            Some(crate::keys::Action::MoveDown),
            "configured keys override the defaults"
        );
        assert_eq!(
            app.keys.action_for(&KeyEvent::from(KeyCode::Char('j'))),
            None,
            "an overridden action drops its default bindings"
        );
        let bare = App::new(Arc::new(DemoApi::new()), true);
        assert_eq!(bare.auto_refresh_secs, 300, "default refresh interval");
    }
}
