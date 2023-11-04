use futures::channel::mpsc::UnboundedReceiver;
use futures::{
    stream::{StreamExt, TryStreamExt},
    Stream,
};
use netlink_packet_core::{NetlinkMessage, NetlinkPayload};
use netlink_packet_route::{
    rtnl::{address::Nla, RtnlMessage::*},
    AddressMessage, RtnlMessage,
};
use netlink_proto::{
    sys::{AsyncSocket, SocketAddr},
    Connection as RtConnection,
};
use rtnetlink::{constants::*, new_connection, AddressHandle, Handle as RtHandle};
use std::io::{Error, ErrorKind, Result};
use std::net::Ipv4Addr;

/// A retrieved address entry.
#[derive(Debug, Clone)]
pub struct Address {
    addr: Ipv4Addr,
    label: String,
}

impl Address {
    /// Gets the IPv4 address.
    pub fn addr(&self) -> &Ipv4Addr {
        &self.addr
    }

    /// Gets the label of the interface.
    pub fn label(&self) -> &str {
        &self.label
    }
}

impl TryFrom<AddressMessage> for Address {
    type Error = Error;

    fn try_from(am: AddressMessage) -> Result<Address> {
        let mut the_addr = None;
        let mut the_label = None;
        for nla in am.nlas {
            match nla {
                Nla::Address(a) => {
                    let c: [u8; 4] = match a.try_into() {
                        Ok(c) => c,
                        _ => continue,
                    };
                    let addr = Ipv4Addr::from(c);
                    if let Some(label) = the_label {
                        return Ok(Address { addr, label });
                    }
                    the_addr = Some(addr);
                }
                Nla::Label(label) => {
                    if let Some(addr) = the_addr {
                        return Ok(Address { addr, label });
                    }
                    the_label = Some(label);
                }
                _ => {}
            }
        }
        Err(Error::from(ErrorKind::NotFound))
    }
}

/// A handle to get current local addresses.
#[derive(Debug, Clone)]
pub struct Addresses {
    handle: RtHandle,
}

impl Addresses {
    /// Streams the current local addresses.
    pub fn stream(self) -> impl Stream<Item = Address> {
        let inner = AddressHandle::new(self.handle)
            .get()
            .execute()
            .into_stream();
        inner.filter_map(|item| async move { item.ok().and_then(|am| am.try_into().ok()) })
    }
}

/// A message from the monitor, denoting a new or deleted address.
#[derive(Debug, Clone)]
pub struct Message {
    addr: Address,
    new: bool,
}

impl Message {
    fn new(addr: Address, new: bool) -> Self {
        Message { addr, new }
    }

    /// Gets the address.
    pub fn addr(&self) -> &Address {
        &self.addr
    }

    /// Checks whether the address is new or deleted.
    pub fn is_new(&self) -> bool {
        self.new
    }
}

impl TryFrom<RtnlMessage> for Message {
    type Error = Error;

    fn try_from(item: RtnlMessage) -> Result<Message> {
        Ok(match item {
            NewAddress(a) => Message::new(a.try_into()?, true),
            DelAddress(a) => Message::new(a.try_into()?, false),
            _ => {
                return Err(Error::from(ErrorKind::InvalidData));
            }
        })
    }
}

impl TryFrom<NetlinkMessage<RtnlMessage>> for Message {
    type Error = Error;

    fn try_from(item: NetlinkMessage<RtnlMessage>) -> Result<Message> {
        if let NetlinkPayload::InnerMessage(m) = item.payload {
            m.try_into()
        } else {
            Err(Error::from(ErrorKind::InvalidData))
        }
    }
}

/// A monitor to watch the changes of local addresses.
#[derive(Debug)]
pub struct Monitor {
    messages: UnboundedReceiver<(NetlinkMessage<RtnlMessage>, SocketAddr)>,
}

impl Monitor {
    /// Streams the monitor messages.
    pub fn stream(self) -> impl Stream<Item = Message> {
        self.messages
            .filter_map(|item| async { item.0.try_into().ok() })
    }
}

/// Handles to get the current local addresses and their changes.
pub struct Handle {
    pub addresses: Addresses,
    pub monitor: Monitor,
}

/// A pending connection to the netlink socket.
pub struct Connection {
    pub conn: RtConnection<RtnlMessage>,
    /// The `conn` future must be spawned before the `handle` could work.
    pub handle: Handle,
}

impl Connection {
    /// Creates a pending connection to the netlink socket.
    pub fn new() -> Result<Self> {
        let (mut conn, handle, messages) = new_connection()?;
        conn.socket_mut()
            .socket_mut()
            .bind(&SocketAddr::new(0, RTMGRP_IPV4_IFADDR))?;
        Ok(Connection {
            conn,
            handle: Handle {
                addresses: Addresses { handle },
                monitor: Monitor { messages },
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Connection;
    use futures::stream::StreamExt;

    #[tokio::test]
    async fn has_loopback() {
        let c = Connection::new().unwrap();
        let rt = tokio::spawn(c.conn);
        let s = c.handle.addresses.stream();
        let r = s.any(|m| async move { m.addr.is_loopback() }).await;
        assert!(r);
        rt.abort();
    }
}
