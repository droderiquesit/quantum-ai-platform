//! Tests for the fabric journal and its replay: every destination, corridor,
//! wallet and gate decision as a record in the hash-chained log, and the
//! state rebuilt from those records alone.
//!
//! The failure each test prevents is a replay that *reads* as evidence and
//! is not: a rebuilt state that differs from the live one, a tampered record
//! stepped over, a reordered log accepted, a record whose chain verifies and
//! whose outcome lies, or two replays of one log that disagree. Every test
//! asserts its premise first — that the journal really holds the mixture of
//! admissions, vetoes and refusals it claims to — so a test that would pass
//! on an empty log fails on the premise instead.

// The workspace denies `panic_in_result_fn` for production code. In a test the
// assertion is the deliverable, and `?` keeps the fixtures readable.
#![allow(clippy::panic_in_result_fn)]

use qip_capital_fabric::corridor::{CorridorCaps, CorridorId, CorridorStage, PermittedHours};
use qip_capital_fabric::custody::{CorridorKind, CustodyClass, CustodyPolicy};
use qip_capital_fabric::destination::{
    ACTIVATION_DELAY, Approver, Asset, DestinationKey, DestinationStatus, SignatureRecord,
};
use qip_capital_fabric::gate::{
    GateCheck, KillSwitchState, SourceBalances, StatedPurpose, TransferHistory, TransferIntent,
    VelocityState,
};
use qip_capital_fabric::journal::{
    CorridorAction, CorridorStep, DestinationAction, FabricCommand, FabricJournal, FabricOutcome,
    FabricRecord, GateCommand, GateVerdict, Outcome, PRODUCER, WalletCommand, WalletOutcome,
};
use qip_capital_fabric::location::{CapitalLocation, Region};
use qip_capital_fabric::replay::{Replayed, chain_hash, replay};
use qip_capital_fabric::wallet::{
    self, HoldingObservation, LedgerView, Provenance, ReconciliationOutcome, TolerancePolicy,
};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::{CorrelationId, Currency, Duration, EventId, Lineage, Timestamp, dec, sha256_hex};
use qip_events::envelope::canonical_json;
use qip_events::log::{GENESIS_HASH, LogRecord};
use qip_events::{Envelope, EventBody, EventLog, Topic};
use serde::{Deserialize, Serialize};

// --- fixtures ---------------------------------------------------------------

/// Thursday 7 March 2024, 09:00 UTC: when everything was proposed.
fn proposed_at() -> Timestamp {
    Timestamp::from_civil(2024, 3, 7).saturating_add(Duration::from_hours(9))
}

/// When the signatures were recorded: two hours after proposal.
fn signed_at() -> Timestamp {
    proposed_at().saturating_add(Duration::from_hours(2))
}

/// When the gate is asked: the delay has elapsed with an hour to spare.
fn now() -> Timestamp {
    signed_at()
        .saturating_add(ACTIVATION_DELAY)
        .saturating_add(Duration::from_hours(1))
}

fn alice() -> Result<Approver> {
    Approver::new("alice")
}

fn bob() -> Result<Approver> {
    Approver::new("bob")
}

fn carol() -> Result<Approver> {
    Approver::new("carol")
}

fn treasury() -> CapitalLocation {
    CapitalLocation::new(Region::new("namr"), Currency::USD, VenueId::new("TREASURY"))
}

fn destination() -> Result<DestinationKey> {
    DestinationKey::new(Asset::new("USD")?, "BANK-XYZ-ACCT-1")
}

fn signature(at: Timestamp, reference: &str) -> Result<SignatureRecord> {
    SignatureRecord::new(carol()?, at, reference)
}

fn caps() -> Result<CorridorCaps> {
    CorridorCaps::new(
        dec!("1000"),
        dec!("3000"),
        dec!("10000"),
        dec!("50000"),
        Duration::from_mins(15),
        PermittedHours::ALL_DAY,
    )
}

