use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::collections::HashMap;
use std::time::{Instant, Duration};
use futures::{Future, Stream};
use futures::future::{ok, FutureResult};
use futures::stream::{self, IterStream};
use futures::sync::mpsc::{unbounded, UnboundedSender, UnboundedReceiver};
use futures::sync::oneshot::Receiver;
use tokio_core::net::TcpStream;
use tokio_core::reactor::{Handle, Interval};
use tk_http::websocket::{Dispatcher, Frame, Error, Packet};

use config::{ListenSocket};
use config::Replication;
use runtime::{Runtime, RuntimeId};

use super::client::spawn_connect;
use super::server::listener;

#[derive(Debug)]
pub struct Link {
    state: State,
    // TODO: add/implement next fields:
    // replication_offset: usize, // ??
    // last_reply: Instant,

    // subscriptions: // vec of TXs of HashMap<Name, Tx>
}

#[derive(Debug, Clone)]
pub enum State {
    /// Initial state when peer has just been added to peers map.
    Void,
    /// Connect started at
    Connecting(Instant),
    /// Connected to peer / Received connection from peer
    Connected(UnboundedSender<Packet>),
}

struct LinksData {
    // we propagate with initial node addresses;
    // then we either wait for connections or connect to known peers

    /// Runtime Id
    pub node_id: RuntimeId,

    /// Outgoing and incoming links.
    ///
    /// Updated from either config or with received connections.
    // Probably use only IP not IP:Port pair
    // as incoming connections only has same IP;
    // alternatively, use RuntimeId
    peers: HashMap<SocketAddr, Link>,

    // Add HashMap<Name, (recv, Vec<senders>)>
    //  for each message in named channel forward it to sender

    /// Shared output channel.
    tx: UnboundedSender<Packet>,

    // /// Incomming proto
    // rx: UnboundedReceiver<Packet>,

    // TODO: add Receiver of Processor Actions.
}

#[derive(Clone)]
pub struct LinksManager(Arc<RwLock<LinksData>>);  // XXX looks ugly

pub struct Handler(UnboundedSender<Packet>, RuntimeId);


impl LinksManager {
    
    pub fn new(node_id: RuntimeId, tx: UnboundedSender<Packet>) -> LinksManager {
        // TODO: pass Processors here;
        //  create channel and spawn rx forwarder into processor pools;

        // parse and send rx events to proper ProcessorPool;
        //
        let data = Arc::new(RwLock::new(LinksData {
            node_id: node_id,
            peers: HashMap::new(),
            tx: tx,
            // rx: rx,
        }));
        LinksManager(data)
    }

    /// Update peers inplace and schedule reconnection.
    pub fn update(&mut self, cfg: &Replication)
    {
        // TODO: read cfg.peers and update self.peers
        // TODO: delete/disconnect removed peers
        let mut data = self.0.write().expect("acquired for update");
        for sock in &cfg.peers {
            match sock {
                &ListenSocket::Tcp(addr) => {
                    // TODO: if exist:
                    //  check state and probably mark for reconnect.
                    data.peers.entry(addr).or_insert_with(|| Link::new());
                }
            }
        }
    }

    /// Spawn listener.
    pub fn listener(&self, addr: SocketAddr, runtime: &Arc<Runtime>,
        handle: &Handle, shutter: Receiver<()>)
        -> Result<(), io::Error>
    {
        listener(addr, self, runtime, handle, shutter)
    }

    /// Runs links management loop (basically links reconnection).
    ///
    /// iterate over known links, find unconnected links (State::Void)
    /// spawn connection task;
    /// find stale links (no reply for some time)
    pub fn reconnect(&mut self, handle: &Handle)
    {
        let data = self.0.read().expect("acquired for read");
        let man2 = self.clone();
        for (addr, link) in data.peers.iter() {
            match link.state {
                State::Void => {}
                _ => continue,
            };
            spawn_connect(addr.clone(), &man2, handle);
        }
    }

    /// Attache/replace sender channed for specfied remote peer
    /// identified with remote address and remote id.
    pub fn attach(&mut self, remote_id: RuntimeId, addr: SocketAddr,
        tx: UnboundedSender<Packet>)
        -> Handler
    {
        let mut data = self.0.write().expect("acquired for update");
        data.peers.entry(addr).or_insert_with(|| Link::new())
        .attach(tx);
        Handler(data.tx.clone(), remote_id)
    }

    pub fn node_id(&self) -> RuntimeId {
        self.0.read().expect("acquired for read").node_id
    }

}

impl Link {
    fn new() -> Link {
        Link { state: State::Void }
    }

    fn attach(&mut self, tx: UnboundedSender<Packet>) {
        let state = match self.state {
            _ => { State::Connected(tx) }
        };
        self.state = state;
    }

    // XXX use protocol message
    fn publish(&self, data: String) {
        match self.state {
            State::Connected(ref tx) => {
                tx.send(Packet::Text(data));
            }
            _ => {
                error!("Invalid state");
            }
        };
    }
}

impl Dispatcher for Handler {
    type Future = FutureResult<(), Error>;

    fn frame (&mut self, frame: &Frame) -> Self::Future {
        // TODO: transform frame into protocol message
        println!("Got frame: {:?}", frame);
        ok(())
    }
}


enum ReplicationState {
    /// Instance has just started and has no shared state.
    Void,

    /// Instance is replicating state from remote
    /// and in a middle of a progress.
    Warmup,

    /// Instance is following its peers.
    Streaming,
}
