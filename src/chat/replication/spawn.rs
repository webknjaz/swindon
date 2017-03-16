use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use futures::{Future, Stream};
use futures::future::{FutureResult, ok};
use futures::sync::oneshot::Receiver;
use futures::sync::mpsc::{unbounded, UnboundedSender};
use tokio_core::reactor::Handle;
use tokio_core::net::{TcpListener, TcpStream};
use tk_listen::ListenExt;
use tk_http::server::{Proto, Config};
use tk_http::websocket::{Config as WsConfig, Loop};
use tk_http::websocket::{Dispatcher, Frame, Error, Packet};
use tk_http::websocket::client::HandshakeProto;
use httparse::Header;

use runtime::{Runtime, RuntimeId};
use super::incoming::Incoming;
use super::outgoing::Authorizer;
use super::watcher::Watcher;


pub fn listen(addr: SocketAddr, watcher: &Arc<RwLock<Watcher>>,
    runtime: &Arc<Runtime>,
    handle: &Handle, shutter: Receiver<()>)
    -> Result<(), io::Error>
{
    let cfg = runtime.config.get();
    let hcfg = Config::new().done();    // TODO: configure;
    let w1 = watcher.clone();
    let h1 = handle.clone();

    let listener = TcpListener::bind(&addr, &handle)?;
    handle.spawn(listener.incoming()
        .sleep_on_error(*cfg.listen_error_timeout, &handle)
        .map(move |(socket, saddr)| {
            Proto::new(socket, &hcfg, Incoming::new(saddr, &w1, &h1), &h1)
            .map_err(|e| debug!("Http protocol error: {}", e))
        })
        .listen(cfg.max_connections)
        .select(shutter.map_err(|_| unreachable!()))
        .map(move |(_, _)| info!("Listener {} exited", addr))
        .map_err(move |(_, _)| info!("Listener {} exited", addr))
    );
    Ok(())
}

pub fn connect(addr: SocketAddr, watcher: &Arc<RwLock<Watcher>>, 
    runtime: &Arc<Runtime>, handle: &Handle)
{
    let wcfg = WsConfig::new().done();
    let w1 = watcher.clone();
    // let runtime_id = runtime.runtime_id;

    // TODO: add connect timeout;
    handle.spawn(TcpStream::connect(&addr, &handle)
    .map_err(|e| format!("Connect error: {}", e))
    .and_then(move |sock| {
        HandshakeProto::new(sock, Authorizer::new(addr))
        .map_err(|e| format!("WS auth error: {}", e))
    })
    .and_then(move |(out, inp, (addr, _))| {
        let (tx, rx) = unbounded();
        let rx = rx.map_err(|_| format!("receiver error"));
        // XXX;
        let tx = w1.write().expect("acquired for update").attach(addr, tx);
        Loop::client(out, inp, rx, Handler(tx), &wcfg)
        .map_err(|e| format!("WS loop error: {}", e))
    })
    .map_err(|e| error!("{}", e)));
}


pub struct Handler(pub UnboundedSender<Packet>);

impl Dispatcher for Handler {
    type Future = FutureResult<(), Error>;

    fn frame (&mut self, frame: &Frame) -> Self::Future {
        // TODO: transform frame into protocol message
        println!("Got frame: {:?}", frame);
        ok(())
    }
}
