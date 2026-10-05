//! MESH-010 at the composition root: the real binary reads the maximum leg
//! count from its environment, refuses a value it may not run under, and
//! says which maximum is in force.
//!
//! `tests/arbitrage.rs` proves the parser and the installer. Neither proves
//! that `main` calls them: a `parse_max_legs` nothing in the binary read
//! would pass every test there while every node ran the engine's default
//! whatever its deployment wrote. So these start the process.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const MAX_LEGS: &str = "QIP_ARBITRAGE_MAX_LEGS";
const STRATEGY: &str = "QIP_ARBITRAGE_STRATEGY";
/// `EX_CONFIG`, what the binary exits with on a configuration it refuses.
const EX_CONFIG: i32 = 78;

fn scratch(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "qip-edge-node-max-legs-{}-{test}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// The binary with everything it refuses to start without, and nothing
/// inherited: a variable set in the developer's shell must not be able to
/// decide what this asserts. The envelope key arrives as a file, the way a
/// deployment mounts it.
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

/// Run a configuration the binary is expected to refuse, and collect what it
/// said.
///
/// Bounded on purpose. A node that *accepts* the configuration serves for
/// ever, and waiting on it without a limit turns the regression this suite
/// exists to catch into a build that hangs instead of one that fails: when
/// the no-desk refusal was removed to prove this test, `Command::output`
/// sat for ten minutes. A process still alive at the bound is killed and
/// reported as the failure it is.
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

/// Read the process's standard output until `wanted` appears in a line, the
/// stream ends, or the wait runs out. Returns every line read.
fn stdout_until(child: &mut Child, wanted: &str) -> Vec<String> {
    let stdout = child.stdout.take().expect("standard output is piped");
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if send.send(line).is_err() {
                break;
            }
        }
    });
    let mut lines = Vec::new();
    while let Ok(line) = receive.recv_timeout(Duration::from_secs(30)) {
        let found = line.contains(wanted);
        lines.push(line);
        if found {
            break;
        }
    }
    lines
}

#[test]
fn the_binary_announces_the_configured_maximum_and_refuses_one_above_twenty_or_one_with_no_desk() {
    // Premise, and the positive half: with five legs configured and a desk
    // to hold to it, this same environment starts and says five. So the
    // refusals below are about the value, not about a fixture the binary
    // would refuse anyway.
    let mut command = node("announced");
    command.env(STRATEGY, "arbitrage-desk").env(MAX_LEGS, "5");
    let mut child = command.spawn().expect("the binary starts");
    let lines = stdout_until(&mut child, "arbitrage cycles of at most");
    let _ = child.kill();
    let _ = child.wait();
    let announced = lines
        .iter()
        .find(|line| line.contains("arbitrage cycles of at most"))
        .unwrap_or_else(|| panic!("the node never announced its leg limit; it said: {lines:#?}"));
    assert!(
        announced.contains("at most 5 legs") && announced.contains(MAX_LEGS),
        "{announced}"
    );

    // Unset is the engine's default of four, announced the same way.
    let mut command = node("default");
    command.env(STRATEGY, "arbitrage-desk");
    let mut child = command.spawn().expect("the binary starts");
    let lines = stdout_until(&mut child, "arbitrage cycles of at most");
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        lines.iter().any(|line| line.contains("at most 4 legs")),
        "an unset limit was not announced as the default: {lines:#?}"
    );

    // Above the ceiling: refused at start, naming the variable, the value
    // and the range. Not lowered to twenty.
    let mut command = node("over");
    command.env(STRATEGY, "arbitrage-desk").env(MAX_LEGS, "21");
    let (output, stderr) = refused(command);
    assert_eq!(output.status.code(), Some(EX_CONFIG), "{stderr}");
    assert!(
        stderr.contains("QIP_ARBITRAGE_MAX_LEGS=21 is refused") && stderr.contains("2 to 20"),
        "{stderr}"
    );

    // Below the floor, and not a number.
    for value in ["1", "many"] {
        let mut command = node("under");
        command.env(STRATEGY, "arbitrage-desk").env(MAX_LEGS, value);
        let (output, stderr) = refused(command);
        assert_eq!(output.status.code(), Some(EX_CONFIG), "{value}: {stderr}");
        assert!(
            stderr.contains(&format!("{MAX_LEGS}={value}")),
            "{value}: {stderr}"
        );
    }

    // A limit with no desk to hold to it would read as a control.
    let mut command = node("no-desk");
    command.env(MAX_LEGS, "5");
    let (output, stderr) = refused(command);
    assert_eq!(output.status.code(), Some(EX_CONFIG), "{stderr}");
    assert!(
        stderr.contains("QIP_ARBITRAGE_MAX_LEGS is set and QIP_ARBITRAGE_STRATEGY is not"),
        "{stderr}"
    );
}
