//! Event fabric composition root's entry point.
//!
//! ADR 0100 assigns this binary the event-fabric broker's role: sole writer
//! of every partition's batch chain, running its own clock for segment roll
//! and archive (ADR 0100 §2). `qip_fabricd::start` is the composition and
//! holds the order every composition root here holds to — configuration
//! refused first, storage proven writable before a port is bound, serving
//! last (`.claude/rules/architecture/00-boundaries.md`). This file reads the
//! environment, calls it, and stops the process naming the reason when it
//! refuses.
//!
//! This binary refused to start outright until its `config`, `health` and
//! `archiver` modules existed, because serving on a configuration nobody
//! validated is the failure that order exists to close. It now starts only
//! on a configuration with no defaults in it. It is still not deployed:
//! `docs/adr/0010-what-gets-deployed.md` keeps it out of the image matrix
//! and every workload catalogue until its placement is decided (ADR 0099
//! C8).

use std::sync::Arc;

use qip_core::SystemClock;
use qip_fabricd::config::Config;

fn main() {
    let started = Config::from_environment()
        .and_then(|config| qip_fabricd::start(&config, Arc::new(SystemClock)));
    match started {
        Ok(running) => {
            eprintln!(
                "qip-fabricd: serving the event-fabric protocol on {} and health on {}",
                running.address(),
                running.health_address()
            );
            running.wait();
        }
        Err(error) => {
            eprintln!("qip-fabricd: refusing to start. {error}");
            std::process::exit(1);
        }
    }
}
