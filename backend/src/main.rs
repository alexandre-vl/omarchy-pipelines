//! `omarchy-pipelines-helper` — the CI poller behind the `oma.pipelines`
//! Omarchy plugin.
//!
//! Speaks line-delimited JSON on stdin/stdout (see [`protocol`]) and is
//! started, supervised and stopped by `Service.qml`. It is not a general
//! purpose CLI: `--version` and `--help` exist so packaging and the release
//! workflow can identify a binary, and everything else is the shell's job.

mod auth;
mod cache;
mod engine;
mod http;
mod protocol;
mod provider;
mod ratelimit;
mod timefmt;

use std::io::BufRead;
use std::sync::mpsc;

use engine::Engine;
use protocol::Envelope;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--version" | "-V") => {
            println!("omarchy-pipelines-helper {VERSION}");
            return;
        }
        Some("--help" | "-h") => {
            println!(
                "omarchy-pipelines-helper {VERSION}\n\n\
                 Reads newline-delimited JSON commands on stdin and writes\n\
                 newline-delimited JSON events on stdout. Started by the\n\
                 oma.pipelines Omarchy plugin; not meant to be run by hand.\n\n\
                 Options:\n  \
                 -V, --version   print the version and exit\n  \
                 -h, --help      print this help and exit"
            );
            return;
        }
        Some(unknown) => {
            eprintln!("omarchy-pipelines-helper: unknown argument: {unknown}");
            std::process::exit(2);
        }
        None => {}
    }

    let (sender, receiver) = mpsc::channel::<Envelope>();

    // stdin gets its own thread so a slow poll cycle can never make the shell
    // block writing a command, and so a malformed line is dropped where it
    // arrives rather than stalling the engine.
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<Envelope>(trimmed) {
                Ok(envelope) => {
                    if sender.send(envelope).is_err() {
                        break;
                    }
                }
                // A command we cannot parse is reported and skipped. Exiting
                // would take the widget down over one bad line, and the input
                // is echoed through the redactor in case it held a token.
                Err(error) => eprintln!(
                    "omarchy-pipelines-helper: ignoring unparseable command: {}",
                    auth::redact(&error.to_string())
                ),
            }
        }
    });

    Engine::new(VERSION).run(&receiver);
}
