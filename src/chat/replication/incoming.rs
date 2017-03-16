use std::str;
use std::ascii::AsciiExt;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use futures::{Stream, Future, Async};
use futures::future::{ok};
use futures::sync::mpsc::unbounded;
use tokio_core::reactor::Handle;
use tokio_io::{AsyncRead, AsyncWrite};
use tk_http::Status;
use tk_http::server::{Dispatcher, Head, Error, Codec, RecvMode, Encoder};
use tk_http::websocket::{Accept, Loop, ServerCodec, Config as WsConfig};
use tk_bufstream::{WriteBuf, ReadBuf};

use incoming::{Request, Reply, Transport};
use runtime::{RuntimeId};
use super::spawn::Handler;
use super::watcher::Watcher;


/// Incoming requests dispatcher.
pub struct Incoming {
    watcher: Arc<RwLock<Watcher>>,
    handle: Handle,
    remote_addr: SocketAddr,
}

struct WebsocketCodec {
    watcher: Arc<RwLock<Watcher>>,
    handle: Handle,
    accept: Accept,
    remote_addr: SocketAddr,
    remote_id: RuntimeId,
}

impl Incoming {

    pub fn new(addr: SocketAddr, watcher: &Arc<RwLock<Watcher>>,
        handle: &Handle)
        -> Incoming
    {
        Incoming {
            watcher: watcher.clone(),
            remote_addr: addr,
            handle: handle.clone(),
        }
    }

    fn parse_remote_id(&self, headers: &Head) -> Option<RuntimeId>
    {
        headers.all_headers().iter()
        .find(|h| h.name.eq_ignore_ascii_case("X-Swindon-Node-Id"))
        .and_then(|h| str::from_utf8(h.value).ok())
        .and_then(|s| RuntimeId::from_str(s))
    }
}


impl<S: Transport> Dispatcher<S> for Incoming {
    type Codec = Request<S>;

    fn headers_received(&mut self, headers: &Head)
        -> Result<Self::Codec, Error>
    {
        if let Some("/v1/swindon-chat") = headers.path() {
            if let Ok(Some(ws)) = headers.get_websocket_upgrade() {
                if let Some(remote_id) = self.parse_remote_id(headers)
                {
                    Ok(Box::new(WebsocketCodec {
                        watcher: self.watcher.clone(),
                        accept: ws.accept,
                        remote_id: remote_id,
                        remote_addr: self.remote_addr,
                        handle: self.handle.clone(),
                    }))
                } else {
                    Ok(error_reply(Status::BadRequest))
                }
            } else {
                Ok(error_reply(Status::BadRequest))
            }
        } else {
            Ok(error_reply(Status::NotFound))
        }
    }
}

impl<S: AsyncRead + AsyncWrite + 'static> Codec<S> for WebsocketCodec {
    type ResponseFuture = Reply<S>;

    fn recv_mode(&mut self) -> RecvMode {
        RecvMode::hijack()
    }

    fn data_received(&mut self, _data: &[u8], _end: bool)
        -> Result<Async<usize>, Error>
    {
        unreachable!()
    }

    fn start_response(&mut self, mut e: Encoder<S>) -> Reply<S> {
        e.status(Status::SwitchingProtocol);
        e.add_header("Connection", "upgrade");
        e.add_header("Upgrade", "websocket");
        e.format_header("Sec-Websocket-Accept", &self.accept);
        // e.format_header("X-Swindon-Node-Id", &self.links_manager.node_id());
        e.done_headers().unwrap();
        Box::new(ok(e.done()))
    }

    fn hijack(&mut self, write_buf: WriteBuf<S>, read_buf: ReadBuf<S>) {
        let out = write_buf.framed(ServerCodec);
        let inp = read_buf.framed(ServerCodec);
        let wcfg = WsConfig::new().done();

        let (tx, rx) = unbounded();
        let rx = rx.map_err(|e| format!("receive error: {:?}", e));
        let tx = self.watcher.write().expect("acquired for write")
        .attach(self.remote_addr, tx);

        self.handle.spawn(
            Loop::server(out, inp, rx, Handler(tx), &wcfg)
            .map_err(|e| error!("Websocket loop error: {:?}", e))
        );
    }
}

// Shortcut for error replies

fn error_reply<S: 'static>(status: Status) -> Request<S> {
    Box::new(QuickReply(Some(status)))
}

struct QuickReply(Option<Status>);

impl<S: 'static> Codec<S> for QuickReply {
    type ResponseFuture = Reply<S>;
    fn recv_mode(&mut self) -> RecvMode {
        RecvMode::buffered_upfront(0)
    }
    fn data_received(&mut self, data: &[u8], end: bool)
        -> Result<Async<usize>, Error>
    {
        assert!(end);
        assert!(data.len() == 0);
        Ok(Async::Ready(0))
    }
    fn start_response(&mut self, mut e: Encoder<S>) -> Reply<S> {
        e.status(self.0.take().expect("start response called once"));
        e.add_length(0);
        e.done_headers().unwrap();
        Box::new(ok(e.done()))
    }
}
