use std::path::PathBuf;
use std::collections::HashMap;

use quire::validate::{Nothing, Enum, Structure, Scalar, Mapping};

use intern::DiskPoolName;


#[derive(RustcDecodable, Debug, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum Mode {
    relative_to_domain_root,
    relative_to_route,
}


#[derive(RustcDecodable, Debug, PartialEq, Eq)]
pub struct Static {
    pub mode: Mode,
    pub path: PathBuf,
    pub text_charset: Option<String>,
    pub pool: DiskPoolName,
    pub extra_headers: HashMap<String, String>,
}

#[derive(RustcDecodable, Debug, PartialEq, Eq)]
pub struct SingleFile {
    pub path: PathBuf,
    pub content_type: String,
    pub pool: DiskPoolName,
    pub extra_headers: HashMap<String, String>,
}

pub fn validator<'x>() -> Structure<'x> {
    Structure::new()
    .member("mode", Enum::new()
        .option("relative_to_domain_root", Nothing)
        .option("relative_to_route", Nothing)
        .allow_plain()
        .plain_default("relative_to_route"))
    .member("path", Scalar::new())
    .member("text_charset", Scalar::new().optional())
    .member("pool", Scalar::new().default("default"))
    .member("extra_headers", Mapping::new(Scalar::new(), Scalar::new()))
}

pub fn single_file<'x>() -> Structure<'x> {
    Structure::new()
    .member("path", Scalar::new())
    .member("content_type", Scalar::new())
    .member("pool", Scalar::new().default("default"))
    .member("extra_headers", Mapping::new(Scalar::new(), Scalar::new()))
}
