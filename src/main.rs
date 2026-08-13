use anyhow::{Result, bail};
use std::sync::Arc;
use xtui::{
    api::XApi,
    app::App,
    auth,
    config::Config,
    demo::DemoApi,
    extension::{self, BrowserTarget, ExtensionApi},
    ui::{Theme, init_theme},
};

#[tokio::main]
async fn main() -> Result<()> {
    let native_origin = format!("chrome-extension://{}/", extension::EXTENSION_ID);
    if std::env::args().nth(1).as_deref() == Some(native_origin.as_str()) {
        return extension::run_native_host();
    }
    let mut args = std::env::args().skip(1);
    let command = args.next();

    if command.as_deref() == Some("native-host") {
        return extension::run_native_host();
    }
    if matches!(command.as_deref(), Some("--help" | "-h" | "help")) {
        print_help();
        return Ok(());
    }
    if matches!(command.as_deref(), Some("--version" | "-V" | "version")) {
        println!("xtui {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if command.as_deref() == Some("login") {
        let client_id = args.next().or_else(|| std::env::var("XTUI_CLIENT_ID").ok());
        auth::login(client_id.as_deref()).await?;
        return Ok(());
    }
    if command.as_deref() == Some("logout") {
        auth::logout()?;
        println!("XTUI API credentials removed.");
        return Ok(());
    }
    if command.as_deref() == Some("extension") {
        return extension_command(args.next().as_deref(), args.next().as_deref()).await;
    }
    if matches!(
        command.as_deref(),
        Some("browser" | "browser-login" | "browser-check")
    ) {
        bail!("isolated browser mode was removed in XTUI 0.2; run `xtui extension install --edge`");
    }

    let config = Config::load()?;
    init_theme(Theme::from_config(&config.theme));
    let extension_mode = command.as_deref() == Some("live") || config.use_extension();
    let demo =
        command.as_deref() == Some("demo") || (!extension_mode && config.access_token().is_none());

    if extension_mode {
        let api = Arc::new(ExtensionApi::connect().await?);
        let mut app = App::new(api.clone(), false)
            .with_config(&config)
            .with_browser_mode();
        let result = xtui::ui::run(&mut app).await;
        api.shutdown().await;
        return result;
    }

    let api: Arc<dyn xtui::api::Api> = if demo {
        Arc::new(DemoApi::new())
    } else {
        Arc::new(XApi::new(config.clone())?)
    };
    let mut app = App::new(api, demo).with_config(&config);
    xtui::ui::run(&mut app).await
}

async fn extension_command(action: Option<&str>, browser: Option<&str>) -> Result<()> {
    let target = BrowserTarget::parse(browser)?;
    match action.unwrap_or("status") {
        "prepare" | "path" => println!("{}", extension::prepare_extension()?.display()),
        "install" => {
            extension::install_extension(target)?;
        }
        "status" => println!(
            "{}",
            serde_json::to_string_pretty(&extension::installation_status(target)?)?
        ),
        "check" => {
            let started = std::time::Instant::now();
            let api = ExtensionApi::connect().await?;
            use xtui::api::Api;
            let me = api.me().await?;
            let session_ms = started.elapsed().as_millis();
            let first_started = std::time::Instant::now();
            let first = api.home(None).await?;
            let first_ms = first_started.elapsed().as_millis();
            let second_started = std::time::Instant::now();
            let second = api.home(first.next_token.as_deref()).await?;
            let second_ms = second_started.elapsed().as_millis();
            let thread_target = first
                .items
                .iter()
                .chain(second.items.iter())
                .find(|post| post.metrics.reply_count > 0);
            let thread_result = if let Some(post) = thread_target {
                let thread_started = std::time::Instant::now();
                let replies = api.thread(&post.id).await?;
                Some((
                    post.id.as_str(),
                    replies.len(),
                    thread_started.elapsed().as_millis(),
                ))
            } else {
                None
            };
            println!(
                "Extension session: @{} ({}) in {} ms\nFirst page: {} posts in {} ms\nNext page: {} posts in {} ms",
                me.username,
                me.name,
                session_ms,
                first.items.len(),
                first_ms,
                second.items.len(),
                second_ms
            );
            if let Some((post_id, posts, elapsed_ms)) = thread_result {
                println!("Thread {post_id}: {posts} posts in {elapsed_ms} ms");
            } else {
                println!("Thread probe: skipped (the sampled cards reported no replies)");
            }
            api.shutdown().await;
        }
        other => bail!(
            "unknown extension action `{other}`; use install, prepare, path, status, or check"
        ),
    }
    Ok(())
}

fn print_help() {
    println!(
        "XTUI {} — X, without leaving your terminal\n\n\
         Usage:\n  xtui                         Start with the saved source, or demo when unconfigured\n  \
         xtui demo                    Explore the complete interface with sample data\n  \
         xtui live                    Browse through the installed browser extension\n  \
         xtui extension install --edge   Prepare and register the Edge extension\n  \
         xtui extension install --chrome Prepare and register the Chrome extension\n  \
         xtui extension status --edge    Inspect extension/native-host installation\n  \
         xtui extension check --edge     Verify the existing X browser session\n  \
         xtui login [CLIENT]          Authorize through X OAuth 2.0 PKCE (paid API)\n  \
         xtui logout                  Remove locally saved API credentials\n  \
         xtui --version               Show the version\n  \
         xtui --help                  Show this help\n\n\
         The extension uses your browser's existing X session; it never copies cookies.",
        env!("CARGO_PKG_VERSION")
    );
}
