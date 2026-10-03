//! Dedicated server: `voxmar-server [--bind ADDR] [--seed N] [--view-distance N] [--no-flight]`.

use std::net::SocketAddr;

use voxmar_protocol::{DEFAULT_PORT, tcp};
use voxmar_server::{ServerConfig, assets_dir, load_content, spawn};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let mut config = ServerConfig::default();
    let mut bind: SocketAddr = ([0, 0, 0, 0], DEFAULT_PORT).into();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--bind" => bind = value()?.parse()?,
            "--seed" => config.seed = value()?.parse()?,
            "--view-distance" => config.view_distance = value()?.parse()?,
            "--no-flight" => config.allow_flight = false,
            _ => return Err(format!("unknown argument {arg}").into()),
        }
    }

    let registry = load_content(&assets_dir())?;
    let server = spawn(config, registry)?;
    let addr = tcp::listen(bind, server.connections())?;
    tracing::info!("listening on {addr}");
    loop {
        std::thread::park();
    }
}
