use futures::stream::{StreamExt, TryStreamExt};
use netlink_packet_core::NetlinkPayload;
use netlink_packet_route::{
    rtnl::{address::Nla, RtnlMessage::*},
    AddressMessage,
};
use netlink_proto::sys::{AsyncSocket, SocketAddr};
use reqwest::{header, Client, ClientBuilder, Url};
use rtnetlink::{constants::*, new_connection, AddressHandle};
use serde::Deserialize;
use std::net::Ipv4Addr;
use std::{env, io};

#[macro_use]
extern crate log;

fn parse_addr(am: AddressMessage, if_name: &str) -> Option<Ipv4Addr> {
    let mut addr = None;
    let mut found_if = false;
    for nla in am.nlas {
        match nla {
            Nla::Address(a) => {
                if a.len() == 4 {
                    let c: [u8; 4] = match a.try_into() {
                        Ok(c) => c,
                        _ => continue,
                    };
                    addr = Some(Ipv4Addr::from(c));
                    if found_if {
                        return addr;
                    }
                }
            }
            Nla::Label(l) => {
                if l != if_name {
                    return None;
                }
                if addr.is_some() {
                    return addr;
                }
                found_if = true;
            }
            _ => {}
        }
    }
    None
}

#[derive(Debug, Clone, Deserialize)]
struct Config {
    #[serde(alias = "interface")]
    if_name: String,
    #[serde(alias = "external-controller")]
    api: String,
    secret: String,
    ip_min: Ipv4Addr,
    ip_max: Ipv4Addr,
    proxy_in: String,
    proxy_out: String,
    selector: String,
}

#[derive(Debug, Clone)]
struct App {
    if_name: String,
    url: Url,
    http: Client,
    ip_min: Ipv4Addr,
    ip_max: Ipv4Addr,
    proxy_in: String,
    proxy_out: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ClashStatus {
    now: String,
}

fn get_config_path() -> Option<String> {
    let mut args = env::args().skip(1);
    if let Some(s) = args.next() {
        if s == "-c" {
            return args.next();
        }
    }
    None
}

impl App {
    fn new() -> Self {
        let config = if let Some(path) = get_config_path() {
            std::fs::read_to_string(path)
        } else {
            std::fs::read_to_string("config.yaml")
        };
        let config = match config {
            Ok(c) => c,
            Err(e) => {
                if e.kind() == io::ErrorKind::NotFound {
                    panic!(
                        "Usage: {} [-c <config-file = config.yaml>]",
                        env::args().next().unwrap()
                    );
                }
                panic!("read config: {}", e);
            }
        };
        let mut conf: Config = serde_yaml::from_str(&config).unwrap();
        let secret = std::mem::replace(&mut conf.secret, "***".to_string());
        info!("config: {:?}", conf);

        let mut h = header::HeaderMap::new();
        let auth = format!("Bearer {}", secret);
        h.append(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(&auth).unwrap(),
        );
        let http = ClientBuilder::new().default_headers(h).build().unwrap();
        let url = format!("http://{}/proxies/{}", conf.api, conf.selector);
        Self {
            if_name: conf.if_name,
            url: Url::parse(&url).unwrap(),
            http,
            ip_min: conf.ip_min,
            ip_max: conf.ip_max,
            proxy_in: conf.proxy_in,
            proxy_out: conf.proxy_out,
        }
    }

    fn in_zone(&self, addr: Ipv4Addr) -> bool {
        addr >= self.ip_min && addr < self.ip_max
    }

    async fn clash_get(&self) -> reqwest::Result<String> {
        let r: ClashStatus = self
            .http
            .get(self.url.as_ref())
            .send()
            .await?
            .json()
            .await?;
        Ok(r.now)
    }

    async fn clash_put(&self, proxy: &str) -> reqwest::Result<()> {
        let body = format!(r#"{{"name":"{}"}}"#, proxy);
        self.http
            .put(self.url.as_ref())
            .body(body)
            .send()
            .await?
            .error_for_status_ref()?;
        Ok(())
    }

    async fn _enter_zone(&self) -> reqwest::Result<()> {
        let now = self.clash_get().await?;
        if now != self.proxy_in {
            self.clash_put(&self.proxy_in).await?;
            info!("enter_zone: {} -> {}", now, self.proxy_in);
        } else {
            warn!("enter_zone: already {}", now);
        }
        Ok(())
    }

    async fn _exit_zone(&self) -> reqwest::Result<()> {
        let now = self.clash_get().await?;
        if now == self.proxy_in {
            self.clash_put(&self.proxy_out).await?;
            info!("exit_zone: {} -> {}", now, self.proxy_out);
        } else {
            warn!("exit_zone: already {}", now);
        }
        Ok(())
    }

    async fn enter_zone(&self) {
        if let Err(e) = self._enter_zone().await {
            error!("enter_zone: {:?}", e);
        }
    }

    async fn exit_zone(&self) {
        if let Err(e) = self._exit_zone().await {
            error!("exit_zone: {:?}", e);
        }
    }
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

    let (mut conn, handle, mut messages) = new_connection().unwrap();

    let addr = SocketAddr::new(0, RTMGRP_IPV4_IFADDR);
    conn.socket_mut().socket_mut().bind(&addr).unwrap();
    tokio::spawn(conn);

    let mut addrs = AddressHandle::new(handle).get().execute().into_stream();
    let mut in_zone = false;
    while let Some(am) = addrs.next().await {
        if let Ok(am) = am {
            if let Some(addr) = parse_addr(am, if_name) {
                info!("{}: {}", if_name, addr);
                if app.in_zone(addr) {
                    app.enter_zone().await;
                    in_zone = true;
                    break;
                }
            }
        }
    }
    drop(addrs);
    if !in_zone {
        app.exit_zone().await;
    }

    while let Some((message, _)) = messages.next().await {
        if let NetlinkPayload::InnerMessage(m) = message.payload {
            match m {
                NewAddress(am) => {
                    if let Some(addr) = parse_addr(am, if_name) {
                        info!("new: {}: {}", if_name, addr);
                        if app.in_zone(addr) {
                            app.enter_zone().await;
                        }
                    }
                }
                DelAddress(am) => {
                    if let Some(addr) = parse_addr(am, if_name) {
                        info!("del: {}: {}", if_name, addr);
                        if app.in_zone(addr) {
                            app.exit_zone().await;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    error!("netlink socket closed");
    Err(io::Error::new(
        io::ErrorKind::ConnectionAborted,
        "netlink socket closed",
    ))
}