fn corridor_id() -> Result<CorridorId> {
    CorridorId::new("treasury-to-xyz")
}

fn correlation() -> CorrelationId {
    CorrelationId::from_string("COR0000000000000000FABRIC1")
}

fn step(id: &CorridorId, step: CorridorStep) -> FabricCommand {
    FabricCommand::Corridor(CorridorAction::Step {
        id: id.clone(),
        step,
    })
}

fn gate(
    kill_switch: KillSwitchState,
    corridor: CorridorId,
    at: Timestamp,
) -> Result<FabricCommand> {
    Ok(FabricCommand::Gate(GateCommand {
        intent: TransferIntent::new(
            treasury(),
            destination()?,
            dec!("500"),
            StatedPurpose::new(dec!("1000"), dec!("500"))?,
        )?,
        corridor,
        custody: CustodyPolicy::blueprint(),
        history: TransferHistory::empty(),
        balances: SourceBalances::new(dec!("10000"), dec!("1000"), dec!("1000"), dec!("1000"))?,
        velocity: VelocityState::CLEAR,
        kill_switch,
        now: at,
    }))
}

/// The mixed sequence every test replays: an allowlist walked to signed, a
/// corridor walked to active with one refused edge on the way, a second
/// corridor left proposed, a wallet with one reconciled and one halted
/// venue-asset, a gate admission, a gate veto, an assessment against a
/// corridor the log never proposed, a suspension, and a veto caused by it.
fn mixed_sequence() -> Result<Vec<FabricCommand>> {
    let key = destination()?;
    let id = corridor_id()?;
    let other = CorridorId::new("treasury-to-abc")?;
    let ghost = CorridorId::new("never-proposed")?;
    let usd = wallet::Asset::new("USD")?;
    let jpy = wallet::Asset::new("JPY")?;
    let observed_at = now().saturating_sub(Duration::from_secs(30));
    Ok(vec![
        FabricCommand::Destination(DestinationAction::Propose {
            key: key.clone(),
            by: alice()?,
            at: proposed_at(),
        }),
        FabricCommand::Destination(DestinationAction::Verify {
            key: key.clone(),
            by: bob()?,
            at: proposed_at().saturating_add(Duration::from_hours(1)),
        }),
        FabricCommand::Destination(DestinationAction::RecordSignature {
            key: key.clone(),
            signature: signature(signed_at(), "vault/dest/1")?,
        }),
        FabricCommand::Corridor(CorridorAction::Propose {
            id: id.clone(),
            source: treasury(),
            source_class: CustodyClass::FiatAtInstitutionOfRecord,
            kind: CorridorKind::InstitutionApprovalFlow,
            destination: key.clone(),
            caps: caps()?,
            purpose: "fund the XYZ margin account ahead of forecast demand".to_string(),
            by: alice()?,
            at: proposed_at(),
        }),
        // A second corridor, so the rebuilt map has an order to get wrong.
        FabricCommand::Corridor(CorridorAction::Propose {
            id: other.clone(),
            source: treasury(),
            source_class: CustodyClass::FiatAtInstitutionOfRecord,
            kind: CorridorKind::InstitutionApprovalFlow,
            destination: key.clone(),
            caps: caps()?,
            purpose: "a second corridor, never taken past proposal".to_string(),
            by: alice()?,
            at: proposed_at(),
        }),
        step(
            &id,
            CorridorStep::Review {
                by: bob()?,
                at: proposed_at().saturating_add(Duration::from_hours(1)),
            },
        ),
        step(
            &id,
            CorridorStep::RecordSignature {
                signature: signature(signed_at(), "vault/corridor/1")?,
            },
        ),
        step(&id, CorridorStep::BeginDelay { now: signed_at() }),
        // Refused: the delay has not elapsed. Recorded as a refusal.
        step(
            &id,
            CorridorStep::Activate {
                now: signed_at().saturating_add(Duration::from_hours(1)),
            },
        ),
        step(
            &id,
            CorridorStep::Activate {
                now: signed_at().saturating_add(ACTIVATION_DELAY),
            },
        ),
        // Refused: no such corridor.
        step(
            &ghost,
            CorridorStep::Review {
                by: bob()?,
                at: now(),
            },
        ),
        FabricCommand::Wallet(WalletCommand::Assemble {
            observations: vec![
                HoldingObservation::new(
                    VenueId::new("XTKS"),
                    jpy.clone(),
                    dec!("999"),
                    observed_at,
                    Provenance::ReadOnlyApiKey,
                ),
                HoldingObservation::new(
                    VenueId::new("XNYS"),
                    usd.clone(),
                    dec!("10000"),
                    observed_at,
                    Provenance::Statement,
                ),
            ],
            ledger_views: vec![
                LedgerView::new(
                    VenueId::new("XNYS"),
                    usd.clone(),
                    dec!("10000"),
                    dec!("0"),
                    dec!("0"),
                )?,
                LedgerView::new(
                    VenueId::new("XTKS"),
                    jpy.clone(),
                    dec!("1000"),
                    dec!("0"),
                    dec!("0"),
                )?,
            ],
            freshness: Duration::from_mins(5),
            now: now(),
        }),
        FabricCommand::Wallet(WalletCommand::Reconcile {
            tolerances: TolerancePolicy::new()
                .with_tolerance(usd, dec!("1"))?
                .with_tolerance(jpy, dec!("0.5"))?,
            at: now(),
        }),
        gate(KillSwitchState::Armed, id.clone(), now())?,
        gate(
            KillSwitchState::Tripped,
            id.clone(),
            now().saturating_add(Duration::from_mins(1)),
        )?,
        // Refused outright: the corridor named was never proposed.
        gate(KillSwitchState::Armed, ghost, now())?,
        step(
            &id,
            CorridorStep::Suspend {
                by: None,
                reason: "reconciliation halted XTKS/JPY".to_string(),
                at: now().saturating_add(Duration::from_mins(2)),
            },
        ),
        gate(
            KillSwitchState::Armed,
            id,
            now().saturating_add(Duration::from_mins(3)),
        )?,
    ])
}

