use futures::sync::mpsc::UnboundedSender;

use chat::processor::Action;
use chat::ProcessorPool;

/// Upstream messages router.
///
/// Checks action and sends it to:
/// * only local processor;
/// * only to remote;
/// * to both;
struct Router {
    processor: ProcessorPool,
    remote: UnboundedSender<Action>, // define receiver
    // remote: ReplicationManager, ???
}

impl Router {

    pub fn send(&mut self, action: Action) {
        match action {
            // if action has conn_id => check if its remote and send only to
            //  remote channel
            // otherwise send to both processor and remote
            _ => {}
        }
    }
}
