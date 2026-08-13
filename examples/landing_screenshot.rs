use anyhow::{Context, Result, bail};
use std::{path::PathBuf, sync::Arc};
use xtui::{
    app::App,
    demo::DemoApi,
    ui::screenshot::{CaptureOptions, capture_svg},
};

fn main() -> Result<()> {
    let mut options = CaptureOptions::landing();
    let mut selected = 0usize;
    let mut output = PathBuf::from("artifacts/landing.svg");
    let mut args = std::env::args().skip(1);

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--width" => options.columns = parse_u16(args.next(), "--width")?,
            "--height" => options.rows = parse_u16(args.next(), "--height")?,
            "--selected" => selected = parse_usize(args.next(), "--selected")?,
            "--output" => output = PathBuf::from(args.next().context("--output requires a path")?),
            "--help" | "-h" => {
                println!(
                    "cargo run --example landing_screenshot -- [--width 110] [--height 44] \\\n+                     [--selected 0] [--output artifacts/landing.svg]"
                );
                return Ok(());
            }
            other => bail!("unknown argument: {other}"),
        }
    }

    let mut app = App::new(Arc::new(DemoApi::new()), true);
    app.landing_selected = selected;
    let svg = capture_svg(&mut app, options)?;
    if let Some(parent) = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(&output, svg)
        .with_context(|| format!("failed to write {}", output.display()))?;
    println!("{}", output.canonicalize()?.display());
    Ok(())
}

fn parse_u16(value: Option<String>, flag: &str) -> Result<u16> {
    value
        .with_context(|| format!("{flag} requires a value"))?
        .parse()
        .with_context(|| format!("{flag} requires an integer"))
}

fn parse_usize(value: Option<String>, flag: &str) -> Result<usize> {
    value
        .with_context(|| format!("{flag} requires a value"))?
        .parse()
        .with_context(|| format!("{flag} requires an integer"))
}
