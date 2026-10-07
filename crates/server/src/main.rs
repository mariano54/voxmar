//! Dedicated server:
//! `voxmar-server [--world DIR] [--bind ADDR] [--seed N] [--no-flight] [--view-distance N] [--max-players N] [--admin-token-file FILE] [--admin NAME]...`.
//!
//! The world lives in `DIR` (default `./world`) and is created there if it does not exist
//! yet, with `--seed` and, given `--no-flight`, in survival mode; an existing world keeps
//! its own seed and mode. It is saved every 30 seconds and when `stop` is typed. Anything
//! else typed is run as a command, as an admin standing at spawn (`time set noon`,
//! `weather storm`, `tp Ada 0 100 0`), or said to everyone with `say …`.
//!
//! Names are not authenticated, so an `--admin` name is an admin only when connecting from
//! this machine (a tunnel that forwards players from elsewhere makes them all look local, so
//! leave `--admin` out behind one). From anywhere, a player whose client sends the server's
//! admin token is an admin: the token is read from `$VOXMAR_ADMIN_TOKEN` or the first line
//! of `--admin-token-file`, never the command line (which other users of the machine can
//! see), and players give it with `VOXMAR_ADMIN_TOKENS=host:port=TOKEN` before starting the
//! game, so it goes to this server only. Anyone else is made one with `op NAME`, typed here
//! or by an admin in game, until they leave.

use std::net::SocketAddr;

use voxmar_protocol::{DEFAULT_PORT, tcp};
use voxmar_server::{ServerConfig, assets_dir, load_content, spawn};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let mut config = ServerConfig { world: Some("world".into()), ..ServerConfig::default() };
    config.admin_token = std::env::var("VOXMAR_ADMIN_TOKEN").ok().filter(|token| !token.is_empty());
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
            "--admin-token-file" => {
                let path = value()?;
                let text = std::fs::read_to_string(&path).map_err(|e| format!("could not read the admin token from {path}: {e}"))?;
                config.admin_token = text.lines().next().map(|line| line.trim().to_owned()).filter(|token| !token.is_empty());
                if config.admin_token.is_none() {
                    return Err(format!("{path} holds no admin token").into());
                }
            }
            "--max-players" => config.max_players = value()?.parse()?,
            _ => return Err(format!("unknown argument {arg}").into()),
        }
    }

    let registry = load_content(&assets_dir())?;
    let server = spawn(config, registry)?;
    let addr = tcp::listen(bind, server.connections())?;
    tracing::info!("listening on {addr}; type a command (`help` lists them), `say …` or `stop` to save and quit");
    for line in std::io::stdin().lines() {
        match line?.trim() {
            "stop" => {
                // Dropping the server saves the world.
                drop(server);
                return Ok(());
            }
            "" => {}
            line => server.console(line),
        }
    }
    // Without a console, run until stopped from outside.
    loop {
        std::thread::park();
    }
}
