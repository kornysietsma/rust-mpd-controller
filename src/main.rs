#![forbid(unsafe_code)]
#![warn(clippy::all)]
#![warn(rust_2018_idioms)]
#![warn(rust_2024_compatibility)]
#![warn(deprecated_safe)]
pub mod mymodule;

use anyhow::Result;
use env_logger::Env;

use clap::Parser;
use log::info;
use log::warn;

use crate::mymodule::frobnicate;

fn run(widgets: u32) -> Result<()> {
    info!("frobnicating!");
    if frobnicate(widgets)? {
        info!("success!");
    }
    Ok(())
}

#[derive(Debug, Parser)]
#[clap(author, version, about = "TODO: better description")]
struct Opts {
    #[clap(value_parser, short, long)]
    widgets: u32,
}

fn main() -> Result<()> {
    // default log level to info
    // set RUST_LOG in environment to override
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    let opts = Opts::parse();

    run(opts.widgets)
}