fn journal_of(seed: u64, commands: Vec<FabricCommand>) -> Result<FabricJournal> {
    let mut journal = FabricJournal::new(seed, correlation());
    for command in commands {
        journal.decide(command)?;
    }
    Ok(journal)
}

fn decoded(records: &[LogRecord]) -> Result<Vec<FabricRecord>> {
    records
        .iter()
        .map(|record| Ok(record.event.decode::<FabricRecord>()?.body))
        .collect()
}

/// Recompute the chain from `from` onwards, as a writer with the log's own
/// rule would after altering a record.
fn rechain(records: &mut [LogRecord], from: usize) -> Result<()> {
    let mut previous = match from.checked_sub(1).and_then(|i| records.get(i)) {
        Some(record) => record.record_hash.clone(),
        None => GENESIS_HASH.to_string(),
    };
    for record in records.iter_mut().skip(from) {
        record.previous_hash = previous.clone();
        record.record_hash = chain_hash(record.sequence, &record.previous_hash, &record.event)?;
        previous = record.record_hash.clone();
    }
    Ok(())
}

fn vetoed_by(record: &FabricRecord) -> Option<GateCheck> {
    match &record.outcome {
        FabricOutcome::Gate(Outcome::Applied(GateVerdict::Vetoed(vetoed))) => Some(vetoed.check),
        _ => None,
    }
}

