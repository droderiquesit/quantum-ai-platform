//! ADR 0100 §4's ordering rule, proven at the seam a wire-supplied batch
//! meets a producer's recorded state: dense append, verified duplicate,
//! sequence conflict, epoch fencing, carry-over across a restart, and the
//! bounded window a retry cannot outlive.
//!
//! Each test below is paired with the mutation the packet named for it, so a
//! reviewer breaking [`ProducerTable::admit`] the way the comment describes
//! should see exactly this test fail, not a different one.

use qip_core::{Rng, Xoshiro256};
use qip_events::event_fabric::codec::ContentHash;
use qip_streaming::event_fabric::producer::{Admission, ProducerTable};

const PRODUCER: &str = "cell-eu-1";
const PARTITION: &str = "market-journal-0";

fn hash(label: &str) -> ContentHash {
    ContentHash::sha256_of(label.as_bytes())
}

#[test]
fn a_same_epoch_retry_with_a_different_payload_is_a_sequence_conflict_not_a_duplicate() {
    let mut table = ProducerTable::new();

    let first = table
        .admit(PRODUCER, PARTITION, 1, 0, 1, hash("first-attempt"))
        .expect("a fresh, in-order batch must not error");
    assert_eq!(first, Admission::Appended { next_sequence: 1 });

    // Premise: the batch actually landed before we test what a conflicting
    // retry of it does — otherwise a table that appends nothing at all would
    // also, trivially, never call anything a duplicate.
    assert_eq!(
        table.last_sequence(PRODUCER, PARTITION),
        Some(0),
        "the first batch must be on record before its retry is tested"
    );

    // Same epoch, same base sequence, different payload: two different sets
    // of records claiming slot 0. A table that deduplicates by sequence
    // alone (mutation: dedup by sequence only) cannot tell this apart from a
    // lost-ack retry and would wrongly acknowledge it as Admission::Duplicate.
    let retry = table
        .admit(PRODUCER, PARTITION, 1, 0, 1, hash("different-content"))
        .expect("a conflicting retry is a decision, not a malformed request");
    assert_eq!(
        retry,
        Admission::Conflict,
        "a same-epoch, same-sequence batch with a different payload hash must be a \
         conflict, never a duplicate"
    );
}

#[test]
fn a_superseded_epoch_is_fenced_after_its_successors_first_append() {
    let mut table = ProducerTable::new();

    let e1 = table
        .admit(PRODUCER, PARTITION, 1, 0, 1, hash("epoch1-seq0"))
        .expect("epoch 1's first batch must append");
    assert_eq!(e1, Admission::Appended { next_sequence: 1 });

    let e2 = table
        .admit(PRODUCER, PARTITION, 2, 1, 1, hash("epoch2-seq1"))
        .expect("epoch 2's first batch, continuing the dense stream, must append");
    assert_eq!(e2, Admission::Appended { next_sequence: 2 });

    // Premise: the successor epoch's first append actually happened and is
    // recorded, which is the event this table's fencing is conditioned on.
    assert_eq!(
        table.current_epoch(PRODUCER, PARTITION),
        Some(2),
        "epoch 2's append must be what this table now recognises as current"
    );

    // Epoch 1 tries again at sequence 2 — which not coincidentally is
    // exactly the sequence this table now expects next. A table that skips
    // the epoch check (mutation: accept epoch < current) would see a
    // matching sequence and append it, when the real defect here is the
    // stale epoch, not the number.
    let stale = table
        .admit(PRODUCER, PARTITION, 1, 2, 1, hash("epoch1-seq2-forged"))
        .expect("a fenced batch is a decision, not a malformed request");
    assert_eq!(
        stale,
        Admission::FencedEpoch { current_epoch: 2 },
        "epoch 1 must be refused once epoch 2 has appended, even when its sequence \
         would otherwise be exactly what this table expects next"
    );
}

#[test]
fn a_new_epoch_continues_at_the_previous_epochs_last_sequence_plus_one() {
    let mut table = ProducerTable::new();

    let first = table
        .admit(PRODUCER, PARTITION, 1, 0, 3, hash("epoch1-batch-of-3"))
        .expect("the first batch, three records wide, must append");
    assert_eq!(first, Admission::Appended { next_sequence: 3 });

    // Premise: the dense stream really did advance to cover all three
    // records before we test what the next epoch continues from.
    assert_eq!(table.last_sequence(PRODUCER, PARTITION), Some(2));

    // A restarted producer, new epoch, must continue at 3 — the previous
    // epoch's last sequence plus one — not reset to 0. A table that resets
    // on a new epoch (the named mutation) would expect 0 here and refuse
    // this as Admission::OutOfOrder instead of appending it.
    let after_restart = table
        .admit(PRODUCER, PARTITION, 2, 3, 1, hash("epoch2-seq3"))
        .expect("the carried-over sequence must be accepted");
    assert_eq!(
        after_restart,
        Admission::Appended { next_sequence: 4 },
        "a new epoch must continue the dense stream rather than restart it at 0"
    );
}

