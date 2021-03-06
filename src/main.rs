#[macro_use] extern crate lazy_static;
#[macro_use] extern crate log;
#[macro_use] extern crate matches;
#[macro_use] extern crate quick_error;
#[macro_use] extern crate scoped_tls;
#[macro_use] extern crate serde_derive;
#[macro_use] extern crate serde_json;
extern crate abstract_ns;
extern crate argparse;
extern crate async_slot;
extern crate blake2;
extern crate byteorder;
extern crate crossbeam;
extern crate digest;
extern crate digest_writer;
extern crate env_logger;
extern crate futures;
extern crate futures_cpupool;
extern crate generic_array;
extern crate httpdate;
extern crate httpbin;
extern crate http_file_headers;
extern crate humantime;
extern crate libc;
extern crate libcantal;
extern crate mime_guess;
extern crate netbuf;
extern crate ns_std_threaded;
extern crate ns_router;
extern crate owning_ref;
extern crate quire;
extern crate rand;
extern crate regex;
extern crate self_meter_http;
extern crate serde;
extern crate slab;
extern crate string_intern;
extern crate time;
extern crate tk_bufstream;
extern crate tk_http;
extern crate tk_listen;
extern crate tk_pool;
extern crate tokio_core;
extern crate tokio_io;
extern crate trimmer;
extern crate typenum;
extern crate void;
mod authorizers;
mod base64;
mod chat;
mod config;
mod default_error_page;
mod handlers;
mod http_pools;  // TODO(tailhook) move to proxy?
mod incoming;
mod intern;
mod logging;
mod metrics;
mod privileges;
mod proxy;
mod request_id;
mod routing;
mod runtime;
mod startup;
mod template;
mod updater;

use std::env;
use std::io::{self, Write};
use std::process::exit;

use futures::stream::Stream;
use argparse::{ArgumentParser, Parse, StoreTrue, Print};
use tokio_core::reactor::Core;


pub fn main() {
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "warn");
    }
    env_logger::init();

    let mut config = String::from("/etc/swindon/main.yaml");
    let mut check = false;
    let mut verbose = false;
    {
        let mut ap = ArgumentParser::new();
        ap.set_description("Runs a web server");
        ap.refer(&mut config)
          .add_option(&["-c", "--config"], Parse,
            "Configuration file name")
          .metavar("FILE");
        ap.refer(&mut check)
          .add_option(&["-C", "--check-config"], StoreTrue,
            "Check configuration file and exit");
        ap.add_option(&["--version"],
            Print(env!("CARGO_PKG_VERSION").to_string()),
            "Show version");
        ap.refer(&mut verbose)
            .add_option(&["--verbose"], StoreTrue,
            "Print some user-friendly startup messages. \
             With --check-config prints config fingerprint.");
        ap.parse_args_or_exit();
    }

    let configurator = match config::Configurator::new(&config) {
        Ok(cfg) => cfg,
        Err(e) => {
            writeln!(&mut io::stderr(), "{}", e).ok();
            exit(1);
        }
    };
    let cfg = configurator.config();

    if check {
        if verbose {
            println!("Config fingerprint: {}", cfg.fingerprint());
        }
        exit(0);
    }

    request_id::with_generator(|| {
        let mut lp = Core::new().unwrap();
        let uhandle = lp.handle();
        let mut loop_state = startup::populate_loop(
            &lp.handle(), &cfg, verbose);

        let mut guard = Some(metrics::start(&loop_state.runtime)
            .map_err(|e| warn!("Error exporting metrics: {}", e)));

        match privileges::drop(&cfg.get()) {
            Ok(()) => {}
            Err(e) => {
                writeln!(&mut io::stderr(),
                    "Can't drop privileges: {}", e).ok();
                exit(2);
            }
        };
        let rx = updater::update_thread(configurator);
        lp.run(rx.for_each(move |()| {
                warn!("Updated config: {}", cfg.fingerprint());
                startup::update_loop(&mut loop_state, &cfg, &uhandle);
                drop(guard.take());
                guard = Some(metrics::start(&loop_state.runtime)
                    .map_err(|e| warn!("Error exporting metrics: {}", e)));
                Ok(())
            })).unwrap();
    });
}
