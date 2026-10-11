//! Two start-up refusals proven at the composition root, by starting the
//! real binary: `QIP_DEPLOYMENT=local` on a process that can see it is
//! deployed, and a deployed node that does not say whether it runs alone.
//!
//! `qip-core`'s and `standby.rs`'s own tests prove the rules. Neither proves
//! that `main` hands them what it observed — a `parse_observed` called with
//! no markers, or a `StandbySettings::read` called with `local` hard-coded,
//! would pass every one of those tests while every deployed node took
//! `local` on its word. So these start the process.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // integration tests may unwrap: a panic is the failure report

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// `EX_CONFIG`, what the binary exits with on a configuration it refuses.
const EX_CONFIG: i32 = 78;

fn scratch(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "qip-edge-node-deployment-{}-{test}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// The binary with everything it refuses to start without and nothing
/// inherited, so neither the developer's shell nor the CI runner's
/// environment decides what this asserts. (The DMI product name is the
/// host's and cannot be cleared; no CI this runs on is a Compute Engine
/// guest, and the premise below would fail loudly on one rather than let
/// the refusal tests pass for the wrong reason.)
fn node(test: &str) -> Command {
    let dir = scratch(test);
    let key = dir.join("capital-envelope-key");
    std::fs::write(&key, "a-cell-envelope-key-for-tests").expect("a key file");
    let journal = dir.join("journal");
    std::fs::create_dir_all(&journal).expect("a journal directory");

    let mut command = Command::new(env!("CARGO_BIN_EXE_qip-edge-node"));
    command
        .env_clear()
        .env("QIP_CELL_ID", "london-1")
        .env("QIP_CELL_REGION", "europe-west2")
        .env("QIP_DEPLOYMENT", "local")
        .env("QIP_VENUES", "CX")
        .env("QIP_REGION_ALLOCATION", "1000000")
        .env("QIP_CAPITAL_ENVELOPE_KEY_FILE", &key)
        .env("QIP_STORAGE_TARGET", "engine")
        .env("QIP_STORAGE_ROOT", &journal)
        .env("QIP_HEALTH_PORT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// Run a configuration the binary must refuse, bounded so that one it
/// accepts fails the test instead of hanging the build.
fn refused(mut command: Command) -> (Output, String) {
    let mut child = command.spawn().expect("the binary starts");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        match child.try_wait().expect("the child can be polled") {
            Some(_) => break,
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "the binary was still running twenty seconds after being given a \
                     configuration it must refuse at start-up"
                );
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    let output = child.wait_with_output().expect("the exit is readable");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    (output, stderr)
}

/// Whether the process printed a line containing `wanted` before exiting
/// or within the wait.
fn announces(child: &mut Child, wanted: &str) -> bool {
    let stdout = child.stdout.take().expect("standard output is piped");
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if send.send(line).is_err() {
                break;
            }
        }
    });
    while let Ok(line) = receive.recv_timeout(Duration::from_secs(30)) {
        if line.contains(wanted) {
            return true;
        }
    }
    false
}

/// The failure this prevents: `gcloud compute instances add-metadata`, or a
/// Cloud Run environment edit, setting `QIP_DEPLOYMENT=local` re-admitted
/// the seed-derived key and plaintext links on a deployed node. Cloud Run's
/// `K_SERVICE` stands in for the platform here because it is the one marker
/// a test can set; the GCE product name is read from the same observation.
/// Mutation: pass `&DeploymentMarkers::none()` to `parse_observed` in
/// `NodeConfig::from_env` and the marked process starts; this fails.
#[test]
fn a_node_that_can_see_it_is_deployed_refuses_local_by_name() {
    // Premise: the same environment with no marker starts.
    let mut child = node("unmarked").spawn().expect("the binary starts");
    let started = announces(&mut child, "trust root");
    let _ = child.kill();
    let _ = child.wait();
    assert!(started, "the unmarked local node did not start");

    let mut command = node("marked");
    command.env("K_SERVICE", "qip-edge-node");
    let (output, stderr) = refused(command);
    assert_eq!(output.status.code(), Some(EX_CONFIG), "{stderr}");
    assert!(
        stderr.contains("QIP_DEPLOYMENT=local and this process can see it is deployed")
            && stderr.contains("K_SERVICE"),
        "the refusal does not name what it saw: {stderr}"
    );
}

/// The failure this prevents: a standby booted from the primary's startup
/// script, with no lease and no role, and traded as a second primary.
/// Mutation: pass `Deployment::Local` to `StandbySettings::read` in
/// `NodeConfig::from_env` and the role-less deployed node gets past the
/// check; this fails.
#[test]
fn a_deployed_node_that_does_not_say_it_runs_alone_refuses_to_start() {
    let mut command = node("roleless");
    command.env("QIP_DEPLOYMENT", "dev");
    let (output, stderr) = refused(command);
    assert_eq!(output.status.code(), Some(EX_CONFIG), "{stderr}");
    assert!(
        stderr.contains("QIP_NODE_ROLE is not set"),
        "a role-less deployed node was refused for something else, or not at all: {stderr}"
    );
}