#[test]
fn a_retry_older_than_the_five_batch_window_is_refused_rather_than_acknowledged_unverified() {
    let mut table = ProducerTable::new();
    let hashes: Vec<ContentHash> = (0..6).map(|i| hash(&format!("seq-{i}"))).collect();

    for (seq, h) in hashes.iter().enumerate() {
        let seq = seq as u64;
        let outcome = table
            .admit(PRODUCER, PARTITION, 1, seq, 1, h.clone())
            .unwrap_or_else(|e| panic!("batch {seq} must append cleanly: {e:?}"));
        assert_eq!(
            outcome,
            Admission::Appended {
                next_sequence: seq + 1
            }
        );
    }

    // Premise, part one: six batches (0..=5) really did append, in order,
    // covering slots 0 through 5.
    assert_eq!(table.last_sequence(PRODUCER, PARTITION), Some(5));

    // Premise, part two: the window mechanism itself works — a retry of the
    // most recent batch, still comfortably inside the five-batch window, is
    // acknowledged as a verified duplicate. Without this, the refusal tested
    // below would be indistinguishable from a table that refuses every
    // retry regardless of the window.
    let recent_retry = table
        .admit(PRODUCER, PARTITION, 1, 5, 1, hashes[5].clone())
        .expect("a recent, in-window retry is a decision, not a malformed request");
    assert_eq!(recent_retry, Admission::Duplicate);

    // Batch 0 is now six appends behind the most recent one — one past the
    // five-batch window ADR 0100 §4 and red-team finding m4 require — so
    // this table's only honest answer is that it can no longer verify it.
    // Presenting the exact original hash is deliberate: a table that
    // acknowledges it anyway (mutation: ack it as a duplicate) is trusting
    // the wire rather than its own bounded memory.
    let stale_retry = table
        .admit(PRODUCER, PARTITION, 1, 0, 1, hashes[0].clone())
        .expect("a stale retry is a decision, not a malformed request");
    assert_eq!(
        stale_retry,
        Admission::OutsideWindow,
        "a retry older than the five-batch window must be refused, never acknowledged \
         as an unverified duplicate"
    );
}

/// ADR 0100 §4's whole point, exercised rather than reasoned about: whatever
/// order sends, lost-ack retries and producer restarts arrive in, this
/// table appends each of a producer's sequences exactly once, and always in
/// order. `Xoshiro256` gives each trial its own reproducible interleaving —
/// this is a property test written by hand, since the workspace carries no
/// property-testing dependency (ADR 0002, ADR 0009).
#[test]
fn for_any_interleaving_of_sends_lost_acks_retries_and_producer_restarts_each_sequence_is_appended_once_in_order()
 {
    const TRIALS: u64 = 40;
    const BATCHES: u64 = 15;

    for trial in 0..TRIALS {
        let mut rng = Xoshiro256::seeded(trial);
        let mut table = ProducerTable::new();
        let mut epoch = 0u64;
        // The last few batches this trial has actually sent under the
        // *current* epoch, so a simulated lost-ack retry always resends a
        // batch the epoch it claims can still vouch for. A restart clears
        // it: a genuine retry of a now-superseded epoch's batch is exactly
        // what `a_superseded_epoch_is_fenced_after_its_successors_first_append`
        // covers, so this property test does not also reach for it.
        let mut recent: Vec<(u64, u64, ContentHash)> = Vec::new();
        let mut appended_next_sequences: Vec<u64> = Vec::new();

        for seq in 0..BATCHES {
            if seq > 0 && rng.below(4) == 0 {
                epoch += 1;
                recent.clear();
            }

            let content = hash(&format!("trial{trial}-epoch{epoch}-seq{seq}"));
            let outcome = table
                .admit(PRODUCER, PARTITION, epoch, seq, 1, content.clone())
                .unwrap_or_else(|e| {
                    panic!("trial {trial}: sending batch {seq} fresh must not error: {e:?}")
                });
            match outcome {
                Admission::Appended { next_sequence } => {
                    appended_next_sequences.push(next_sequence)
                }
                other => panic!(
                    "trial {trial}: batch {seq} under epoch {epoch} must append cleanly \
                     since it is exactly the next expected sequence; got {other:?}"
                ),
            }

            recent.push((epoch, seq, content));
            if recent.len() > 5 {
                recent.remove(0);
            }

            // Zero to two lost-ack retries of a recently sent batch, picked
            // at random, each of which must be acknowledged as a verified
            // duplicate rather than appended again.
            let retry_rounds = rng.below(3);
            for _ in 0..retry_rounds {
                let idx = rng.below(recent.len() as u64) as usize;
                let (r_epoch, r_seq, r_hash) = recent[idx].clone();
                let retry_outcome = table
                    .admit(PRODUCER, PARTITION, r_epoch, r_seq, 1, r_hash)
                    .unwrap_or_else(|e| {
                        panic!("trial {trial}: retrying batch {r_seq} must not error: {e:?}")
                    });
                assert_eq!(
                    retry_outcome,
                    Admission::Duplicate,
                    "trial {trial}: a same-epoch retry of batch {r_seq}, still inside the \
                     window, must be acknowledged as a duplicate rather than appended again"
                );
            }
        }

        // A mutant that accepts base_seq <= last as a fresh append (rather
        // than only base_seq == last + 1) would either duplicate an entry in
        // this list or knock it out of strict, gapless order the moment a
        // retry landed — this is the one assertion that would catch it.
        let expected: Vec<u64> = (1..=BATCHES).collect();
        assert_eq!(
            appended_next_sequences, expected,
            "trial {trial}: every sequence 0..{BATCHES} must be appended exactly once, \
             strictly in order, regardless of how many retries or restarts happened along \
             the way"
        );
    }
}
