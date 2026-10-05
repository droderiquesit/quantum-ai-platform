//! `qip`'s own storage configuration, held to the rule the serving binaries
//! hold (ARCH-054, RES-072): a cache is never the store of record.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::process::Command;

/// Run `qip storage` with exactly the storage variables given.
fn storage_command(variables: &[(&str, &str)]) -> (String, String, bool) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_qip"));
    command
        .arg("storage")
        .env_remove("QIP_STORAGE_TARGET")
        .env_remove("QIP_STORAGE_ROOT");
    for (name, value) in variables {
        command.env(name, value);
    }
    let output = command.output().expect("the qip binary runs");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

#[test]
fn the_command_line_refuses_to_keep_the_event_log_archive_in_the_memorystore_cache() {
    // The failure this prevents: `qip cycle` archives the hash-chained event
    // log on the configured store. The four serving binaries refuse
    // Memorystore as that store; this one did not, so the one root an
    // operator runs by hand could put the only copy of the chain in a cache
    // whose instance has persistence disabled.
    //
    // Premise: the command works when nothing names the cache, so the
    // refusal below is about the cache and not about the command.
    let (printed, errors, succeeded) = storage_command(&[]);
    assert!(succeeded, "{errors}");
    assert!(printed.contains("target:    memory"), "{printed}");

    let (printed, errors, succeeded) = storage_command(&[("QIP_STORAGE_TARGET", "memorystore")]);
    assert!(
        !succeeded,
        "the command accepted a cache as its store: {printed}"
    );
    // The refusal that names the reason, not the one a missing Redis
    // address would produce a line later — that one says what to configure
    // to make the cache work, which is the opposite of what an operator
    // should be told.
    assert!(
        errors.contains("a cache whose instance has persistence disabled"),
        "the refusal does not say why: {errors}"
    );
}
