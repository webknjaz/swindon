use std::sync::Arc;
use std::path::PathBuf;
use std::os::unix::io::AsRawFd;

use futures::{BoxFuture, Future, Async, finished};
use tokio_core::io::Io;
use tokio_core::reactor::{Handle, Remote};
use tokio_service::Service;
use tk_bufstream::IoBuf;
use minihttp::{Error, GenericResponse, ResponseWriter, Status, Request};
use minihttp::enums::Method;
use minihttp::client::HttpClient;
use httpbin::HttpBin;
use rustc_serialize::json::Json;

use config::{Config, EmptyGif};
use config::static_files::{Static, SingleFile};

use response::DebugInfo;
use default_error_page::write_error_page;
use handlers::{files, empty_gif, proxy};
use handlers::proxy::ProxyCall;
use chat;
use websocket;
use {Pickler};


pub struct Serializer {
    config: Arc<Config>,
    debug: DebugInfo,
    response: Response,
    handle: Remote,
    request: Option<Request>,
}

pub enum Response {
    ErrorPage(Status),
    EmptyGif(Arc<EmptyGif>),
    HttpBin,
    Static {
        path: PathBuf,
        settings: Arc<Static>,
    },
    SingleFile(Arc<SingleFile>),
    WebsocketEcho(websocket::Init),
    WebsocketChat(chat::ChatInit),
    Proxy(ProxyCall),
}

impl Response {
    pub fn serve(self, req: Request, cfg: Arc<Config>,
                 debug: DebugInfo, handle: &Handle,
                 http_client: &HttpClient)
        -> BoxFuture<Serializer, Error>
    {
        use self::Response::*;
        use super::chat::ChatInit::*;
        match self {
            Proxy(ProxyCall::Prepare{ hostport, settings}) => {
                let handle = handle.remote().clone();

                let mut client = http_client.clone();
                proxy::prepare(req, hostport, settings, &mut client);

                client.done()
                    .map_err(|e| e.into())
                    .and_then(move |resp| {
                        let resp = Response::Proxy(ProxyCall::Ready {
                            response: resp,
                        });
                        finished(Serializer {
                            config: cfg,
                            debug: debug,
                            response: resp,
                            handle: handle,
                            request: None,
                        })
                    }).boxed()
            }
            WebsocketChat(Prepare(init, router)) => {
                let remote = handle.remote().clone();
                let client = http_client.clone();

                let url = router.get_url("tangle.authorize_connection".into());
                let http_cookies = req.headers.iter()
                    .filter(|&&(ref k, _)| k == "Cookie")
                    .map(|&(_, ref v)| v.clone())
                    .collect::<String>();
                let http_auth = req.headers.iter()
                    .find(|&&(ref k, _)| k == "Authorization")
                    .map(|&(_, ref v)| v.clone())
                    .unwrap_or("".to_string());

                let mut data = chat::Kwargs::new();
                // TODO: parse cookie string to hashmap;
                data.insert("http_cookie".into(),
                    Json::String(http_cookies));
                data.insert("http_authorization".into(),
                    Json::String(http_auth));
                let payload = chat::Message::Auth(data).encode();

                let mut auth = http_client.clone();
                auth.request(Method::Post, url.as_str());
                // TODO: write auth message with cookies
                //  get request's headers (cookie & authorization)
                //  TODO: connection with id;
                auth.add_header("Content-Type".into(), "application/json");
                auth.add_length(payload.as_bytes().len() as u64);
                auth.done_headers();
                auth.write_body(payload.as_bytes());
                auth.done()
                .map_err(|e| e.into())
                .and_then(move |resp| {
                    let resp = if resp.status == Status::Ok {
                        match chat::parse_userinfo(resp.status, resp.body) {
                            userinfo @ chat::Message::Hello(_) => {
                                WebsocketChat(
                                    Ready(init, client, router, userinfo))
                            }
                            other => {
                                WebsocketChat(AuthError(init, other))
                            }
                        }
                    } else {
                        Response::ErrorPage(Status::InternalServerError)
                    };
                    finished(Serializer {
                        config: cfg,
                        debug: debug,
                        response: resp,
                        handle: remote,
                        request: None,
                    })
                })
                .boxed()
            }
            _ => {
                finished(Serializer {
                    config: cfg,
                    debug: debug,
                    response: self,
                    handle: handle.remote().clone(),
                    request: Some(req), // only needed for HttpBin
                }).boxed()
            }
        }
    }
}

impl<S: Io + AsRawFd + Send + 'static> GenericResponse<S> for Serializer {
    type Future = BoxFuture<IoBuf<S>, Error>;
    fn into_serializer(mut self, writer: ResponseWriter<S>) -> Self::Future {
        use super::chat::ChatInit::*;
        let writer = Pickler(writer, self.config, self.debug);
        match self.response {
            Response::ErrorPage(status) => {
                write_error_page(status, writer).done().boxed()
            }
            Response::EmptyGif(cfg) => {
                empty_gif::serve(writer, cfg)
            }
            Response::HttpBin => {
                // TODO(tailhook) it's not very good idea to unpack the future
                // this way
                match HttpBin::new().call(self.request.take().unwrap()).poll()
                {
                    Ok(Async::Ready(gen_response)) => {
                        gen_response.into_serializer(writer.0).boxed()
                    }
                    _ => unreachable!(),
                }
            }
            Response::Static { path, settings } => {
                files::serve(writer, path, settings)
            }
            Response::SingleFile(settings) => {
                files::serve_file(writer, settings)
            }
            Response::WebsocketEcho(init) => {
                websocket::negotiate(writer, init, self.handle,
                    websocket::Kind::Echo)
            }
            Response::WebsocketChat(Ready(init, client, router, userinfo)) => {
                chat::negotiate(writer, init, self.handle, client,
                    router, userinfo)
            }
            Response::WebsocketChat(_) => {
                write_error_page(Status::BadRequest, writer).done().boxed()
            }
            Response::Proxy(ProxyCall::Ready { response }) => {
                proxy::serialize(writer, response)
            }
            Response::Proxy(_) => {
                write_error_page(Status::BadRequest, writer)
                .done().boxed()
            }
        }
    }
}
