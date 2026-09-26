//! The reflex journal under chain v2, and a journal that trims behind what it
//! has shipped.
//!
//! Each test names the red-team finding it closes. The chain is the only
//! record of what a cell decided at speed, so every property here is one an
//! auditor reading a mirrored journal relies on without seeing the cell.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::reflex::{ChainVersion, GATE_JOURNAL_ENCODING, chain_digest_v1};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use qip_edge::cell::{Cell, CellConfig, PolledHalt};
use qip_edge::journal::{Decision, Journal, JournalEntry, MemoryMirror, MirrorBatch, ship};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;

const CELL: &str = "london-1";

/// An instant with a sub-second part, so a digest that drops it can be told
/// from one that keeps it.
fn at(secs: i64, nanos: i64) -> Timestamp {
    Timestamp::from_nanos((1_760_000_000 + secs) * 1_000_000_000 + nanos)
}

fn refusal(index: usize) -> Decision {
    Decision::Refused {
        gate: "capital".to_string(),
        reason: format!("item {index}"),
    }
}

/// Re-read a journal after moving entry `index`'s instant by `nanos`.
fn with_instant_moved(journal: &Journal, index: usize, nanos: i64) -> Journal {
    let mut value = serde_json::to_value(journal).expect("a journal serialises");
    let slot = &mut value["entries"][index]["at"];
    let stored = slot
        .as_i64()
        .expect("a v2 entry stores its instant as nanoseconds");
    *slot = serde_json::Value::from(stored + nanos);
    serde_json::from_value(value).expect("an edited journal still reads")
}

#[test]
fn a_sub_second_edit_to_a_recorded_instant_breaks_the_v2_chain() {
    // F3. v1 hashed `at.as_secs()`, so moving an entry's instant by anything
    // under a second left its digest unchanged and the chain verified the
    // edit — to the very fact an ordering dispute between two cells turns on.
    let mut journal = Journal::new();
    for index in 0..3 {
        journal.record(refusal(index), at(index as i64, 100_000_000));
    }
    assert_eq!(
        journal.verify(),
        Ok(()),
        "premise: the untouched chain verifies"
    );
    let entry = &journal.entries()[1];
    assert_eq!(
        entry.version,
        ChainVersion::V2,
        "premise: new entries seal under v2"
    );

    // Premise, and the finding: under v1 the half-second edit is invisible.
    let moved = Timestamp::from_nanos(entry.at.as_nanos() + 400_000_000);
    assert_eq!(
        chain_digest_v1(&journal.entries()[0].digest, 1, entry.at, &entry.decision),
        chain_digest_v1(&journal.entries()[0].digest, 1, moved, &entry.decision),
        "premise: v1 cannot see a sub-second edit"
    );

    assert_eq!(
        with_instant_moved(&journal, 1, 400_000_000).verify(),
        Err(1),
        "a 400 ms edit to entry 1's instant verified under v2"
    );
    assert_eq!(
        with_instant_moved(&journal, 1, 1).verify(),
        Err(1),
        "a one-nanosecond edit to entry 1's instant verified under v2"
    );
}

/// An entry exactly as a cell sealed it under v1, before this chain had a
/// version: the SLICE-07 pinned fill, its digest and its text.
const V1_ENTRY: &str = "{\"sequence\":0,\"at\":\"2023-11-14T22:13:20.000Z\",\"decision\":\
    {\"Filled\":{\"order_id\":\"ord-1\",\"venue\":\"SIM\",\"object\":\"obj-1\",\
    \"quantity\":\"10\",\"price\":\"101.5\",\"simulated\":true,\
    \"shares\":[[\"alpha\",\"10\"]]}},\
    \"digest\":\"9f27d4aac9d56ec26c8844c7757e32252e1980a8a5d37d6378f4e6ae93706f85\"}";

