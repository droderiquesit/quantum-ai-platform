//! Framed transports and stream reassembly.
//!
//! The property that matters here is indifference to chunking: a stream cut into
//! pieces at any offsets must decode to exactly what the whole stream decodes
//! to. It is asserted exhaustively rather than at a few chosen offsets, because
//! the offsets that break a reassembler are the ones nobody thought to pick.

use qip_contracts::{MarketMessage, MessageBody, VenueId};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::testing::Property;
use qip_core::{Decimal, Timestamp};
use qip_protocols::decoder::SkipReason;
use qip_protocols::{
    ByteOrder, Decoder, Framing, InstrumentPartitions, ItchDecoder, JsonFrameDecoder,
    ProtocolRegistry, StreamAssembler,
};

fn at() -> Timestamp {
    Timestamp::from_nanos(1_704_207_845_500_000_000)
}

fn instruments() -> InstrumentPartitions {
    InstrumentPartitions::new()
        .with("BTC-USD", 1)
        .with("ETH-USD", 2)
}

fn json_decoder(framing: Framing) -> JsonFrameDecoder {
    JsonFrameDecoder::new(VenueId::new("CRYPTOX"), "ws-1", instruments(), framing)
}

fn frames() -> Vec<String> {
    vec![
        r#"{"seq":1,"ts":1704207845000,"symbol":"BTC-USD","type":"level","side":"bid","price":"64000.25","qty":"1.5","orders":4}"#.to_string(),
        r#"{"seq":2,"ts":1704207845010,"symbol":"BTC-USD","type":"trade","side":"sell","price":"64000.25","qty":"0.25"}"#.to_string(),
        r#"{"seq":3,"ts":1704207845020,"symbol":"ETH-USD","type":"quote","bid":["3400.10","12"],"ask":["3400.60","8"]}"#.to_string(),
        r#"{"seq":4,"ts":1704207845030,"symbol":"BTC-USD","type":"remove","order_id":991}"#.to_string(),
        r#"{"seq":5,"ts":1704207845040,"symbol":"BTC-USD","type":"status","status":"halted"}"#.to_string(),
    ]
}

fn newline_stream() -> Vec<u8> {
    let mut out = Vec::new();
    for frame in frames() {
        out.extend_from_slice(frame.as_bytes());
        out.push(b'\n');
    }
    out
}

fn length_prefixed_stream(width: usize, includes_prefix: bool) -> Vec<u8> {
    let mut out = Vec::new();
    for frame in frames() {
        let declared = if includes_prefix {
            frame.len() + width
        } else {
            frame.len()
        };
        qip_protocols::sbe::put_uint(&mut out, ByteOrder::Big, width, declared as u64);
        out.extend_from_slice(frame.as_bytes());
    }
    out
}

/// Decode `bytes` delivered in the given chunk sizes, through the assembler.
fn decode_in_chunks(
    decoder: &mut dyn Decoder,
    bytes: &[u8],
    cuts: &[usize],
) -> Vec<MarketMessage> {
    let mut assembler = StreamAssembler::new();
    let mut messages = Vec::new();
    let mut start = 0usize;
    for cut in cuts.iter().copied().chain(std::iter::once(bytes.len())) {
        let end = cut.clamp(start, bytes.len());
        let chunk = bytes.get(start..end).unwrap_or_default();
        messages.extend(
            assembler
                .push(decoder, chunk, at())
                .unwrap_or_else(|error| panic!("chunk {start}..{end} failed: {error}")),
        );
        start = end;
    }
    messages
}

#[test]
fn a_newline_delimited_stream_decodes_to_one_message_per_frame() {
    let mut decoder = json_decoder(Framing::NewlineDelimited);
    let messages = decoder.decode(&newline_stream(), at()).expect("decodes");

    assert_eq!(messages.len(), 5);
    assert_eq!(messages[0].origin.sequence, 1);
    assert!(matches!(messages[0].body, MessageBody::LevelSet { .. }));
    assert!(matches!(messages[2].body, MessageBody::Quote { .. }));
    assert!(matches!(messages[4].body, MessageBody::StatusChange { .. }));
    assert_eq!(
        messages[0].venue_time,
        Timestamp::from_millis(1_704_207_845_000)
    );
}

