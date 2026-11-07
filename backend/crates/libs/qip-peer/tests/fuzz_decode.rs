//! The CI fuzz gate over the peer wire (R12-CICD-9101, owner decision 14 of
//! 2026-10-31): a seeded random-input test over [`qip_peer::decode`], the
//! function that reads a frame another region's node sent.
//!
//! A peer frame arrives over a socket from a process this one does not
//! control. A decoder that panics on one of them takes the edge node's peer
//! link down with it — and in release builds, which abort on panic, the
//! whole node. The property is therefore not that a frame decodes, but that
//! every frame is either decoded or refused with an error, and nothing in
//! between. The test runs in the workspace's ordinary test run, so it runs
//! in CI's test job on every pull request.
//!
//! Mutation: `decode`'s length prefix was read through `frame[..4]` in
//! place of `frame.get(..4)`, and this test failed naming the case seed and
//! the hex of a frame shorter than four bytes.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::panic::{AssertUnwindSafe, catch_unwind};

use qip_core::hash::to_hex;
use qip_core::{Decimal, Rng, Xoshiro256};
use qip_peer::{ExecutableEdge, PeerMessage, decode, encode};
// ---------------------------------------------------------------------------
// The fuzz harness (R12-CICD-9101, owner decision 14 of 2026-10-31).
//
// The same harness is copied into each decoder crate's fuzz test rather than
// shared, because a shared one would be a new crate or a test helper in
// `qip-core`'s public surface, and neither is warranted for ~150 lines.
// Keep the copies identical: `diff` them when one changes.
//
// What it guarantees: every input is generated from a `u64` case seed by
// `Xoshiro256`, so a failure printed as `case seed 0x…` reproduces exactly by
// adding that seed to `REGRESSION_SEEDS`; and the hex of the input is printed
// beside it, so the crash also reproduces without this harness at all.
// ---------------------------------------------------------------------------

/// The fresh seed when `GITHUB_SHA` is unset: a run on a laptop is the same
/// run every time, and a run in CI explores a new stream per commit.
const FIXED_FRESH_SEED: u64 = 0x5EED_0000_9101_0014;

/// The fresh run's seed: the first eight bytes of SHA-256 of `GITHUB_SHA`
/// when the variable is set (every GitHub Actions job sets it), the fixed
/// seed otherwise. Read here, in the test, and nowhere in a library.
fn fresh_seed() -> u64 {
    match std::env::var("GITHUB_SHA") {
        Ok(sha) if !sha.trim().is_empty() => {
            let digest = qip_core::sha256(sha.trim().as_bytes());
            let mut head = [0u8; 8];
            head.copy_from_slice(&digest[..8]);
            u64::from_be_bytes(head)
        }
        _ => FIXED_FRESH_SEED,
    }
}

/// Integers a decoder most often mishandles, as ASCII, for number fields.
const AWKWARD_NUMBERS: [&str; 10] = [
    "-1",
    "0",
    "4294967296",
    "18446744073709551615",
    "18446744073709551616",
    "-9223372036854775809",
    "1e309",
    "-0.0",
    "0.000000000000000000000000000001",
    "99999999999999999999999999999999999999",
];

/// Bytes that are structure in JSON or awkward in UTF-8.
const AWKWARD_BYTES: [u8; 14] = [
    0x00, 0xFF, b'"', b'{', b'}', b'[', b']', b'\\', b',', b':', 0x80, 0xC0, 0xED, b'u',
];

fn below(rng: &mut Xoshiro256, n: usize) -> usize {
    // `n` is a slice length or a small constant; the cast cannot lose bits
    // that matter and `below(0)` is never asked for.
    rng.below(n.max(1) as u64) as usize
}

fn uniform(rng: &mut Xoshiro256, len: usize) -> Vec<u8> {
    (0..len).map(|_| rng.next_u64() as u8).collect()
}

/// Replace one run of ASCII digits (with any leading minus) by an awkward
/// number. Returns the input unchanged when it holds no digit. Unused where
/// the format carries no ASCII numbers (a zstd frame), hence the allow.
#[allow(dead_code)]
fn corrupt_number(rng: &mut Xoshiro256, bytes: &[u8]) -> Vec<u8> {
    let starts: Vec<usize> = (0..bytes.len())
        .filter(|&i| bytes[i].is_ascii_digit() && (i == 0 || !bytes[i - 1].is_ascii_digit()))
        .collect();
    if starts.is_empty() {
        return bytes.to_vec();
    }
    let start = starts[below(rng, starts.len())];
    let end = (start..bytes.len())
        .find(|&i| !bytes[i].is_ascii_digit())
        .unwrap_or(bytes.len());
    let mut out = bytes[..start].to_vec();
    out.extend_from_slice(AWKWARD_NUMBERS[below(rng, AWKWARD_NUMBERS.len())].as_bytes());
    out.extend_from_slice(&bytes[end..]);
    out
}

