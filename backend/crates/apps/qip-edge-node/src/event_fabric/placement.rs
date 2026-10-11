//! Where the spool writer runs: pinned to a core of its own when the
//! deployment names one, measured either way (R12-FABRIC-7058).
//!
//! §26.1 puts the reflex node's journal writer "on a separate core". The
//! writer was already a thread of its own behind a `try_send` handoff, so a
//! stalled writer could not block the decision thread — but nothing placed
//! it, so the scheduler was free to run it on the decision thread's core,
//! where an fsync storm still costs the decisions their cache and their
//! timeslice.
//!
//! The workspace forbids `unsafe` and carries no libc binding, so this
//! process cannot call `sched_setaffinity` itself. It asks the operating
//! system's own tool, `taskset`, to pin one thread by its id, waits for it
//! under a bound, and then reads the thread's allowed-CPU list back from
//! `/proc/thread-self/status` — so a pin is believed because the kernel says
//! it holds, not because a child process exited zero. A deployment that
//! names a core and cannot have it is stopped: an operator who asked for
//! isolation and silently did not get it would be measuring the wrong
//! machine.
//!
//! Unpinned — the variable unset — the writer's placement is still read once
//! at start and announced, and [`Placement`] is what the health surface and a
//! test can compare against the decision thread's.

use qip_core::error::{Error, Result};

/// The CPU the spool writer is pinned to. Unset leaves it to the scheduler,
/// announced.
pub const WRITER_CPU_VARIABLE: &str = "QIP_SPOOL_WRITER_CPU";

/// How long `taskset` may take before the pin is treated as failed.
const PIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Where the calling thread is allowed to run and last ran.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    /// The kernel's id of the thread.
    pub tid: u32,
    /// CPUs the thread may run on, ascending.
    pub allowed: Vec<usize>,
    /// The CPU the thread last ran on.
    pub last_cpu: usize,
}

/// Read `QIP_SPOOL_WRITER_CPU`'s value: `None` unset or blank, else a CPU
/// index, refused if it is not one.
pub fn parse_writer_cpu(value: Option<&str>) -> Result<Option<usize>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    value.parse::<usize>().map(Some).map_err(|_| {
        Error::invalid(format!(
            "configuration: {WRITER_CPU_VARIABLE}={value} is not a CPU index; name one core, \
             such as `3`, or unset it to leave the writer to the scheduler"
        ))
    })
}

/// Parse a kernel CPU list such as `0-3,6,8-9`.
pub fn parse_cpu_list(list: &str) -> Result<Vec<usize>> {
    let mut cpus = Vec::new();
    for part in list.trim().split(',').filter(|part| !part.is_empty()) {
        let bad = || Error::invalid(format!("`{list}` is not a CPU list"));
        match part.split_once('-') {
            Some((from, to)) => {
                let (from, to) = (
                    from.trim().parse::<usize>().map_err(|_| bad())?,
                    to.trim().parse::<usize>().map_err(|_| bad())?,
                );
                if to < from || to - from > 4_096 {
                    return Err(bad());
                }
                cpus.extend(from..=to);
            }
            None => cpus.push(part.trim().parse::<usize>().map_err(|_| bad())?),
        }
    }
    cpus.sort_unstable();
    cpus.dedup();
    Ok(cpus)
}

/// The placement described by a thread's `status` and `stat` texts.
pub fn parse_placement(status: &str, stat: &str) -> Result<Placement> {
    let field = |name: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .map(str::trim)
            .ok_or_else(|| Error::invalid(format!("thread status carries no `{name}`")))
    };
    let tid = field("Pid:")?
        .parse::<u32>()
        .map_err(|_| Error::invalid("thread status `Pid:` is not a number"))?;
    let allowed = parse_cpu_list(field("Cpus_allowed_list:")?)?;
    // `stat`'s second field is the command in parentheses and may hold
    // spaces; count fields from after its closing parenthesis. The CPU last
    // run on is field 39, the 37th after the command.
    let after = stat
        .rsplit_once(')')
        .map(|(_, rest)| rest)
        .ok_or_else(|| Error::invalid("thread stat has no command field"))?;
    let last_cpu = after
        .split_whitespace()
        .nth(36)
        .and_then(|cpu| cpu.parse::<usize>().ok())
        .ok_or_else(|| Error::invalid("thread stat carries no last-CPU field"))?;
    Ok(Placement {
        tid,
        allowed,
        last_cpu,
    })
}

/// Where the calling thread is allowed to run and last ran, from `/proc`.
pub fn current_placement() -> Result<Placement> {
    let read = |path: &str| {
        std::fs::read_to_string(path)
            .map_err(|error| Error::io(format!("cannot read {path}: {error}")))
    };
    parse_placement(
        &read("/proc/thread-self/status")?,
        &read("/proc/thread-self/stat")?,
    )
}

/// Pin the calling thread to `cpu`, and prove it from the kernel's own
/// account. Refuses a CPU the thread is not already allowed: the pin narrows
/// a placement, never widens one a cgroup or the unit has set.
pub fn pin_current_thread(cpu: usize) -> Result<Placement> {
    let before = current_placement()?;
    if !before.allowed.contains(&cpu) {
        return Err(Error::invalid(format!(
            "configuration: {WRITER_CPU_VARIABLE}={cpu} is not a CPU this process may run on \
             ({:?}); the writer's core must be one the unit already allows",
            before.allowed
        )));
    }
    let mut child = std::process::Command::new("taskset")
        .args(["-p", "-c", &cpu.to_string(), &before.tid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| {
            Error::io(format!(
                "cannot run taskset to pin the spool writer to CPU {cpu}: {error}; install \
                 util-linux on the node or unset {WRITER_CPU_VARIABLE}"
            ))
        })?;
    let began = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if began.elapsed() < PIN_TIMEOUT => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::io(format!(
                    "taskset did not pin the spool writer within {PIN_TIMEOUT:?}"
                )));
            }
            Err(error) => return Err(Error::io(format!("waiting on taskset: {error}"))),
        }
    };
    if !status.success() {
        return Err(Error::io(format!(
            "taskset refused to pin the spool writer to CPU {cpu} ({status})"
        )));
    }
    let after = current_placement()?;
    if after.allowed != [cpu] {
        return Err(Error::io(format!(
            "taskset reported success and the kernel still allows the spool writer {:?}, not \
             CPU {cpu} alone",
            after.allowed
        )));
    }
    Ok(after)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cpu_list_and_a_threads_proc_texts_parse_and_garbage_is_refused() {
        assert_eq!(
            parse_cpu_list("0-3,6,8-9").expect("a list"),
            vec![0, 1, 2, 3, 6, 8, 9]
        );
        assert!(parse_cpu_list("3-1").is_err());
        assert!(parse_cpu_list("a").is_err());
        let status = "Name:\tqip-spool-writer\nPid:\t4242\nCpus_allowed_list:\t2\n";
        let mut fields = vec!["S"; 37];
        fields[36] = "2";
        let stat = format!("4242 (qip spool) {}", fields.join(" "));
        let placement = parse_placement(status, &stat).expect("parses");
        assert_eq!(placement.tid, 4242);
        assert_eq!(placement.allowed, vec![2]);
        assert_eq!(placement.last_cpu, 2);
        assert_eq!(parse_writer_cpu(None).expect("unset"), None);
        assert_eq!(parse_writer_cpu(Some(" 3 ")).expect("a cpu"), Some(3));
        assert!(parse_writer_cpu(Some("three")).is_err());
    }
}
