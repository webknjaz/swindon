use std::sync::Arc;
use std::net::SocketAddr;

use minihttp::server::Head;
use tokio_core::reactor::Handle;

use config::Config;
use runtime::Runtime;
use incoming::{Debug, IntoContext};


pub struct Input<'a> {
    pub addr: SocketAddr,
    pub runtime: &'a Arc<Runtime>,
    pub config: &'a Arc<Config>,
    pub debug: Debug,
    pub headers: &'a Head<'a>,
    pub prefix: &'a str,
    pub suffix: &'a str,
    pub handle: &'a Handle,
}

impl<'a> IntoContext for Input<'a> {
    fn into_context(self) -> (Arc<Config>, Debug) {
        (self.config.clone(), self.debug)
    }
}