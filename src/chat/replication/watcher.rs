use std::sync::{Arc, RwLock};
use std::net::SocketAddr;
use std::collections::HashMap;
use tokio_core::reactor::Handle;
use futures::sync::mpsc::{unbounded, UnboundedSender};
use futures::sync::oneshot::{channel, Sender};
use tk_http::websocket::Packet;

use runtime::Runtime;
use config::{ListenSocket, Replication};
use super::spawn::listen;


type WebsocketChannel = UnboundedSender<Packet>;


pub struct ReplicationSession {
    data: Arc<RwLock<Watcher>>,
    shutters: Vec<Sender<()>>,
}

/// Connections watcher.
///
/// Updates, restarts connections to known peers.
pub struct Watcher {
    peers: HashMap<SocketAddr, State>,
    queue: WebsocketChannel,
}

enum State {
    Void,
    Connecting,
    Connected {
        tx: WebsocketChannel,
    },
}

impl Watcher {
    pub fn new() -> Watcher {
        let (tx, rx) = unbounded();
        Watcher {
            peers: HashMap::new(),
            queue: tx,
        }
    }

    pub fn update(&mut self, cfg: &Replication)
    {
        for sock in &cfg.peers {
            match sock {
                &ListenSocket::Tcp(addr) => {
                    self.peers.entry(addr).or_insert(State::Void);
                }
            }
        }
    }

    pub fn process(&mut self) {
        // check and update peers state (reconnect, etc);
        for (addr, state) in self.peers.iter() {
            match state {
                &State::Void => {
                    // TODO: spawn connect
                    // on connect call manager.attach()
                }
                _ => continue,
            }
        }
    }

    pub fn attach(&mut self, addr: SocketAddr, output: WebsocketChannel)
        -> WebsocketChannel
    {
        self.peers.insert(addr, State::Connected { tx: output });
        self.queue.clone()
    }
}

impl ReplicationSession {
    pub fn new() -> ReplicationSession {
        ReplicationSession {
            data: Arc::new(RwLock::new(Watcher::new())),
            shutters: Vec::new(),
        }
    }

    pub fn update(&mut self, cfg: &Replication, runtime: &Arc<Runtime>,
        handle: &Handle)
    {
        self.data.write().expect("acquired for write").update(cfg);
        //
        for addr in &cfg.listen {
            match *addr {
                ListenSocket::Tcp(addr) => {
                    let (tx, rx) = channel();
                    match listen(addr, &self.data, runtime, handle, rx) {
                        Ok(()) => {
                            //
                        }
                        Err(e) => {
                            error!("Error listening {}: {}. \
                                Will retry on next config reload", addr, e);
                        }
                    }
                }
            }
        }
    }
}

pub fn remote_channel(processor: Processor, handle: &Handle) -> UnboundedSender<> {
    let (tx, rx) = unbounded();
    handle.spawn(rx
    .for_each(move |(pool_name, action)| {
        processor.send(pool_name, action);
        Ok(())
    }));
}
