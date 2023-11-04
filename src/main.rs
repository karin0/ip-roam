#[macro_use]
extern crate log;

use std::net::Ipv4Addr;
use std::pin::pin;
use std::{env, io};

use futures::stream::StreamExt;
use ip_roam::{Address, Addresses, Connection};

use app::App;

mod app;

fn parse_addr(am: &Address, if_name: &str) -> Option<Ipv4Addr> {
    if am.label() == if_name {
        Some(*am.addr())
    } else {
        None
    }
}

async fn in_zone(addresses: Addresses, if_name: &str) -> bool {
    let mut addrs = pin!(addresses.stream());
    while let Some(am) = addrs.next().await {
        if let Some(addr) = parse_addr(&am, if_name) {
            info!("{}: {}", if_name, addr);
            return true;
        }
    }
    false
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "info");
    }
    if env::var("PRETTY_ENV_LOGGER_COMPACT").is_err() {
        env::set_var("PRETTY_ENV_LOGGER_COMPACT", "1");
    }
    pretty_env_logger::init_timed();

    let app = App::new();
    let if_name = &app.if_name;

    let c = Connection::new()?.spawn();
    if in_zone(c.addresses, if_name).await {
        app.enter_zone().await;
    } else {
        app.exit_zone().await;
    }

    let mut msgs = pin!(c.monitor.stream());
    while let Some(msg) = msgs.next().await {
        let am = msg.addr();
        if let Some(addr) = parse_addr(am, if_name) {
            if msg.is_new() {
                info!("new: {}: {}", if_name, addr);
                if app.in_zone(addr) {
                    app.enter_zone().await;
                }
            } else {
                info!("del: {}: {}", if_name, addr);
                if app.in_zone(addr) {
                    app.exit_zone().await;
                }
            }
        }
    }
    Err(io::Error::from(io::ErrorKind::ConnectionAborted))
}
