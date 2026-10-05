//! `qip agency shadow --request <path>`: one shadow pass of the agency loop.
//!
//! An operator hands it a request document (a goal, the affordance graph, the
//! acting identity, the evidence on each lever and the proposed plans) and is
//! told what the engine would do with it and why: which stages passed, which
//! proposals were refused and for what, and which steps cleared every gate.
//!
//! It cannot act, twice over. It runs the engine with no action policy, which
//! is shadow authority, so the engine calls no adapter; and the adapter it
//! hands over refuses every call, so a defect that raised the authority would
//! still send nothing. If the engine ever reports a step as executed this
//! command refuses to print it as a shadow result.
//!
//! The exit code is the verdict, as for `registrations` and `replay`: zero
//! when a plan cleared every gate, [`WITHHELD`] when the engine declined to
//! act, and one when the document could not be read as a request at all.

use qip_agency::engine::{Adapter, Outcome, Request};
use qip_agency::plan::Step;
use qip_core::error::{Error, Result};

/// A plan cleared every gate and would have run.
pub const ADMITTED: u8 = 0;

/// The engine declined: no action, abstain, gather evidence, propose an
/// experiment, or a stage that stopped the pass.
pub const WITHHELD: u8 = 3;

const USAGE: &str = "usage: qip agency shadow --request <path>";

/// The adapter this binary has: none.
struct Unwired;

impl Adapter for Unwired {
    fn call(&mut self, step: &Step) -> Result<()> {
        Err(Error::denied(format!(
            "no action adapter is wired into this binary; `{}` was not sent",
            step.tool
        )))
    }
}

/// Run one shadow pass over a request document. Returns the lines to print
/// and the exit code, so a test asserts on what the operator reads.
pub fn shadow(document: &str) -> Result<(Vec<String>, u8)> {
    let request: Request = serde_json::from_str(document)
        .map_err(|error| Error::invalid(format!("the request document is refused: {error}")))?;
    let report = qip_agency::engine::run(request, None, &mut Unwired)?;

    let mut lines = vec![
        "agency shadow pass: no adapter is wired, so nothing below was sent".to_string(),
        format!("stages passed: {:?}", report.passed),
    ];
    if let Some(stage) = report.failed {
        lines.push(format!("stage failed: {stage:?}"));
    }
    lines.push(format!("levers: {}", report.levers.join(", ")));
    for (index, refusal) in &report.refused {
        lines.push(format!(
            "refused proposal {}: {}",
            index + 1,
            refusal.message()
        ));
    }
    let code = match report.outcome {
        Outcome::Shadowed(steps) => {
            lines.push("outcome: shadowed; these steps cleared every gate".to_string());
            for step in steps {
                lines.push(format!(
                    "  {} on {}, exposure {}",
                    step.tool, step.variable, step.exposure
                ));
            }
            ADMITTED
        }
        Outcome::NoAction => {
            lines.push("outcome: no action; no proposal beat doing nothing".to_string());
            WITHHELD
        }
        Outcome::Abstain(levers) => {
            lines.push(format!(
                "outcome: abstain; weakly identified and not open to experiment: {}",
                levers.join(", ")
            ));
            WITHHELD
        }
        Outcome::GatherEvidence(levers) => {
            lines.push(format!(
                "outcome: gather evidence; nothing is measured about: {}",
                levers.join(", ")
            ));
            WITHHELD
        }
        Outcome::ProposeExperiment(levers) => {
            lines.push(format!(
                "outcome: propose an experiment on: {}",
                levers.join(", ")
            ));
            WITHHELD
        }
        Outcome::Stopped(why) => {
            lines.push(format!("outcome: stopped; {why}"));
            WITHHELD
        }
        Outcome::Executed(_) => {
            return Err(Error::guard(
                "a shadow pass reported an executed step; refusing to print it as shadow",
            ));
        }
    };
    Ok((lines, code))
}

/// The `agency` family. One subcommand today.
pub fn run(arguments: &[String]) -> Result<u8> {
    match arguments {
        [action, flag, path] if action == "shadow" && flag == "--request" => {
            let document = std::fs::read_to_string(path)
                .map_err(|error| Error::io(format!("could not read {path}: {error}")))?;
            let (lines, code) = shadow(&document)?;
            for line in lines {
                println!("{line}");
            }
            Ok(code)
        }
        _ => Err(Error::invalid(USAGE)),
    }
}
