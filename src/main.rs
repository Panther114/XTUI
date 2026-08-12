use anyhow::Result;
use std::sync::Arc;
use xtui::{
    api::{Api, XApi},
    app::App,
    auth,
    config::Config,
    demo::DemoApi,
    scrape::ScrapeApi,
};

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next();

    if matches!(command.as_deref(), Some("--help" | "-h" | "help")) {
        print_help();
        return Ok(());
    }

    if command.as_deref() == Some("login") {
        let client_id = args.next().or_else(|| std::env::var("XTUI_CLIENT_ID").ok());
        auth::login(client_id.as_deref()).await?;
        return Ok(());
    }

    if command.as_deref() == Some("logout") {
        auth::logout()?;
        println!("XTUI credentials removed.");
        return Ok(());
    }

    if command.as_deref() == Some("browser-login") {
        ScrapeApi::open_login().await?;
        let mut config = Config::load()?;
        config.source = Some("browser".into());
        config.save()?;
        println!(
            "XTUI opened its isolated browser profile. Sign in to X there, then run `xtui browser`."
        );
        return Ok(());
    }
    if command.as_deref() == Some("browser-check") {
        let api = ScrapeApi::connect().await?;
        let me = api.me().await?;
        let home = api.home(None).await?;
        let more = api.home(home.next_token.as_deref()).await?;
        let first_ids: std::collections::HashSet<_> = home.items.iter().map(|p| &p.id).collect();
        let new_posts = more
            .items
            .iter()
            .filter(|p| !first_ids.contains(&p.id))
            .count();
        let more_again = api.home(more.next_token.as_deref()).await?;
        let loaded_ids: std::collections::HashSet<_> = home
            .items
            .iter()
            .chain(more.items.iter())
            .map(|post| &post.id)
            .collect();
        let newer_posts = more_again
            .items
            .iter()
            .filter(|post| !loaded_ids.contains(&post.id))
            .count();
        println!(
            "Browser session: @{} ({})\nVisible Following posts: {}\nNew posts after scroll: {}\nNew posts after second scroll: {}",
            me.username,
            me.name,
            home.items.len(),
            new_posts,
            newer_posts
        );
        if let Some(check) = args.next() {
            if check == "--all" {
                let sweep_started = std::time::Instant::now();
                let thread_target = home
                    .items
                    .iter()
                    .chain(more.items.iter())
                    .chain(more_again.items.iter())
                    .max_by_key(|post| post.metrics.reply_count)
                    .map(|post| {
                        (
                            post.id.clone(),
                            post.author.username.clone(),
                            post.metrics.reply_count,
                        )
                    });
                let search = api.search("rust", None).await.map(|page| page.items.len());
                eprintln!(
                    "checked search ({:.1}s)",
                    sweep_started.elapsed().as_secs_f32()
                );
                let bookmarks = api.bookmarks(None).await.map(|page| page.items.len());
                eprintln!(
                    "checked bookmarks ({:.1}s)",
                    sweep_started.elapsed().as_secs_f32()
                );
                let mentions = api.mentions(None).await.map(|page| page.items.len());
                eprintln!(
                    "checked mentions ({:.1}s)",
                    sweep_started.elapsed().as_secs_f32()
                );
                let profile = api.user_by_username(&me.username).await;
                eprintln!(
                    "checked profile ({:.1}s)",
                    sweep_started.elapsed().as_secs_f32()
                );
                let posts = api
                    .user_posts(&me.username, None)
                    .await
                    .map(|page| page.items.len());
                eprintln!(
                    "checked profile posts ({:.1}s)",
                    sweep_started.elapsed().as_secs_f32()
                );
                let likes = api
                    .likes(&me.username, None)
                    .await
                    .map(|page| page.items.len());
                eprintln!(
                    "checked likes ({:.1}s)",
                    sweep_started.elapsed().as_secs_f32()
                );
                let lists = api.lists().await.map(|items| items.len());
                eprintln!(
                    "checked lists ({:.1}s)",
                    sweep_started.elapsed().as_secs_f32()
                );
                let thread = match thread_target.as_ref() {
                    Some((id, _, _)) => api.thread(id).await.map(|items| items.len()),
                    None => Ok(0),
                };
                eprintln!(
                    "checked thread ({:.1}s)",
                    sweep_started.elapsed().as_secs_f32()
                );
                println!(
                    "Search: {}\nBookmarks: {}\nMentions: {}\nProfile: {}\nProfile posts: {}\nLikes: {}\nLists: {}\nThread target: {}\nThread posts: {}",
                    smoke(search),
                    smoke(bookmarks),
                    smoke(mentions),
                    profile
                        .map(|user| format!("ok (@{})", user.username))
                        .unwrap_or_else(|error| format!("ERROR: {error}")),
                    smoke(posts),
                    smoke(likes),
                    smoke(lists),
                    thread_target
                        .map(|(id, user, replies)| format!("@{user}/{id} ({replies} replies)"))
                        .unwrap_or_else(|| "none".into()),
                    smoke(thread)
                );
            } else {
                let results = api.search(&check, None).await?;
                println!("Search `{check}` results: {}", results.items.len());
            }
        }
        return Ok(());
    }

    let config = Config::load()?;
    let browser_mode = command.as_deref() == Some("browser") || config.use_browser();
    let demo =
        command.as_deref() == Some("demo") || (!browser_mode && config.access_token().is_none());
    let api: Arc<dyn xtui::api::Api> = if demo {
        Arc::new(DemoApi::new())
    } else if browser_mode {
        Arc::new(ScrapeApi::connect().await?)
    } else {
        Arc::new(XApi::new(config)?)
    };

    let mut app = App::new(api, demo);
    if browser_mode {
        app = app.with_browser_mode();
    }
    xtui::ui::run(&mut app).await
}

fn smoke(result: anyhow::Result<usize>) -> String {
    result
        .map(|count| format!("ok ({count})"))
        .unwrap_or_else(|error| format!("ERROR: {error}"))
}

fn print_help() {
    println!(
        "XTUI — X, without leaving your terminal\n\n\
         Usage:\n  xtui                 Start with saved login, or demo mode when logged out\n  \
         xtui demo            Explore the complete interface with sample data\n  \
         xtui browser-login   Open XTUI's isolated browser profile for X sign-in\n  \
         xtui browser         Browse your live feed through the browser companion\n  \
         xtui browser-check   Verify the browser session and timeline extraction\n  \
         xtui login [CLIENT]  Authorize through X OAuth 2.0 PKCE (paid API)\n  \
         xtui logout          Remove locally saved credentials\n  \
         xtui --help          Show this help\n\n\
         You can also set XTUI_ACCESS_TOKEN and XTUI_CLIENT_ID."
    );
}