/// The premise every test shares: the sequence really is mixed.
fn assert_mixed(records: &[FabricRecord]) {
    let refused = records.iter().filter(|r| r.outcome.is_refused()).count();
    let admitted = records
        .iter()
        .filter(|r| {
            matches!(
                r.outcome,
                FabricOutcome::Gate(Outcome::Applied(GateVerdict::Admitted(_)))
            )
        })
        .count();
    let vetoed = records.iter().filter(|r| vetoed_by(r).is_some()).count();
    let halted = records
        .iter()
        .filter(|r| match &r.outcome {
            FabricOutcome::Wallet(Outcome::Applied(WalletOutcome::Reconciled { outcomes })) => {
                outcomes.iter().any(ReconciliationOutcome::is_halt)
            }
            _ => false,
        })
        .count();
    assert_eq!(
        records.len(),
        18,
        "the fixture is not the sequence described"
    );
    assert_eq!(
        refused, 3,
        "expected the early activation, the ghost review and the ghost gate"
    );
    assert_eq!(admitted, 1, "expected exactly one admitted assessment");
    assert_eq!(
        vetoed, 2,
        "expected the kill-switch veto and the suspended-corridor veto"
    );
    assert_eq!(halted, 1, "expected one reconciliation with a halt in it");
}

// --- the properties ---------------------------------------------------------

#[test]
fn a_state_rebuilt_from_the_journal_equals_the_live_state_after_a_mixed_sequence() -> Result<()> {
    // The failure this prevents: a journal that records most decisions, or
    // records them with an input missing, so the replay drifts from the
    // process that wrote it. The live state is the one the controls built
    // as they ran; the replayed state is built from the log alone; the two
    // must be equal on every field, including the refusals.
    let journal = journal_of(7, mixed_sequence()?)?;
    let records = decoded(journal.records())?;
    assert_mixed(&records);
    assert_eq!(journal.log().verify_chain(), Ok(()));

    // The live state is what the sequence says it is, before comparing.
    let live = journal.state();
    let id = corridor_id()?;
    let corridor = live.corridor(&id).expect("the corridor was proposed");
    assert_eq!(corridor.stage(), CorridorStage::Suspended);
    assert_eq!(live.corridors().len(), 2);
    assert!(matches!(
        live.destinations().get(&destination()?).map(|r| &r.status),
        Some(DestinationStatus::Signed { .. })
    ));
    assert_eq!(live.reconciliations().len(), 2);
    assert_eq!(
        live.reconciliations()
            .values()
            .filter(|o| o.is_halt())
            .count(),
        1
    );
    assert_eq!(live.assessments().len(), 3);
    assert!(live.wallet().is_some());

    let Replayed {
        state,
        applied,
        passed_over,
    } = replay(journal.records())?;
    assert_eq!(applied, records.len());
    assert_eq!(passed_over, 0);
    assert_eq!(&state, live);
    Ok(())
}

#[test]
fn a_tampered_record_is_refused_naming_its_position() -> Result<()> {
    // The failure this prevents: a replay that steps over a record whose
    // content no longer hashes to what the chain committed, and carries on
    // with a state that reads as rebuilt from the log. A hand edit to the
    // JSONL — a corridor's caps raised after the fact — must stop the whole
    // replay and say where.
    let journal = journal_of(7, mixed_sequence()?)?;
    let mut records = journal.records().to_vec();
    assert_mixed(&decoded(&records)?);
    // Premise: untampered, the same records replay.
    assert!(replay(&records).is_ok());

    let position = 8; // one-based: the corridor's begin_delay
    let target = &mut records[position - 1];
    let mut payload = target.event.payload.clone();
    payload["command"]["step"] = serde_json::Value::String("activate".to_string());
    target.event.payload = payload;

    let err = replay(&records).expect_err("a tampered record must refuse the replay");
    let message = err.message();
    assert!(
        message.contains(&format!("position {position} ")),
        "the refusal must name the position: {message}"
    );
    assert!(
        message.contains("has been altered"),
        "the refusal must say the record was altered: {message}"
    );
    Ok(())
}