/// One mutation of `base`, drawn from `corpus` for splices, and its name.
/// `field` is the crate's own corruption of the length or number fields its
/// format carries.
fn mutate(
    rng: &mut Xoshiro256,
    base: &[u8],
    corpus: &[Vec<u8>],
    field: fn(&mut Xoshiro256, &[u8]) -> Vec<u8>,
    depth: u32,
) -> (&'static str, Vec<u8>) {
    let len = base.len();
    match rng.below(if depth == 0 { 9 } else { 8 }) {
        0 => {
            let n = below(rng, 2 * len + 1);
            ("uniform", uniform(rng, n))
        }
        1 => {
            let mut out = base.to_vec();
            if !out.is_empty() {
                for _ in 0..1 + below(rng, 8) {
                    let at = below(rng, out.len());
                    out[at] ^= 1 << below(rng, 8);
                }
            }
            ("bit-flip", out)
        }
        2 => ("truncate", base[..below(rng, len)].to_vec()),
        3 => ("length-field", field(rng, base)),
        4 => {
            let other = &corpus[below(rng, corpus.len())];
            let mut out = base[..below(rng, len + 1)].to_vec();
            out.extend_from_slice(&other[below(rng, other.len() + 1)..]);
            ("splice", out)
        }
        5 => {
            let mut out = base.to_vec();
            if !out.is_empty() {
                for _ in 0..1 + below(rng, 4) {
                    let at = below(rng, out.len());
                    out[at] = AWKWARD_BYTES[below(rng, AWKWARD_BYTES.len())];
                }
            }
            ("awkward-byte", out)
        }
        6 => {
            let a = below(rng, len + 1);
            let b = a + below(rng, len - a + 1);
            let mut out = base[..a].to_vec();
            if rng.below(2) == 0 {
                out.extend_from_slice(&base[a..b]);
                out.extend_from_slice(&base[a..]);
                ("duplicate-chunk", out)
            } else {
                out.extend_from_slice(&base[b..]);
                ("delete-chunk", out)
            }
        }
        7 => {
            let at = below(rng, len + 1);
            let n = below(rng, 64);
            let mut out = base[..at].to_vec();
            out.extend(uniform(rng, n));
            out.extend_from_slice(&base[at..]);
            ("insert-random", out)
        }
        _ => {
            let (_, first) = mutate(rng, base, corpus, field, depth + 1);
            let (_, second) = mutate(rng, &first, corpus, field, depth + 1);
            ("stacked", second)
        }
    }
}

/// What a fuzz run saw.
#[derive(Debug, Default)]
struct Tally {
    cases: usize,
    decoded: usize,
    refused: usize,
}

/// Run `decode` over `cases` inputs for every seed. `decode` returns `true`
/// when the input decoded and `false` when it was refused; a panic fails the
/// test naming the case seed, the strategy and the input in hex.
///
/// A run seed is itself its run's first case seed, and the rest are drawn
/// from a stream it seeds. So a case seed copied from a failure into
/// `REGRESSION_SEEDS` replays exactly that input, first, on every later run.
fn run(
    seeds: &[u64],
    cases: usize,
    corpus: &[Vec<u8>],
    field: fn(&mut Xoshiro256, &[u8]) -> Vec<u8>,
    decode: &dyn Fn(&[u8]) -> bool,
) -> Tally {
    let mut tally = Tally::default();
    for &seed in seeds {
        let mut stream = Xoshiro256::seeded(seed);
        for case in 0..cases {
            let case_seed = if case == 0 { seed } else { stream.next_u64() };
            let mut rng = Xoshiro256::seeded(case_seed);
            let base = &corpus[below(&mut rng, corpus.len())];
            let (strategy, input) = mutate(&mut rng, base, corpus, field, 0);
            match catch_unwind(AssertUnwindSafe(|| decode(&input))) {
                Ok(true) => tally.decoded += 1,
                Ok(false) => tally.refused += 1,
                Err(_) => panic!(
                    "the decoder panicked instead of refusing: run seed {seed:#018x}, case seed \
                     {case_seed:#018x} (add it to REGRESSION_SEEDS to replay), strategy \
                     `{strategy}`, {} input byte(s): {}",
                    input.len(),
                    to_hex(&input)
                ),
            }
            tally.cases += 1;
        }
    }
    tally
}

/// Every valid encoding must decode, inside the same panic guard: a corpus
/// the decoder refuses would make every mutation of it a refusal for the
/// wrong reason.
fn assert_corpus_decodes(corpus: &[Vec<u8>], decode: &dyn Fn(&[u8]) -> bool) {
    assert!(!corpus.is_empty(), "premise: the corpus holds encodings");
    for (i, valid) in corpus.iter().enumerate() {
        assert!(!valid.is_empty(), "premise: corpus entry {i} is non-empty");
        let decoded = catch_unwind(AssertUnwindSafe(|| decode(valid)));
        assert!(
            matches!(decoded, Ok(true)),
            "premise: corpus entry {i} is a valid encoding and decodes: {}",
            to_hex(valid)
        );
    }
}

