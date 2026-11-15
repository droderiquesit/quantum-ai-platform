//! `qip-edge-node replay` over a spool the shipped `run()` wrote
//! (R12-TICK-10184).
//!
//! The in-process replay tests build their own session; none proved that the
//! binary's recorded spool is one its own subcommand can read back. A recorder
//! and a replayer that were each right alone and wrong together would pass
//! every one of them.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const CELL: &str = "cell-replay-sub";

fn scratch(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "qip-edge-node-replay-sub-{}-{test}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

fn node(dir: &std::path::Path, subcommand: Option<&[&str]>) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_qip-edge-node"));
    let key = dir.join("capital-envelope-key");
    std::fs::write(&key, "a-cell-envelope-key-for-tests").unwrap();
    command
        .env_clear()
        .env("QIP_CELL_ID", CELL)
        .env("QIP_CELL_REGION", "europe-west2")
        .env("QIP_DEPLOYMENT", "local")
        .env("QIP_VENUES", "CX")
        .env("QIP_REGION_ALLOCATION", "1000000")
        .env("QIP_CAPITAL_ENVELOPE_KEY_FILE", &key)
        .env("QIP_STORAGE_TARGET", "engine")
        .env("QIP_STORAGE_ROOT", dir.join("journal"))
        .env("QIP_HEALTH_PORT", "0")
        .env(qip_edge_node::feed::FEED_VARIABLE, "simulated")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(arguments) = subcommand {
        command.args(arguments);
    }
    command
}

/// Run the shipped node until its spool holds a pass marker, then kill it: the
/// spool a deployed node leaves behind, torn tail and all.
fn spool_of_a_run(dir: &std::path::Path) -> PathBuf {
    std::fs::create_dir_all(dir.join("journal")).unwrap();
    let mut child = node(dir, None).spawn().expect("the binary starts");
    let spool = dir.join("journal").join("spool");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(
            child.try_wait().unwrap().is_none(),
            "the node exited before it spooled a pass"
        );
        let has_segment =
            qip_edge_node::replay::RecordedSession::from_spool_directory(&spool).is_ok();
        if has_segment {
            break;
        }
        assert!(Instant::now() < deadline, "the node spooled nothing in 30s");
        std::thread::sleep(Duration::from_millis(25));
    }
    let _ = child.kill();
    let _ = child.wait();
    spool
}

fn replay(dir: &std::path::Path, spool: &std::path::Path, venues: &str) -> std::process::Output {
    let spool = spool.to_string_lossy().to_string();
    node(dir, Some(&["replay", &spool]))
        .env("QIP_VENUES", venues)
        .output()
        .expect("replay runs")
}

/// Mutation: in `replay_command`/`replay_once`, build the cell from a
/// configuration other than `cell_configuration()` (or drop the pass markers
/// from the spool read) and the verdict stops being REPRODUCED.
#[test]
fn the_replay_subcommand_reproduces_a_spool_the_running_node_wrote() {
    let dir = scratch("reproduces");
    let spool = spool_of_a_run(&dir);
    let out = replay(&dir, &spool, "CX");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "replay exited {:?}: {stdout} {stderr}",
        out.status
    );
    assert!(stdout.contains("REPRODUCED"), "{stdout}");
    // Premise: it replayed at least one recorded pass, so REPRODUCED is not
    // the verdict on an empty session.
    let passes: u64 = stdout
        .split("recorded passes")
        .next()
        .and_then(|head| head.rsplit(": ").next())
        .and_then(|n| n.trim().parse().ok())
        .expect("the report names how many passes it replayed");
    assert!(passes >= 1, "{stdout}");
}

/// A replay under a different configuration is a different node: it is
/// refused rather than reported as reproducing the recorded one.
#[test]
fn the_replay_subcommand_refuses_a_configuration_the_spool_was_not_recorded_under() {
    let dir = scratch("refuses");
    let spool = spool_of_a_run(&dir);
    let out = replay(&dir, &spool, "CX,CY");
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.status.success(),
        "a different configuration replayed: {all}"
    );
    assert!(!all.contains("REPRODUCED"), "{all}");
}
