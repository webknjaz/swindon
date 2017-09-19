use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::fs::{File, Metadata, metadata};
use std::io::{self, Read};
use std::path::{PathBuf, Path, Component};
use std::rc::Rc;
use std::sync::Arc;

use quire::{self, Pos, Include, ErrorCollector, Options, parse_config};
use quire::{raw_parse as parse_yaml};
use quire::ast::{Ast, process as process_ast};

use config::root::{ConfigData, Mixin, config_validator, mixin_validator};
use super::Handler;
use config::static_files::Mode;
use config::log;
use intern::{LogFormatName};


quick_error! {
    #[derive(Debug)]
    pub enum Error {
        Io(err: io::Error) {
            display("IO error: {}", err)
            description("IO error")
            from()
        }
        Config(err: quire::ErrorList) {
            display("config error: {}", err)
            description("config error")
            from()
        }
        Validation(err: String) {
            display("validation error: {}", err)
            description("validation error")
            from()
        }
        BadMixinPath(filename: PathBuf) {
            display("bad mixin path: {:?}, \
                only relative paths are allowed", filename)
            description("bad mixin path")
        }
        InvalidPrefixInMixin(prefix: String, filename: PathBuf,
                             typ: &'static str, name: String)
        {
            display("mixin {:?} has {} named {:?} without prefix {:?}",
                filename, typ, name, prefix)
            description("item in mixin file has invalid prefix")
        }
        MixinConflict(filename: PathBuf, typ: &'static str, name: String) {
            display("mixin {:?} has overlapping {} {:?}",
                filename, typ, name)
            description("mixin has overlapping item")
        }
    }
}

macro_rules! err {
    // Shortcut to config post-load validation error
    ($msg:expr, $($a:expr),*) => (
        return Err(format!($msg, $($a),*).into())
    )
}

fn join_filename(base: &Path, relative: &Path) -> Result<PathBuf, ()> {
    let mut path = PathBuf::from(&*base);
    path.pop(); // pop original filename
    for component in relative.components() {
        match component {
            Component::Normal(x) => path.push(x),
            _ => {
                return Err(());
            }
        }
    }
    Ok(path)
}

#[allow(dead_code)]
pub fn include_file(files: &RefCell<&mut Vec<(PathBuf, String, Metadata)>>,
    pos: &Pos, include: &Include,
    err: &ErrorCollector, options: &Options)
    -> Ast
{
    match *include {
        Include::File { filename } => {
            let path = match join_filename(&Path::new(&**pos.filename),
                                           &Path::new(filename))
            {
                Ok(path) => path,
                Err(()) => {
                    // TODO(tailhook) should this error exist?
                    err.add_error(quire::Error::preprocess_error(pos,
                        format!("Only relative paths without parent \
                                 directories can be included")));
                    return Ast::void(pos);
                }
            };

            debug!("{} Including {:?}", pos, path);

            let mut body = String::new();
            File::open(&path)
            .and_then(|mut f| {
                let m = f.metadata();
                f.read_to_string(&mut body)?;
                m
            })
            .map_err(|e| {
                err.add_error(quire::Error::open_error(&path, e))
            }).ok()
            .and_then(|metadata| {
                files.borrow_mut().push((
                    path.to_path_buf(),
                    String::from(filename),
                    metadata,
                ));
                parse_yaml(Rc::new(path.display().to_string()), &body,
                    |doc| { process_ast(&options, doc, err) },
                ).map_err(|e| err.add_error(e)).ok()
            })
            .unwrap_or_else(|| Ast::void(pos))
        }
    }
}

fn prefix_error<N: fmt::Display>(prefix: &str, filename: &Path,
    typ: &'static str, name: &N)
    -> Error
{
    Error::InvalidPrefixInMixin(prefix.to_string(), filename.to_path_buf(),
        typ, name.to_string())
}

fn conflict<N: fmt::Display>(filename: &Path, typ: &'static str, name: N)
    -> Error
{
    Error::MixinConflict(filename.to_path_buf(), typ, name.to_string())
}

fn mix_in<K, V>(
    filename: &Path, prefix: &str,
    dest: &mut HashMap<K, V>, src: HashMap<K, V>,
    typ: &'static str)
    -> Result<(), Error>
    where K: ::std::hash::Hash + ::std::ops::Deref<Target=str>,
          K: ::std::fmt::Display + Eq,
{
    for (h, handler) in src {
        if !(&*h).starts_with(prefix) {
            return Err(prefix_error(prefix, filename, typ, &h));
        }
        if dest.contains_key(&h) {
            return Err(conflict(filename, typ, &h));
        }
        dest.insert(h, handler);
    }
    Ok(())
}

pub fn read_config<P: AsRef<Path>>(filename: P)
    -> Result<(ConfigData, Vec<(PathBuf, String, Metadata)>), Error>
{
    let filename = filename.as_ref();
    let mut files = Vec::new();
    files.push((
        filename.to_path_buf(),
        String::from("<main>"),
        metadata(filename)?,
    ));
    let mut cfg: ConfigData = {
        let cell = RefCell::new(&mut files);
        let mut opt = Options::default();
        opt.allow_include(
            |a, b, c, d| include_file(&cell, a, b, c, d));
        parse_config(filename, &config_validator(), &opt)?
    };

    for (prefix, ref incl) in &cfg.mixins {
        let incl_path = join_filename(filename, &Path::new(incl))
            .map_err(|()| Error::BadMixinPath(filename.to_path_buf()))?;
        files.push((
            incl_path.clone(),
            incl.display().to_string(),
            metadata(&incl_path)?,
        ));

        let mixin: Mixin = {
            let cell = RefCell::new(&mut files);
            let mut opt = Options::default();
            opt.allow_include(
                |a, b, c, d| include_file(&cell, a, b, c, d));
            parse_config(&incl_path, &mixin_validator(), &opt)?
        };
        mix_in(&incl_path, prefix,
               &mut cfg.handlers, mixin.handlers, "handler")?;
        mix_in(&incl_path, prefix,
               &mut cfg.authorizers, mixin.authorizers, "authorizer")?;
        mix_in(&incl_path, prefix,
               &mut cfg.session_pools, mixin.session_pools, "session-pool")?;
        mix_in(&incl_path, prefix, &mut cfg.http_destinations,
            mixin.http_destinations, "http-destination")?;
        mix_in(&incl_path, prefix, &mut cfg.ldap_destinations,
            mixin.ldap_destinations, "ldap-destination")?;
        mix_in(&incl_path, prefix,
            &mut cfg.networks, mixin.networks, "network")?;
        mix_in(&incl_path, prefix,
            &mut cfg.log_formats, mixin.log_formats, "log-format")?;
        mix_in(&incl_path, prefix,
            &mut cfg.disk_pools, mixin.disk_pools, "disk-pools")?;
    }

    // Set some defaults
    if !cfg.log_formats.contains_key("debug-log") {
        cfg.log_formats.insert(LogFormatName::from("debug-log"),
            log::Format::from_string(r#"
                {{ request.client_ip }}
                {{ request.host }}
                "{{ request.method }}
                {{ request.path }}
                {{ request.version }}"
                {{ response.status_code }}
            "#.into()).expect("can always compile debug log"));
    }

    // Extra config validations

    for &(ref domain, ref sub) in cfg.routing.hosts() {
        for (path, route) in sub {
            if cfg.handlers.get(&route.destination).is_none() {
                err!("Unknown handler {:?}", route.destination)
            }
            if path.as_ref().map(|x| x.ends_with("/")).unwrap_or(false) {
                err!("Path must not end with /: {:?} {:?}",
                     domain, path);
            }
            if let Some(&Handler::StripWWWRedirect) =
                cfg.handlers.get(&route.destination)
            {
                if !domain.matches_www() {
                    err!(concat!("Expected `www.` prefix for StripWWWRedirect",
                                 " handler route: {:?} {:?}"), domain, path);
                }
            }
        }
    }
    for (name, h) in &cfg.handlers {
        match h {
            &Handler::SwindonLattice(ref chat) => {
                match cfg.session_pools.get_mut(&chat.session_pool) {
                    None => {
                        err!("No session pool {:?} defined", chat.session_pool)
                    }
                    Some(mut pool) => {
                        let tangle = chat.use_tangle_prefix();
                        if pool.use_tangle_prefix.is_some() &&
                            pool.use_tangle_prefix != Some(tangle)
                        {
                            err!("Inconsistent `compatibility` for \
                                  pool {:?}", chat.session_pool);
                        }
                        Arc::make_mut(&mut pool)
                            .use_tangle_prefix = Some(tangle);

                        let dest = if tangle {
                            chat.message_handlers.resolve(
                                "tangle.session_inactive")
                        } else {
                            chat.message_handlers.resolve(
                                "swindon.session_inactive")
                        };

                        let tangle = chat.use_tangle_auth();
                        if pool.use_tangle_auth.is_some() &&
                            pool.use_tangle_auth != Some(tangle)
                        {
                            err!("Inconsistent `compatibility` for \
                                  pool {:?}", chat.session_pool);
                        }
                        Arc::make_mut(&mut pool)
                            .use_tangle_auth = Some(tangle);

                        let tangle = chat.weak_content_type();
                        if pool.weak_content_type.is_some() &&
                            pool.weak_content_type != Some(tangle)
                        {
                            err!("Inconsistent `compatibility` for \
                                  pool {:?}", chat.session_pool);
                        }
                        Arc::make_mut(&mut pool)
                            .weak_content_type = Some(tangle);

                        if !pool.inactivity_handlers.contains(dest) &&
                            pool.inactivity_handlers.len() != 0 {
                            err!(concat!(
                                "Inactivity destinations mismatch for",
                                 "{:?}: {:?}"), name, dest)
                        }
                    }
                }
                if let Some(h) = chat.http_route.as_ref() {
                    if !cfg.handlers.contains_key(h) {
                        err!("{:?}: unknown http route {:?}", name, h)
                    }
                }
                let u = &chat.message_handlers.default.upstream;
                if let Some(http_dest) = cfg.http_destinations.get(u) {
                    if http_dest.override_host_header.is_none() {
                        err!("http destination {:?} is used \
                             in message-handler of {:?}, so must contain \
                             override-host-header setting.", u, name);
                    }
                } else {
                    err!("{:?}: unknown http destination {:?}", name, u)
                }
                for (_, dest) in &chat.message_handlers.map {
                    if !cfg.http_destinations.contains_key(&dest.upstream) {
                        err!("{:?}: unknown http destination {:?}",
                             name, dest.upstream)
                    }
                }
            }
            &Handler::Proxy(ref proxy) => {
                let u = &proxy.destination.upstream;
                if !cfg.http_destinations.contains_key(u) {
                    err!("{:?}: unknown http destination {:?}", name, u)
                }
                if proxy.request_id_header.is_some() {
                    warn!(concat!(
                        "{:?}: request_id_header is deprecated",
                        " in !Proxy setting, it must be specified",
                        " in http destination"), name);
                }
            }
            &Handler::Static(ref config) => {
                if config.strip_host_suffix.is_some() &&
                   config.mode != Mode::with_hostname
                {
                    err!("{:?}: `strip-host-suffix` only \
                        works when `mode: with-hostname`", name);
                }
            }
            _ => {}
        }
    }
    // TODO: verify session_pool inactivity handlers
    for (name, s) in &cfg.session_pools {
        for dest in &s.inactivity_handlers {
            if !cfg.http_destinations.contains_key(&dest.upstream) {
                err!("{:?}: unknown http destination {:?}",
                     name, dest.upstream)
            }
        }
    }

    Ok((cfg, files))
}
