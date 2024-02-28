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
pub struct Site {
    selector: String,
    rules: Vec<Rule>,
}

#[derive(Debug, Clone, Deserialize)]
struct Config {
    #[serde(alias = "interface")]
    if_name: String,
    #[serde(alias = "external-controller")]
    api: String,
    secret: String,
    sites: Vec<Site>,
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
    api: String,
    http: Client,
    sites: Vec<Site>,
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

        if conf.sites.is_empty() {
            panic!("no sites");
        }
        conf.sites.shrink_to_fit();
        for site in &mut conf.sites {
            if site.rules.is_empty() {
                panic!("site {}: no rules", site.selector);
            }
            site.rules.shrink_to_fit();
        }

        let mut h = header::HeaderMap::new();
        let auth = format!("Bearer {}", secret);
        h.append(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(&auth).unwrap(),
        );
        let http = ClientBuilder::new().default_headers(h).build().unwrap();
        Self {
            if_name: conf.if_name,
            api: conf.api,
            http,
            sites: conf.sites,
        }
    }

    fn url(&self, site: &Site) -> Url {
        let url = format!("http://{}/proxies/{}", self.api, site.selector);
        Url::parse(&url).unwrap()
    }

    async fn clash_get(&self, site: &Site) -> reqwest::Result<String> {
        let r: ClashStatus = self.http.get(self.url(site)).send().await?.json().await?;
        Ok(r.now)
    }

    async fn clash_put(&self, site: &Site, proxy: &str) -> reqwest::Result<()> {
        let body = format!(r#"{{"name":"{}"}}"#, proxy);
        self.http
            .put(self.url(site))
            .body(body)
            .send()
            .await?
            .error_for_status_ref()?;
        Ok(())
    }

    async fn _enter_zone(&self, site: &Site, rule: &Rule) -> reqwest::Result<()> {
        let now = self.clash_get(site).await?;
        if now != rule.proxy_in {
            self.clash_put(site, &rule.proxy_in).await?;
            info!("enter: {}: {} -> {}", site.selector, now, rule.proxy_in);
        } else {
            warn!("enter: {}: already {}", site.selector, now);
        }
        Ok(())
    }

    async fn _exit_zone(&self, site: &Site, rule: &Rule) -> reqwest::Result<()> {
        let now = self.clash_get(site).await?;
        if now == rule.proxy_in {
            self.clash_put(site, &rule.proxy_out).await?;
            info!("exit: {}: {} -> {}", site.selector, now, rule.proxy_out);
        } else {
            warn!("exit: {}: already {}", site.selector, now);
        }
        Ok(())
    }

    async fn _handle_zone(&self, site: &Site, rule: &Rule, enter: bool) {
        let r = if enter {
            self._enter_zone(site, rule).await
        } else {
            self._exit_zone(site, rule).await
        };
        if let Err(e) = r {
            error!(
                "{}: {}: {:?}",
                if enter { "enter" } else { "exit" },
                site.selector,
                e
            );
        }
    }

    async fn _notify(&self, site: &Site, addr: Ipv4Addr, enter: bool) -> bool {
        for rule in &site.rules {
            if rule.has(addr) {
                self._handle_zone(site, rule, enter).await;
                return true;
            }
        }
        false
    }

    pub(crate) async fn notify(&self, addr: Ipv4Addr, enter: bool) {
        for site in &self.sites {
            self._notify(site, addr, enter).await;
        }
    }

    pub(crate) async fn initialize(&self, addr: Ipv4Addr) {
        for site in &self.sites {
            if !self._notify(site, addr, true).await {
                self._handle_zone(site, &site.rules[0], false).await;
            }
        }
    }
}