#[test]
fn a_stream_split_at_every_single_offset_decodes_to_exactly_what_the_whole_stream_does() {
    for (label, framing, bytes) in [
        (
            "newline",
            Framing::NewlineDelimited,
            newline_stream(),
        ),
        (
            "length-prefixed",
            Framing::LengthPrefixed {
                width: 4,
                order: ByteOrder::Big,
                includes_prefix: false,
            },
            length_prefixed_stream(4, false),
        ),
        (
            "length-prefixed-inclusive",
            Framing::LengthPrefixed {
                width: 2,
                order: ByteOrder::Big,
                includes_prefix: true,
            },
            length_prefixed_stream(2, true),
        ),
    ] {
        let expected = json_decoder(framing)
            .decode(&bytes, at())
            .unwrap_or_else(|error| panic!("{label}: whole stream failed: {error}"));
        assert_eq!(expected.len(), 5, "{label}");

        for cut in 0..=bytes.len() {
            let mut decoder = json_decoder(framing);
            let actual = decode_in_chunks(&mut decoder, &bytes, &[cut]);
            assert_eq!(
                actual, expected,
                "{label}: a split at byte {cut} changed the decoded stream"
            );
        }
    }
}

#[test]
fn a_stream_split_at_every_pair_of_offsets_still_decodes_identically() {
    // Two frames rather than five: the interesting case is a boundary that lands
    // inside a frame *and* a second read that also stops mid-frame, and that is
    // already exercised by every pair of offsets over a short stream.
    let bytes: Vec<u8> = newline_stream()
        .split_inclusive(|byte| *byte == b'\n')
        .take(2)
        .flatten()
        .copied()
        .collect();
    let expected = json_decoder(Framing::NewlineDelimited)
        .decode(&bytes, at())
        .expect("decodes");
    assert_eq!(expected.len(), 2);

    for first in 0..=bytes.len() {
        for second in first..=bytes.len() {
            let mut decoder = json_decoder(Framing::NewlineDelimited);
            let actual = decode_in_chunks(&mut decoder, &bytes, &[first, second]);
            assert_eq!(
                actual, expected,
                "splits at {first} and {second} changed the decoded stream"
            );
        }
    }
}

#[test]
fn a_binary_stream_delivered_in_random_chunks_decodes_identically() {
    // The same property for a fixed-width binary protocol, where a bad split
    // corrupts silently rather than failing to parse.
    let mut bytes = Vec::new();
    for index in 0..12u64 {
        let mut message = vec![b'A'];
        message.extend_from_slice(&[0, 1, 0, 0]);
        message.extend_from_slice(&(index * 1_000).to_be_bytes()[2..]);
        message.extend_from_slice(&index.to_be_bytes());
        message.push(if index % 2 == 0 { b'B' } else { b'S' });
        message.extend_from_slice(&(100u32 + index as u32).to_be_bytes());
        message.extend_from_slice(b"AAPL    ");
        message.extend_from_slice(&1_002_500u32.to_be_bytes());
        bytes.extend_from_slice(&message);
    }

    let build = || {
        ItchDecoder::new(
            VenueId::new("XNAS"),
            "itch-a",
            InstrumentPartitions::new().with("AAPL", 3),
            Timestamp::from_civil(2024, 1, 2),
        )
    };
    let expected = build().decode(&bytes, at()).expect("decodes");
    assert_eq!(expected.len(), 12);

    Property::new("random chunking never changes the decoded stream")
        .cases(200)
        .for_all(
            |rng: &mut Xoshiro256| {
                let mut cuts: Vec<usize> = (0..4)
                    .map(|_| rng.below(bytes.len() as u64 + 1) as usize)
                    .collect();
                cuts.sort_unstable();
                cuts
            },
            |cuts| {
                let mut decoder = build();
                let actual = decode_in_chunks(&mut decoder, &bytes, cuts);
                if actual == expected {
                    Ok(())
                } else {
                    Err(format!("{} messages instead of {}", actual.len(), expected.len()))
                }
            },
        );
}

#[test]
fn a_frame_that_is_not_valid_json_costs_that_frame_and_nothing_else() {
    // Unlike FIX, the frame boundaries here do not depend on the content, so a
    // bad frame does not put the stream in doubt.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"{not json at all}\n");
    bytes.extend_from_slice(newline_stream().as_slice());

    let mut decoder = json_decoder(Framing::NewlineDelimited);
    let messages = decoder.decode(&bytes, at()).expect("the stream survives");
    assert_eq!(messages.len(), 5);
    assert!(matches!(
        decoder.diagnostics().recent_skips[0].reason,
        SkipReason::Malformed { .. }
    ));
}