#[test]
fn a_journal_written_under_v1_still_verifies_after_v2_lands() -> Result<()> {
    // Every journal a cell has already mirrored was sealed under v1. If v2
    // landing made any of them fail to verify, every incident review of the
    // past would read as tampering.
    let entry: JournalEntry = serde_json::from_str(V1_ENTRY).expect("a v1 entry reads");
    assert_eq!(
        entry.version,
        ChainVersion::V1,
        "an unversioned entry must read as v1"
    );
    assert_eq!(
        serde_json::to_string(&entry).expect("serialises"),
        V1_ENTRY,
        "a v1 entry did not re-serialise byte-for-byte"
    );

    // As a mirrored batch, the centre's side of the chain.
    let batch: MirrorBatch = serde_json::from_str(&format!(
        "{{\"cell\":\"{CELL}\",\"at\":\"2023-11-14T22:13:21.000Z\",\"chains_onto\":\"genesis\",\
         \"entries\":[{V1_ENTRY}],\"watermarks\":[]}}"
    ))
    .expect("a v1 batch reads");
    batch.verify_against(Journal::GENESIS)?;

    // As a journal, and one that carries on under v2 from where v1 stopped.
    let mut journal: Journal =
        serde_json::from_str(&format!("{{\"entries\":[{V1_ENTRY}],\"shipped\":1}}"))
            .expect("a v1 journal reads");
    assert_eq!(journal.verify(), Ok(()), "a v1 journal no longer verifies");
    journal.record(refusal(1), Timestamp::from_secs(1_700_000_001));
    assert_eq!(journal.entries()[1].version, ChainVersion::V2);
    assert_eq!(
        journal.verify(),
        Ok(()),
        "a v2 entry did not chain onto a v1 one"
    );

    // And the reverse is refused: nothing seals v1 any more, so a v1 entry
    // after a v2 one is a relabel or a rolled-back writer.
    let mut value = serde_json::to_value(&journal).expect("serialises");
    value["entries"][1]["version"] = serde_json::Value::from("v1");
    let relabelled: Journal = serde_json::from_value(value).expect("reads");
    assert_eq!(relabelled.verify(), Err(1));
    Ok(())
}

#[test]
fn a_decision_whose_json_cannot_be_produced_is_refused_and_never_hashed_as_its_kind() {
    // F4. v1 hashed a decision that would not serialise as its bare kind, so
    // every such decision of one kind sealed to one digest and the chain
    // vouched for contents nobody could read back. A non-finite conviction
    // is the live case: `serde_json` writes it as `null` without complaint,
    // and `null` does not read back as a number.
    for conviction in [f64::NAN, f64::INFINITY] {
        let decision = Decision::SignalRaised {
            strategy: "alpha".to_string(),
            object: "obj-1".to_string(),
            kind: "enter".to_string(),
            conviction_shrunk_f64: conviction,
        };
        assert!(
            serde_json::to_string(&decision).is_ok(),
            "premise: the serialiser accepts it silently, which is the hazard"
        );

        let mut journal = Journal::new();
        let instant = at(0, 5);
        let entry = journal.record(decision, instant).clone();
        match &entry.decision {
            Decision::Refused { gate, reason } => {
                assert_eq!(gate, GATE_JOURNAL_ENCODING);
                assert!(
                    reason.starts_with("a signal_raised decision was not recorded as itself: "),
                    "the refusal does not name what was refused: {reason}"
                );
            }
            other => panic!("an unencodable decision was sealed as itself: {other:?}"),
        }
        let kind_digest = qip_core::sha256_hex(
            format!("v2|genesis|0|{}|signal_raised", instant.as_nanos()).as_bytes(),
        );
        assert_ne!(
            entry.digest, kind_digest,
            "the kind was hashed in place of a body"
        );

        journal.record(refusal(1), at(1, 0));
        assert_eq!(
            journal.verify(),
            Ok(()),
            "the refusal does not verify as sealed"
        );
    }
}

