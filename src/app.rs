use std::net::Ipv4Addr;
use std::{env, io};

use reqwest::{header, Client, ClientBuilder, Url};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    ip_min: Ipv4Addr,
    ip_max: Ipv4Addr,
    proxy_in: String,
    proxy_out: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Config {
    #[serde(alias = "interface")]
    if_name: String,
    #[serde(alias = "external-controller")]
    api: String,
    secret: String,
    selector: String,
    rules: Vec<Rule>,
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

impl Rule {
    pub(crate) fn has(&self, addr: Ipv4Addr) -> bool {
        addr >= self.ip_min && addr < self.ip_max
    }
}

#[derive(Debug, Clone)]
pub struct App {
    pub(crate) if_name: String,
    url: Url,
    http: Client,
    rules: Vec<Rule>,
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
        if conf.rules.is_empty() {
            panic!("no rules");
        }
        conf.rules.shrink_to_fit();

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
            rules: conf.rules,
        }
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

    async fn _enter_zone(&self, rule: &Rule) -> reqwest::Result<bool> {
        let now = self.clash_get().await?;
        Ok(if now != rule.proxy_in {
            self.clash_put(&rule.proxy_in).await?;
            info!("enter: {} -> {}", now, rule.proxy_in);
            true
        } else {
            warn!("enter: already {}", now);
            false
        })
    }

    async fn _exit_zone(&self, rule: &Rule) -> reqwest::Result<bool> {
        let now = self.clash_get().await?;
        Ok(if now == rule.proxy_in {
            self.clash_put(&rule.proxy_out).await?;
            info!("exit: {} -> {}", now, rule.proxy_out);
            true
        } else {
            warn!("exit: already {}", now);
            false
        })
    }

    async fn _handle_zone(&self, rule: &Rule, enter: bool) -> bool {
        let r = if enter {
            self._enter_zone(rule).await
        } else {
            self._exit_zone(rule).await
        };

        match r {
            Ok(r) => r,
            Err(e) => {
                error!("{}: {:?}", if enter { "enter" } else { "exit" }, e);
                false
            }
        }
    }

    pub(crate) async fn notify(&self, addr: Ipv4Addr, enter: bool) -> bool {
        for rule in &self.rules {
            if rule.has(addr) && self._handle_zone(rule, enter).await {
                return true;
            }
        }
        false
    }

    pub(crate) async fn fallback(&self) {
        self._handle_zone(&self.rules[0], false).await;
    }
}