#[test]
fn an_empty_frame_is_a_keep_alive_and_produces_nothing() {
    let mut decoder = json_decoder(Framing::NewlineDelimited);
    let messages = decoder.decode(b"\n\r\n", at()).expect("decodes");
    assert!(messages.is_empty());
    assert_eq!(decoder.diagnostics().messages_skipped, 0);
    assert_eq!(decoder.consumed(), 3);
}

#[test]
fn an_amendment_keeps_the_order_identity_the_venue_keeps() {
    let frame = br#"{"seq":9,"ts":1704207845050,"symbol":"BTC-USD","type":"amend","order_id":7,"price":"64001","qty":"2"}"#;
    let mut bytes = frame.to_vec();
    bytes.push(b'\n');

    let mut decoder = json_decoder(Framing::NewlineDelimited);
    let messages = decoder.decode(&bytes, at()).expect("decodes");
    assert!(matches!(
        messages[0].body,
        MessageBody::OrderReplaced {
            order_ref: 7,
            quantity,
            ..
        } if quantity == Decimal::from_int(2)
    ));
}

#[test]
fn a_buffer_that_never_completes_a_frame_is_refused_before_it_exhausts_memory() {
    let mut decoder = json_decoder(Framing::NewlineDelimited);
    let mut assembler = StreamAssembler::new().with_capacity_limit(64);
    let error = assembler
        .push(&mut decoder, &[b'x'; 128], at())
        .expect_err("an unbounded carry-over is a memory leak triggered by a broken feed");
    assert!(error.message().contains("frame boundary"), "{error}");
    assert_eq!(
        assembler.pending(),
        0,
        "the buffer is dropped rather than kept growing"
    );
}

#[test]
fn a_registry_routes_a_read_to_the_decoder_registered_for_that_feed() {
    let mut registry = ProtocolRegistry::new();
    registry
        .register(
            VenueId::new("CRYPTOX"),
            "ws-1",
            Box::new(json_decoder(Framing::NewlineDelimited)),
        )
        .expect("first registration succeeds");
    registry
        .register(
            VenueId::new("XNAS"),
            "itch-a",
            Box::new(ItchDecoder::new(
                VenueId::new("XNAS"),
                "itch-a",
                InstrumentPartitions::new().with("AAPL", 3),
                Timestamp::from_civil(2024, 1, 2),
            )),
        )
        .expect("a second feed registers");

    let venue = VenueId::new("CRYPTOX");
    let stream = newline_stream();
    let split = stream.len() / 2;
    let first = registry
        .push(&venue, "ws-1", &stream[..split], at())
        .expect("decodes");
    assert!(
        registry.pending(&venue, "ws-1").is_some_and(|held| held > 0),
        "the frame straddling the split is held rather than decoded in halves"
    );
    let second = registry
        .push(&venue, "ws-1", &stream[split..], at())
        .expect("decodes");
    assert_eq!(first.len() + second.len(), 5);
    assert_eq!(registry.pending(&venue, "ws-1"), Some(0));

    assert!(
        registry
            .decoder_mut(&VenueId::new("XNAS"), "itch-a")
            .is_ok_and(|decoder| decoder.protocol() == "itch.5.0")
    );
    assert!(
        registry.decoder_mut(&venue, "ws-2").is_err(),
        "an unregistered feed is an error, not a silent no-op"
    );
}

#[test]
fn registering_a_second_decoder_for_one_feed_is_refused() {
    let mut registry = ProtocolRegistry::new();
    let venue = VenueId::new("CRYPTOX");
    registry
        .register(
            venue.clone(),
            "ws-1",
            Box::new(json_decoder(Framing::NewlineDelimited)),
        )
        .expect("first registration succeeds");
    let error = registry
        .register(
            venue,
            "ws-1",
            Box::new(json_decoder(Framing::NewlineDelimited)),
        )
        .expect_err("two decoders on one feed would keep two sequence positions");
    assert!(error.message().contains("already has a decoder"), "{error}");
}