#[test]
fn trimming_behind_a_shipped_watermark_keeps_sequences_continuous_and_batches_chain_continuous()
-> Result<()> {
    // F2. A trim that renumbered would issue sequence 0 twice in one session,
    // and a batch that looked up its predecessor by sequence index after a
    // trim would find nothing and claim to start the session.
    let mut journal = Journal::trimmed_on_ship();
    let mut mirror = MemoryMirror::new();
    let mut recorded = 0usize;
    for (round, size) in [3usize, 2, 4].into_iter().enumerate() {
        for _ in 0..size {
            journal.record(refusal(recorded), at(recorded as i64, 7));
            recorded += 1;
        }
        assert_eq!(
            ship(
                &mut journal,
                &mut mirror,
                CELL,
                Vec::new(),
                at(100, round as i64)
            )?,
            size
        );
        let first = mirror
            .batches()
            .last()
            .and_then(|batch| batch.entries.first())
            .map(|entry| entry.sequence);
        assert_eq!(
            first,
            Some((recorded - size) as u64),
            "round {round} reissued a sequence an earlier batch already carried"
        );
        assert_eq!(
            journal.retained(),
            0,
            "round {round} did not trim what it shipped"
        );
        assert_eq!(journal.len(), recorded);
    }

    let sequences: Vec<u64> = mirror
        .batches()
        .iter()
        .flat_map(|batch| batch.entries.iter().map(|entry| entry.sequence))
        .collect();
    assert_eq!(
        sequences,
        (0..9).collect::<Vec<u64>>(),
        "sequences were reissued or skipped"
    );
    for pair in mirror.batches().windows(2) {
        assert_eq!(
            pair[1].chains_onto,
            pair[0].tail_digest(),
            "a batch lost its predecessor"
        );
    }
    mirror.verify_continuity()?;

    // What is recorded after a trim chains onto the trimmed tail, and an
    // unshipped entry is never trimmed: it exists nowhere else.
    journal.record(refusal(recorded), at(200, 0));
    assert_eq!(journal.entries()[0].sequence, 9);
    assert_eq!(journal.verify(), Ok(()));
    assert!(
        journal.trim_through(9).is_err(),
        "an unshipped entry was trimmed"
    );
    assert_eq!(journal.retained(), 1);
    Ok(())
}

fn cell(config: CellConfig) -> Result<Cell> {
    Cell::new(
        config,
        FeatureEngine::new(MarketState::default(), Duration::from_secs(5)),
    )
}

#[test]
fn a_journal_built_to_trim_on_ship_keeps_only_unshipped_entries_and_one_built_without_keeps_all()
-> Result<()> {
    // m6. The default journal keeps the whole session in memory, which is
    // what every existing reader of `entries()` expects and is unbounded on a
    // cell that runs for days. Trimming is opt-in so those readers keep their
    // numbers, and `len()` means the same thing either way.
    let mut kept = Journal::new();
    let mut trimmed = Journal::trimmed_on_ship();
    let (mut kept_mirror, mut trimmed_mirror) = (MemoryMirror::new(), MemoryMirror::new());
    for index in 0..4 {
        kept.record(refusal(index), at(index as i64, 0));
        trimmed.record(refusal(index), at(index as i64, 0));
    }
    ship(&mut kept, &mut kept_mirror, CELL, Vec::new(), at(10, 0))?;
    ship(
        &mut trimmed,
        &mut trimmed_mirror,
        CELL,
        Vec::new(),
        at(10, 0),
    )?;
    for index in 4..6 {
        kept.record(refusal(index), at(index as i64, 0));
        trimmed.record(refusal(index), at(index as i64, 0));
    }
    assert_eq!(
        kept_mirror.batches(),
        trimmed_mirror.batches(),
        "premise: one history"
    );
    assert_eq!(kept.len(), 6);
    assert_eq!(
        trimmed.len(),
        kept.len(),
        "len() means something else when trimming"
    );
    assert_eq!(
        kept.retained(),
        6,
        "a journal not built to trim lost entries"
    );
    assert_eq!(
        trimmed.retained(),
        2,
        "a journal built to trim kept what it shipped"
    );
    assert_eq!(trimmed.unshipped(), kept.unshipped());

    // The same through a cell: the setting reaches the journal it builds.
    let config = || CellConfig::new(CELL, "europe-west2").with_venue(VenueId::new("XLON"));
    let mut plain = cell(config())?;
    let mut lean = cell(config().with_journal_trimmed_on_ship())?;
    for each in [&mut plain, &mut lean] {
        each.apply_polled_halt(PolledHalt::Engaged("drill".to_string()), at(1, 0));
        each.apply_polled_halt(PolledHalt::Released, at(2, 0));
        each.flush(&mut MemoryMirror::new(), at(3, 0))?;
    }
    assert_eq!(
        plain.journal().len(),
        2,
        "premise: the halt and release were journaled"
    );
    assert_eq!(lean.journal().len(), plain.journal().len());
    assert_eq!(plain.journal().retained(), 2);
    assert_eq!(
        lean.journal().retained(),
        0,
        "the cell's journal was not built to trim"
    );
    Ok(())
}
