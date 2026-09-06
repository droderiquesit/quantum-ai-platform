//! `qip registrations` — what every catalogued source demands, where it
//! stands, and the one command that moves a pending one.
//!
//! The platform does everything about a venue registration except the part
//! that has to be a person's: reading the terms, opening the account,
//! creating the key, writing it to Secret Manager. So this command answers
//! the question an operator actually has — *which of those have I not done
//! yet, and what is the exact line for the one I am missing* — and answers
//! it from the same registry the feed's admission gate consults, so the
//! console, the gate and this command cannot disagree about who registered.
//!
//! Three properties, and each is structural rather than asserted:
//!
//! * **Names only.** Every row is built from
//!   [`qip_api::registration_views`], whose `secret` fields are deployment
//!   variable names read off the shipped manifests. A credential value is
//!   nowhere in this process to be printed, and the command it prints reads
//!   the value from stdin (`--data-file=-`) so that running it puts nothing
//!   in a shell history, an argument list or a process listing.
//! * **One statement of the command.** [`qip_api::registration_views::secret_command`]
//!   is where the `gcloud` line is written, once, for the console and for
//!   here. A second copy would be a second claim about one fact, and the
//!   quieter of the two would be the wrong one.
//! * **The exit code is the verdict.** [`PENDING`] rather than zero when any
//!   catalogued source is still refused, so a deployment script can gate on
//!   `qip registrations` instead of on somebody reading the output.
//!
//! [`report`] takes the rows rather than the platform on purpose: the
//! interesting question — *does a catalogue with nothing pending exit zero*
//! — cannot be asked of the shipped table, which declares two sources that
//! need an account, and a function that reached for the platform itself
//! could only ever be tested against the one answer.

use qip_api::registration_views::{SourceRegistrationView, StandingView};

/// Every catalogued source is admitted: keyless, or registered by a named
/// person.
pub const ADMITTED: u8 = 0;

/// At least one catalogued source is refused until somebody registers.
///
/// Three rather than one, because one is what this command exits with when
/// it could not answer at all — a configuration that does not parse, a
/// platform that does not assemble. A caller that treated "I could not
/// look" and "I looked and two sources are pending" as the same outcome
/// would report a broken deployment as a pending registration.
pub const PENDING: u8 = 3;

/// What the command printed and what it exits with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub lines: Vec<String>,
    pub code: u8,
}

/// The banner every run prints, before any source.
///
/// It says what the command will not do, because the request this whole
/// surface exists under was for a platform that registers itself: it does
/// not, it prints the command for a person to run, and saying so at the top
/// is cheaper than an operator discovering it at the bottom.
const BANNER: [&str; 3] = [
    "qip registrations — every catalogued source, what it demands, where it stands.",
    "Names only: no credential value is read, printed or stored by this command, and",
    "no source is registered by it. A registration is a person's act, under their name.",
];

/// Render the report for `sources`, in the order given.
pub fn report(sources: &[SourceRegistrationView]) -> Report {
    let mut lines: Vec<String> = BANNER.iter().map(|line| (*line).to_string()).collect();
    let mut pending: Vec<&str> = Vec::new();

    for source in sources {
        lines.push(String::new());
        lines.push(source.source_id.clone());
        lines.push(format!(
            "  requirement:  {}",
            // `None` is not "keyless": it is a source nobody wrote a
            // requirement for, and the registry refuses it for exactly that
            // reason. Printing the absence rather than a guess keeps the row
            // consistent with the standing beside it.
            source
                .requirement
                .clone()
                .unwrap_or_else(|| "not declared".to_string())
        ));
        match &source.standing {
            StandingView::Keyless => {
                lines
                    .push("  standing:     admitted — keyless; no registration needed".to_string());
            }
            StandingView::Registered {
                operator,
                terms_read_at,
                secret,
            } => {
                lines.push(format!(
                    "  standing:     admitted — registered by {operator}, terms read at \
                     {terms_read_at}"
                ));
                lines.push(format!(
                    "  credential:   read under {secret} (a name, not a value)"
                ));
            }
            StandingView::Pending {
                who_must_register,
                reason,
            } => {
                pending.push(source.source_id.as_str());
                lines.push(format!(
                    "  standing:     PENDING — {who_must_register} must register, in their own \
                     name"
                ));
                if let Some(terms) = &source.terms {
                    lines.push(format!("  terms:        {terms}"));
                }
                lines.push(format!("  refused:      {reason}"));
                lines.extend(instructions(source));
            }
        }
    }

    lines.push(String::new());
    lines.push(verdict(sources.len(), &pending));
    Report {
        lines,
        code: if pending.is_empty() {
            ADMITTED
        } else {
            PENDING
        },
    }
}

/// What an operator does about a pending source, ending in the exact lines
/// they run.
///
/// A source whose manifest reads no credential gets a different answer and
/// not a fabricated command: Kalshi's manifest declares `auth: none` while
/// the shipped requirement table says the venue needs an account, so there
/// is genuinely no secret to add and a `gcloud` line printed anyway would
/// name a secret nothing reads.
fn instructions(source: &SourceRegistrationView) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(slot) = &source.secret_slot {
        let mut slots = vec![slot.clone()];
        slots.extend(
            source
                .companion_secret_slots
                .iter()
                .map(|companion| companion.variable.clone()),
        );
        lines.push(format!("  reads:        {}", slots.join(", ")));
    }
    lines.push(
        "  do this:      read the terms above, register with the venue under your own".to_string(),
    );
    lines.push(
        "                identity, and create the credential in the venue's dashboard.".to_string(),
    );
    match &source.secret_command {
        None => {
            lines.push(
                "                This source's manifest reads no credential, so there is no secret"
                    .to_string(),
            );
            lines.push(
                "                to add; record the registration once the account exists."
                    .to_string(),
            );
        }
        Some(command) => {
            lines.push(format!("  then run:     {command}"));
            for companion in &source.companion_secret_slots {
                lines.push(format!("                {}", companion.secret_command));
            }
            lines.push(
                "                The value goes in on stdin, so it reaches no shell history,"
                    .to_string(),
            );
            lines.push("                no argument list and no process listing.".to_string());
        }
    }
    lines.push(format!(
        "  then record:  POST /registrations/{}/approve, as the operator who did it.",
        source.source_id
    ));
    lines
}

/// The last line, which is the one a person reads first.
fn verdict(catalogued: usize, pending: &[&str]) -> String {
    if pending.is_empty() {
        return format!(
            "every one of {catalogued} catalogued source(s) is admitted; nothing is waiting on a \
             registration."
        );
    }
    format!(
        "{} of {catalogued} catalogued source(s) pending: {}. The platform reads none of them \
         until somebody registers.",
        pending.len(),
        pending.join(", ")
    )
}
