use std::net::Ipv4Addr;
use std::{env, io};

use reqwest::{header, Client, ClientBuilder, Url};
use serde::Deserialize;

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

#[derive(Debug, Clone, Deserialize)]
struct ClashStatus {
    now: String,
}

fn get_config_path() -> Option<String> {
    let mut args = env::args().skip(1);
    if let Some("-c") = args.next().as_deref() {
        return args.next();
    }
    None
}

#[derive(Debug, Clone)]
pub struct App {
    pub(crate) if_name: String,
    url: Url,
    http: Client,
    ip_min: Ipv4Addr,
    ip_max: Ipv4Addr,
    proxy_in: String,
    proxy_out: String,
}

impl App {
    pub(crate) fn new() -> Self {
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

    pub(crate) fn in_zone(&self, addr: Ipv4Addr) -> bool {
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

    pub(crate) async fn enter_zone(&self) {
        if let Err(e) = self._enter_zone().await {
            error!("enter_zone: {:?}", e);
        }
    }

    pub(crate) async fn exit_zone(&self) {
        if let Err(e) = self._exit_zone().await {
            error!("exit_zone: {:?}", e);
        }
    }
}