#[test]
fn a_record_out_of_sequence_is_refused_naming_its_position() -> Result<()> {
    // The failure this prevents: two records swapped — a suspension replayed
    // after the assessment it should have vetoed — and a replay that
    // accepted the reordering because each record still hashed to itself.
    let journal = journal_of(7, mixed_sequence()?)?;
    let mut records = journal.records().to_vec();
    assert_mixed(&decoded(&records)?);
    assert!(replay(&records).is_ok());

    records.swap(2, 3); // positions 3 and 4
    let err = replay(&records).expect_err("a reordered log must refuse the replay");
    let message = err.message();
    assert!(
        message.contains("position 3 carries sequence 4"),
        "the refusal must name the position and the sequence found there: {message}"
    );
    assert!(
        message.contains("does not reorder or skip"),
        "the refusal must say what the replay refuses to do: {message}"
    );
    Ok(())
}

#[test]
fn a_record_whose_recorded_outcome_disagrees_with_the_control_is_refused_even_when_the_chain_verifies()
-> Result<()> {
    // The failure this prevents: a writer other than the control — or the
    // control against a state the log does not hold — records "admitted" on
    // inputs the seven checks veto, and re-chains the log so every hash is
    // valid. A replay that copied outcomes out of records would launder it.
    // This replay re-runs the command and refuses the disagreement.
    let journal = journal_of(7, mixed_sequence()?)?;
    let mut records = journal.records().to_vec();
    let bodies = decoded(&records)?;
    assert_mixed(&bodies);

    let admitted = bodies
        .iter()
        .position(|r| {
            matches!(
                r.outcome,
                FabricOutcome::Gate(Outcome::Applied(GateVerdict::Admitted(_)))
            )
        })
        .expect("an admitted assessment is in the fixture");
    let vetoed = bodies
        .iter()
        .position(|r| vetoed_by(r) == Some(GateCheck::KillSwitch))
        .expect("a kill-switch veto is in the fixture");

    // Lie: give the vetoed assessment the admitted one's outcome, then
    // re-chain so the lie is hash-consistent.
    let admitted_outcome = records[admitted].event.payload["outcome"].clone();
    let target = &mut records[vetoed];
    target.event.payload["outcome"] = admitted_outcome;
    target.event.payload_hash = sha256_hex(canonical_json(&target.event.payload).as_bytes());
    rechain(&mut records, vetoed)?;

    let err = replay(&records).expect_err("a lying record must refuse the replay");
    let message = err.message();
    assert!(
        message.contains(&format!("position {} ", vetoed + 1)),
        "the refusal must name the position: {message}"
    );
    assert!(
        message.contains("records an outcome the control does not produce"),
        "the refusal must be about the outcome, not the chain — the chain was valid: {message}"
    );
    Ok(())
}

#[test]
fn replay_is_deterministic_across_two_runs_and_two_journals() -> Result<()> {
    // The failure this prevents: a rebuilt state whose iteration order
    // depends on insertion history or hashing, so two replays of one log
    // render differently and a diff between them is noise. Every map in the
    // state is a BTreeMap and every id comes from a seeded generator, so
    // two journals of the same commands are byte-identical and two replays
    // of them are equal.
    let first = journal_of(7, mixed_sequence()?)?;
    let second = journal_of(7, mixed_sequence()?)?;
    assert_mixed(&decoded(first.records())?);
    // Premise: there is an order to get wrong.
    assert!(first.state().corridors().len() >= 2);
    assert!(first.state().reconciliations().len() >= 2);

    assert_eq!(first.records(), second.records());
    let hashes: Vec<&str> = first
        .records()
        .iter()
        .map(|r| r.record_hash.as_str())
        .collect();
    let again: Vec<&str> = second
        .records()
        .iter()
        .map(|r| r.record_hash.as_str())
        .collect();
    assert_eq!(hashes, again);

    let one = replay(first.records())?;
    let two = replay(first.records())?;
    let three = replay(second.records())?;
    assert_eq!(one, two);
    assert_eq!(one, three);
    assert_eq!(format!("{:?}", one.state), format!("{:?}", three.state));
    Ok(())
}