/// Run the regression seeds and then the fresh batch, and assert the
/// decoder was exercised on both sides of its accept/refuse line.
fn fuzz(
    regression: &[u64],
    cases_per_regression_seed: usize,
    fresh_cases: usize,
    corpus: &[Vec<u8>],
    field: fn(&mut Xoshiro256, &[u8]) -> Vec<u8>,
    decode: &dyn Fn(&[u8]) -> bool,
) {
    assert_corpus_decodes(corpus, decode);
    let fresh = fresh_seed();
    eprintln!("fuzz: fresh seed {fresh:#018x}");
    let pinned = run(regression, cases_per_regression_seed, corpus, field, decode);
    let new = run(&[fresh], fresh_cases, corpus, field, decode);
    eprintln!("fuzz: regression {pinned:?}, fresh {new:?}");
    assert_eq!(
        pinned.cases + new.cases,
        regression.len() * cases_per_regression_seed + fresh_cases,
        "premise: every case ran"
    );
    assert!(
        pinned.refused > 0 && new.refused > 0,
        "premise: some mutated input is refused, so the decoder's refusals were reached \
         (regression {pinned:?}, fresh {new:?})"
    );
}
/// The frame bound the decoder is asked to enforce.
const MAX_FRAME: usize = 4 * 1024;

/// Seeds every run replays. Add the case seed of any panic found here, so
/// the input that crashed the decoder is in every later run.
const REGRESSION_SEEDS: [u64; 6] = [
    0x0000_0000_0000_0001,
    0x0000_0000_0000_0002,
    0x9101_0000_0000_0001,
    0xDEAD_BEEF_0000_0000,
    0x0123_4567_89AB_CDEF,
    0xFFFF_FFFF_FFFF_FFFF,
];

/// One valid frame of each shape the corpus covers.
fn corpus() -> Vec<Vec<u8>> {
    let messages = [
        PeerMessage::ClockProbe {
            node: "london".into(),
            nonce: 7,
            sent_ns: 1_760_000_000_000_000_000,
        },
        PeerMessage::ClockEcho {
            node: "newyork".into(),
            to: "london".into(),
            nonce: 7,
            origin_ns: 1_760_000_000_000_000_000,
            received_ns: 1_760_000_000_000_400_000,
            sent_ns: 1_760_000_000_000_450_000,
        },
        PeerMessage::EdgeState {
            node: "london".into(),
            sequence: 3,
            edges: vec![ExecutableEdge {
                venue: "XLON".into(),
                instrument: "obj-ACME".into(),
                bid: Decimal::from_int(99),
                bid_size: Decimal::from_int(10),
                ask: Decimal::from_int(101),
                ask_size: Decimal::from_int(12),
                as_of_ns: 1_760_000_000_000_000_000,
            }],
        },
        PeerMessage::Fill {
            opportunity: "opp-1".into(),
            sequence: 1,
            leg: "buy".into(),
            quantity: Decimal::from_int(10),
        },
        PeerMessage::Declined {
            opportunity: "opp-1".into(),
            sequence: 1,
            leg: "sell".into(),
            reason: "reservation book full".into(),
        },
    ];
    messages
        .iter()
        .map(|m| encode(m, MAX_FRAME).expect("a corpus message encodes"))
        .collect()
}

/// Corrupt the four-byte length prefix — to zero, off by one either way, at
/// and past the bound, or to anything — or, half the time, a number in the
/// JSON body.
fn corrupt_length(rng: &mut Xoshiro256, frame: &[u8]) -> Vec<u8> {
    if frame.len() < 4 || rng.below(2) == 0 {
        return corrupt_number(rng, frame);
    }
    let body = (frame.len() - 4) as u32;
    let declared = match rng.below(7) {
        0 => 0,
        1 => body.wrapping_sub(1),
        2 => body + 1,
        3 => MAX_FRAME as u32,
        4 => MAX_FRAME as u32 + 1,
        5 => u32::MAX,
        _ => rng.next_u64() as u32,
    };
    let mut out = frame.to_vec();
    out[..4].copy_from_slice(&declared.to_be_bytes());
    out
}

/// Every frame of random or mutated bytes is decoded or refused; none
/// panics. The corpus decodes first, and some mutated frame must be refused,
/// so the test cannot pass on a decoder that is never reached.
#[test]
fn a_peer_frame_of_random_or_mutated_bytes_is_refused_or_decoded_and_never_panics() {
    let corpus = corpus();
    fuzz(
        &REGRESSION_SEEDS,
        512,
        4096,
        &corpus,
        corrupt_length,
        &|frame| decode(frame, MAX_FRAME).is_ok(),
    );
}
