use quire::validate::{Structure, Sequence, Scalar};

use super::listen::{self, ListenSocket};


#[derive(Debug, RustcDecodable, PartialEq, Eq)]
pub struct Replication {
    pub listen: Vec<ListenSocket>,
    pub peers: Vec<ListenSocket>,
}

pub fn validator<'x>() -> Structure<'x> {
    Structure::new()
    .member("listen", Sequence::new(listen::validator()))
    .member("peers", Sequence::new(listen::validator()))
}
