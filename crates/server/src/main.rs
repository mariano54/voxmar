//! Dedicated server:
//! `voxmar-server [--world DIR] [--bind ADDR] [--seed N] [--no-flight] [--view-distance N] [--admin NAME]...`.
//!
//! The world lives in `DIR` (default `./world`) and is created there if it does not exist
//! yet, with `--seed` and, given `--no-flight`, in survival mode; an existing world keeps
//! its own seed and mode. It is saved every 30 seconds and when `stop` is typed.

use std::net::SocketAddr;

use voxmar_protocol::{DEFAULT_PORT, tcp};
use voxmar_server::{ServerConfig, assets_dir, load_content, spawn};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let mut config = ServerConfig { world: Some("world".into()), ..ServerConfig::default() };
    let mut bind: SocketAddr = ([0, 0, 0, 0], DEFAULT_PORT).into();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--world" => config.world = Some(value()?.into()),
            "--bind" => bind = value()?.parse()?,
            "--seed" => config.seed = value()?.parse()?,
            "--view-distance" => config.view_distance = value()?.parse()?,
            "--no-flight" => config.allow_flight = false,
            "--admin" => config.admins.push(value()?),
            _ => return Err(format!("unknown argument {arg}").into()),
        }
    }

    let registry = load_content(&assets_dir())?;
    let server = spawn(config, registry)?;
    let addr = tcp::listen(bind, server.connections())?;
    tracing::info!("listening on {addr}; type `stop` to save and quit");
    for line in std::io::stdin().lines() {
        match line?.trim() {
            "stop" => {
                // Dropping the server saves the world.
                drop(server);
                return Ok(());
            }
            "" => {}
            other => tracing::warn!("unknown command `{other}`; type `stop` to save and quit"),
        }
    }
    // Without a console, run until stopped from outside.
    loop {
        std::thread::park();
    }
}
