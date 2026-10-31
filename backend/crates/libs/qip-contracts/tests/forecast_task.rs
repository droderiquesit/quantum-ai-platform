//! The forecast router's task contract (R12-WORLD-10091).

use qip_contracts::forecast::{ForecastSolver, ForecastTask, ForecastTrigger};
use qip_core::{Duration, Timestamp};

/// A task over no time asks nothing and a task about a blank subject asks
/// about nothing; both are refused rather than issued and later scored
/// against an answer that cannot exist. A well-formed task is due one span
/// after issue and names its solver, so two kinds asked the same question
/// at the same instant are two tasks.
///
/// Mutations: drop the span check; drop the blank-subject check; leave the
/// solver out of the task id.
#[test]
fn a_task_over_no_time_or_about_nothing_is_refused_and_each_solver_gets_its_own() {
    let at = Timestamp::from_secs(1_760_000_000);
    let day = Duration::from_days(1);
    let task = |subject: &str, span: Duration, solver: ForecastSolver| {
        ForecastTask::new(
            subject,
            "hours_days",
            span,
            solver,
            ForecastTrigger::Surprise,
            at,
        )
    };
    assert!(task("AAPL", Duration::ZERO, ForecastSolver::Agent).is_err());
    assert!(task("  ", day, ForecastSolver::Agent).is_err());
    let agent = task("AAPL", day, ForecastSolver::Agent).expect("a well-formed task");
    let model = task("AAPL", day, ForecastSolver::SpecialistModel).expect("a well-formed task");
    assert_eq!(agent.due_by, at.saturating_add(day));
    assert_ne!(
        agent.task_id, model.task_id,
        "two solvers shared one task id"
    );
}
