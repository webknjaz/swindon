//! This is a thread that processes websocket messages
//!
//! Important things about this thread:
//!
//! 1. It's fully driven by input message
//! 2. No timers here
//! 3. No time management here, only accept time value from events (*)
//!
//! This allows much better unit tests

use std::time::Instant;
use std::sync::Arc;
use std::collections::HashMap;
use std::fmt;

use rustc_serialize::json::Json;
use rustc_serialize::{Encodable, Encoder};
use futures::sync::mpsc::{UnboundedSender as Sender};

use config;
use intern::{Topic, SessionId, SessionPoolName, Lattice as Namespace};
use intern::LatticeKey;
use chat::Cid;
use chat::message::Meta;
use chat::error::MessageError;
use websocket::CloseReason;

mod main;
mod pool;
mod public;
mod session;
mod heap;
mod try_iter;  // temporary
mod connection;
mod lattice;

pub use self::public::{Processor, ProcessorPool};
pub use self::lattice::Delta;


pub struct Event {
    pool: SessionPoolName,
    timestamp: Instant,
    action: Action,
}

#[derive(Debug)]
pub enum ConnectionMessage {
    /// Topic publish message:
    /// `["message", {"topic": topic}, data]`
    Publish(Topic, Arc<Json>),
    /// Auth response message:
    /// `["hello", {}, json_data]`
    Hello(Arc<Json>),
    /// Websocket call result;
    Result(Meta, Json),
    /// Lattice update message
    Lattice(Namespace, Arc<HashMap<LatticeKey, lattice::Values>>),
    /// Error response to websocket call
    Error(Meta, MessageError),
    /// Force websocket stop
    StopSocket(CloseReason),
}

#[derive(Debug)]
pub enum PoolMessage {
    InactiveSession {
        session_id: SessionId,
        // This is mostly for debugging for now
        connections_active: usize,
        metadata: Arc<Json>,
    },
}

pub enum Action {

    // ------ Session pool management ------
    //   For all actions session pool name is passed in event structure
    NewSessionPool {
        config: Arc<config::SessionPool>,
        channel: Sender<PoolMessage>,
    },
    StopSessionPool,

    // ------ Connection management ------
    NewConnection {
        conn_id: Cid,
        channel: Sender<ConnectionMessage>,
    },
    Associate {
        conn_id: Cid,
        session_id: SessionId,
        metadata: Arc<Json>
    },
    UpdateActivity {
        conn_id: Cid,
        // We receive duration from client, but we expect request handling
        // code to validate and normalize it for us
        timestamp: Instant,
    },
    Disconnect {
        conn_id: Cid,
    },

    // ------ Subscriptions ------
    Subscribe {
        conn_id: Cid,
        topic: Topic,
    },
    Unsubscribe {
        conn_id: Cid,
        topic: Topic,
    },
    Publish {
        topic: Topic,
        data: Arc<Json>,
    },

    // ------ Lattices ------
    /// Attaches (subscribes to) lattice for this user
    ///
    /// Sends current lattice data to this connection immediately
    ///
    /// Note: data must *already* be in there
    Attach {
        namespace: Namespace,
        conn_id: Cid,
    },
    /// Updates data in lattice
    ///
    /// This works both for initial attach (subscription) of lattice and
    /// for subsequent updates
    ///
    /// Note: this message must be sent *before* Attach when connection
    /// initially attaches to the lattice
    Lattice {
        namespace: Namespace,
        delta: lattice::Delta,
    },
    Detach {
        namespace: Namespace,
        conn_id: Cid,
    },
}

impl Encodable for ConnectionMessage {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error>
    {
        use self::ConnectionMessage::*;
        match *self {
            Publish(ref topic, ref json) => {
                s.emit_seq(3, |s| {
                    #[derive(RustcEncodable)]
                    struct Meta<'a> {
                        topic: &'a Topic,
                    }
                    s.emit_seq_elt(0, |s| s.emit_str("message"))?;
                    s.emit_seq_elt(1, |s| {
                        Meta { topic: topic }.encode(s)
                    })?;
                    s.emit_seq_elt(2, |s| json.encode(s))
                })
            }
            Hello(ref json) => {
                s.emit_seq(3, |s| {
                    s.emit_seq_elt(0, |s| s.emit_str("hello"))?;
                    s.emit_seq_elt(1, |s| s.emit_map(0, |_| Ok(())))?;
                    s.emit_seq_elt(2, |s| json.encode(s))
                })
            }
            Lattice(ref namespace, ref json) => {
                s.emit_seq(3, |s| {
                    #[derive(RustcEncodable)]
                    struct Meta<'a> {
                        namespace: &'a Namespace,
                    }
                    s.emit_seq_elt(0, |s| s.emit_str("lattice"))?;
                    s.emit_seq_elt(1, |s| {
                        Meta { namespace: namespace }.encode(s)
                    })?;
                    s.emit_seq_elt(2, |s| json.encode(s))
                })
            }
            Result(ref meta, ref json) => {
                s.emit_seq(3, |s| {
                    s.emit_seq_elt(0, |s| s.emit_str("result"))?;
                    s.emit_seq_elt(1, |s| meta.encode(s))?;
                    s.emit_seq_elt(2, |s| json.encode(s))
                })
            }
            Error(ref meta, ref err) => {
                s.emit_seq(3, |s| {
                    s.emit_seq_elt(0, |s| s.emit_str("error"))?;
                    s.emit_seq_elt(1, |s| meta.encode(s))?;
                    s.emit_seq_elt(2, |s| err.encode(s))
                })
            }
            StopSocket(ref reason) => {
                s.emit_seq(2, |s| {
                    s.emit_seq_elt(0, |s| s.emit_str("stop"))?;
                    s.emit_seq_elt(1, |s| reason.encode(s))
                })
            }
        }
    }
}

// NOTE: UnboundSender does not derive from Debug.
impl fmt::Debug for Action {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use self::Action::*;
        match self {
            &NewSessionPool {..} => {
                write!(f, "Action::NewSessionPool")
            }
            &StopSessionPool => {
                write!(f, "Action::StopSessionPool")
            }
            &NewConnection { ref conn_id, .. } => {
                write!(f, "Action::NewConnection({:?})", conn_id)
            }
            &Associate { ref conn_id, ref session_id, .. } => {
                write!(f, "Action::Associate({:?}, {:?})", conn_id, session_id)
            }
            &UpdateActivity { ref conn_id, .. } => {
                write!(f, "Action::UpdateActivity({:?})", conn_id)
            }
            &Disconnect { ref conn_id } => {
                write!(f, "Action::Disconnect({:?})", conn_id)
            }
            &Subscribe { ref conn_id, ref topic } => {
                write!(f, "Action::Subscribe({:?}, {:?})", conn_id, topic)
            }
            &Unsubscribe { ref conn_id, ref topic } => {
                write!(f, "Action::Unsubscribe({:?}, {:?})", conn_id, topic)
            }
            &Publish { ref topic, .. } => {
                write!(f, "Action::Publish({:?})", topic)
            }
            &Attach { ref conn_id, ref namespace } => {
                write!(f, "Action::Attach({:?}, {:?})", conn_id, namespace)
            }
            &Lattice { ref namespace, .. } => {
                write!(f, "Action::Lattice({:?})", namespace)
            }
            &Detach { ref conn_id, ref namespace } => {
                write!(f, "Action::Detach({:?}, {:?})", conn_id, namespace)
            }
        }
    }
}
