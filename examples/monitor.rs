use futures::StreamExt;
use ip_roam::Connection;
use std::io::{Error, ErrorKind, Result};
use std::pin::pin;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let c = Connection::new()?.spawn();

    let mut s = pin!(c.addresses.stream());
    while let Some(addr) = s.next().await {
        println!("current: {:?}", addr);
    }

    let mut s = pin!(c.monitor.stream());
    while let Some(item) = s.next().await {
        println!("monitor: {:?}", item);
    }

    Err(Error::from(ErrorKind::ConnectionAborted))
}