#[test]
fn the_journals_chain_rule_agrees_with_the_event_logs_own() -> Result<()> {
    // The failure this prevents: `chain_hash` restates a rule `qip-events`
    // keeps private. If the two drift, a replay would either refuse every
    // honest log or accept a tampered one, and nothing else would notice.
    // Here every record the log wrote is verified by the log itself and
    // recomputed by the replay's rule, and the two must agree on each.
    let journal = journal_of(7, mixed_sequence()?)?;
    let records = journal.records();
    assert!(!records.is_empty());
    assert_eq!(journal.log().verify_chain(), Ok(()));
    for record in records {
        assert_eq!(
            chain_hash(record.sequence, &record.previous_hash, &record.event)?,
            record.record_hash,
            "sequence {} hashes differently under the replay's rule",
            record.sequence
        );
    }
    Ok(())
}

#[test]
fn a_gate_refusal_record_names_the_refusing_check_as_a_delimited_token() -> Result<()> {
    // The failure this prevents: a veto recorded as prose alone, so an
    // operator reading the log knows a transfer was refused and not by which
    // of the seven checks. The check is a field, and it is asserted as a
    // delimited JSON token rather than a substring — `kill_switch` is also a
    // substring of the reason text the veto carries.
    let journal = journal_of(7, mixed_sequence()?)?;
    let bodies = decoded(journal.records())?;
    assert_mixed(&bodies);
    let vetoed = bodies
        .iter()
        .position(|r| vetoed_by(r) == Some(GateCheck::KillSwitch))
        .expect("a kill-switch veto is in the fixture");
    let admitted = bodies
        .iter()
        .position(|r| {
            matches!(
                r.outcome,
                FabricOutcome::Gate(Outcome::Applied(GateVerdict::Admitted(_)))
            )
        })
        .expect("an admitted assessment is in the fixture");

    let veto_json = canonical_json(&journal.records()[vetoed].event.payload);
    assert!(
        veto_json.contains(r#""check":"kill_switch""#),
        "the veto record must carry the check as a field: {veto_json}"
    );
    let admitted_json = canonical_json(&journal.records()[admitted].event.payload);
    assert!(
        !admitted_json.contains(r#""check":"#),
        "an admission names no refusing check: {admitted_json}"
    );
    assert!(
        admitted_json.contains(r#""checks_passed":["corridor_authority","#),
        "an admission lists the checks that passed: {admitted_json}"
    );
    Ok(())
}

/// A foreign event, for a log the fabric shares with other writers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Note {
    text: String,
}

impl EventBody for Note {
    const TOPIC: Topic = Topic::SystemAlert;
    const SCHEMA_VERSION: u32 = 1;
}

fn foreign_event(at: Timestamp, producer: &str) -> Result<qip_events::AnyEvent> {
    Envelope::new(
        EventId::from_string("EVT0000000000000000FOREIGN"),
        at,
        at,
        Lineage::root(correlation(), producer),
        Note {
            text: "another topic's business".to_string(),
        },
    )
    .erase()
}

#[test]
fn a_journal_resumed_from_a_shared_log_rebuilds_its_state_and_chain_verifies_foreign_records()
-> Result<()> {
    // The failure this prevents: a composition root hands the fabric the
    // platform's one log, which already carries other topics, and the fabric
    // either refuses the log for holding records that are not its own or
    // ignores them so completely that a tampered foreign record breaks no
    // chain. Foreign records are passed over and still verified.
    let mut log = EventLog::in_memory();
    log.append(&foreign_event(proposed_at(), "somebody-else")?)?;
    let mut journal = FabricJournal::resume(log, 7, correlation())?;
    for command in mixed_sequence()? {
        journal.decide(command)?;
    }
    let bodies: Result<Vec<FabricRecord>> = journal
        .records()
        .iter()
        .skip(1)
        .map(|record| Ok(record.event.decode::<FabricRecord>()?.body))
        .collect();
    assert_mixed(&bodies?);

    let replayed = replay(journal.records())?;
    assert_eq!(replayed.passed_over, 1);
    assert_eq!(replayed.applied, 18);
    assert_eq!(&replayed.state, journal.state());

    // Resuming the very log rebuilds the same state and keeps writing.
    let live = journal.state().clone();
    let mut resumed = FabricJournal::resume(journal.into_log(), 7, correlation())?;
    assert_eq!(resumed.state(), &live);
    let record = resumed.decide(step(
        &corridor_id()?,
        CorridorStep::Revoke {
            by: alice()?,
            reason: "closing the corridor".to_string(),
            at: now().saturating_add(Duration::from_hours(1)),
        },
    ))?;
    assert!(matches!(
        record.outcome,
        FabricOutcome::Corridor(Outcome::Applied(ref standing))
            if standing.stage == CorridorStage::Revoked
    ));
    assert_eq!(resumed.log().verify_chain(), Ok(()));

    // A tampered foreign record breaks the chain for everything after it.
    let mut records = resumed.records().to_vec();
    records[0].event.payload["text"] = serde_json::Value::String("edited".to_string());
    let err = replay(&records).expect_err("a tampered foreign record must refuse the replay");
    assert!(
        err.message().contains("position 1 "),
        "the refusal must name the foreign record's position: {}",
        err.message()
    );
    Ok(())
}

#[test]
fn a_fabric_record_that_cannot_be_decoded_is_refused_rather_than_passed_over() -> Result<()> {
    // The failure this prevents: a record on the fabric's topic, from the
    // fabric's producer, whose payload is not a fabric record — a schema the
    // build does not know, or a corruption the chain still commits to —
    // treated as foreign and stepped over. It is the fabric's own record and
    // the fabric cannot read it, which is a refusal, not a pass.
    let mut log = EventLog::in_memory();
    let mut event = foreign_event(proposed_at(), PRODUCER)?;
    event.topic = FabricRecord::TOPIC;
    log.append(&event)?;
    // Premise: the record is on the fabric topic under the fabric producer.
    let record = &log.records()[0];
    assert_eq!(record.event.topic, FabricRecord::TOPIC);
    assert_eq!(record.event.lineage.producer, PRODUCER);
    assert_eq!(log.verify_chain(), Ok(()));

    let err = replay(log.records()).expect_err("an undecodable fabric record must refuse");
    let message = err.message();
    assert!(
        message.contains("position 1 ") && message.contains("cannot be decoded"),
        "the refusal must name the position and the reason: {message}"
    );
    assert!(
        FabricJournal::resume(log, 7, correlation()).is_err(),
        "a journal must not resume a log it cannot replay"
    );
    Ok(())
}

#[test]
fn the_journal_adopts_a_decision_only_after_the_log_has_it() -> Result<()> {
    // The failure this prevents: state first, record second, so a log that
    // refuses the append — full, here, of records that may not be evicted —
    // leaves the platform holding a decision the log does not. The journal
    // must refuse the command and leave the state exactly where it was.
    let log = EventLog::in_memory().with_capacity(1)?;
    let mut journal = FabricJournal::resume(log, 7, correlation())?;
    let mut commands = mixed_sequence()?.into_iter();
    let first = commands.next().expect("the fixture has a first command");
    let second = commands.next().expect("the fixture has a second command");
    journal.decide(first)?;
    let before = journal.state().clone();
    // Premise: the first record is in, and it is audit-class, so the second
    // append has nothing to evict.
    assert_eq!(journal.records().len(), 1);
    assert!(FabricRecord::TOPIC.requires_permanent_retention());

    let err = journal
        .decide(second)
        .expect_err("a full log must refuse the decision");
    assert!(
        err.message().contains("will not discard an audit record"),
        "the refusal is the log's own: {}",
        err.message()
    );
    assert_eq!(journal.state(), &before);
    assert_eq!(journal.records().len(), 1);
    Ok(())
}
