//! FIX 4.4 decoding: framing integrity, group expansion and refusal behaviour.

use qip_contracts::{BookSide, MessageBody, TradeCondition, VenueId};
use qip_core::{Decimal, Timestamp};
use qip_protocols::decoder::SkipReason;
use qip_protocols::{Decoder, FixDecoder, InstrumentPartitions};

const SOH: char = '\x01';

fn body(fields: &[(u32, &str)]) -> Vec<u8> {
    let mut text = String::new();
    for (tag, value) in fields {
        text.push_str(&format!("{tag}={value}{SOH}"));
    }
    text.into_bytes()
}

fn framed(fields: &[(u32, &str)]) -> Vec<u8> {
    qip_protocols::fix::frame_message("FIX.4.4", &body(fields))
}

fn decoder() -> FixDecoder {
    FixDecoder::new(
        VenueId::new("XNAS"),
        "fix-md",
        InstrumentPartitions::new().with("AAPL", 7).with("MSFT", 9),
    )
}

fn snapshot() -> Vec<u8> {
    framed(&[
        (35, "W"),
        (34, "101"),
        (52, "20240102-15:04:05.123"),
        (55, "AAPL"),
        (268, "2"),
        (269, "0"),
        (270, "100.25"),
        (271, "500"),
        (269, "1"),
        (270, "100.30"),
        (271, "300"),
    ])
}

fn incremental() -> Vec<u8> {
    framed(&[
        (35, "X"),
        (34, "102"),
        (52, "20240102-15:04:06.000"),
        (268, "2"),
        (279, "0"),
        (269, "0"),
        (55, "AAPL"),
        (270, "100.24"),
        (271, "400"),
        (279, "2"),
        (269, "1"),
        (55, "AAPL"),
        (270, "100.30"),
        (271, "300"),
    ])
}

fn at() -> Timestamp {
    Timestamp::from_nanos(1_704_207_845_500_000_000)
}

#[test]
fn a_full_refresh_tells_the_book_to_discard_before_it_states_the_levels() {
    let mut decoder = decoder();
    let messages = decoder
        .decode(&snapshot(), at())
        .expect("a well-formed snapshot decodes");

    assert_eq!(messages.len(), 3, "a reset and two levels");
    assert!(
        matches!(messages[0].body, MessageBody::Reset { .. }),
        "a snapshot that did not reset first would leave removed levels in the book forever"
    );
    assert!(matches!(
        messages[1].body,
        MessageBody::LevelSet {
            side: BookSide::Bid,
            ..
        }
    ));
    assert!(matches!(
        messages[2].body,
        MessageBody::LevelSet {
            side: BookSide::Ask,
            ..
        }
    ));
    for message in &messages {
        assert_eq!(message.origin.sequence, 101, "all facts share MsgSeqNum");
        assert_eq!(message.origin.partition, 7, "AAPL's configured partition");
    }
}

#[test]
fn a_deleted_level_is_expressed_as_a_level_of_zero_size_whatever_size_the_venue_sent() {
    let mut decoder = decoder();
    let messages = decoder
        .decode(&incremental(), at())
        .expect("a well-formed incremental refresh decodes");

    assert_eq!(messages.len(), 2);
    let MessageBody::LevelSet { quantity, .. } = &messages[1].body else {
        panic!("the delete should still be a level set");
    };
    assert_eq!(
        *quantity,
        Decimal::ZERO,
        "the venue sent 300 on the delete; trusting it would resurrect the level"
    );
}

#[test]
fn a_fill_becomes_a_trade_and_an_order_acknowledgement_becomes_nothing() {
    let mut decoder = decoder();
    let bytes = [
        framed(&[
            (35, "8"),
            (34, "103"),
            (52, "20240102-15:04:07.000"),
            (55, "AAPL"),
            (150, "F"),
            (31, "100.27"),
            (32, "100"),
            (54, "1"),
        ]),
        framed(&[
            (35, "8"),
            (34, "104"),
            (52, "20240102-15:04:08.000"),
            (55, "AAPL"),
            (150, "0"),
            (39, "0"),
        ]),
    ]
    .concat();

    let messages = decoder.decode(&bytes, at()).expect("both reports frame");
    assert_eq!(messages.len(), 1, "only the fill carries a market fact");
    assert!(matches!(
        messages[0].body,
        MessageBody::Trade {
            condition: TradeCondition::Regular,
            aggressor: Some(BookSide::Bid),
            ..
        }
    ));
    assert_eq!(decoder.diagnostics().messages_skipped, 1);
    assert!(
        !decoder.diagnostics().lost_information(),
        "an order acknowledgement is not market data, so skipping it loses nothing"
    );
}

#[test]
fn a_corrupt_checksum_is_refused_with_a_reason_that_names_the_check() {
    let mut bytes = snapshot();
    let last = bytes.len() - 2;
    bytes[last] = if bytes[last] == b'0' { b'1' } else { b'0' };

    let error = decoder()
        .decode(&bytes, at())
        .expect_err("a message whose checksum disagrees must not be accepted");
    assert!(
        error.message().contains("checksum"),
        "the reason has to name the failed check so an operator can tell corruption from a bug: {error}"
    );
}

#[test]
fn a_single_flipped_body_byte_is_caught_even_though_the_framing_still_looks_right() {
    // The point of the checksum: the body length still locates the trailer, so
    // nothing about the framing looks wrong. Only the sum notices.
    let mut bytes = snapshot();
    let position = bytes
        .windows(7)
        .position(|window| window == b"100.25\x01")
        .expect("the snapshot quotes 100.25");
    bytes[position + 5] = b'9';

    let error = decoder().decode(&bytes, at()).expect_err("corruption is refused");
    assert!(error.message().contains("checksum"));
}

#[test]
fn a_body_length_that_does_not_locate_the_checksum_is_refused() {
    // Declare one byte fewer than the body actually holds. The message is
    // otherwise perfect, and a decoder that trusted the length would start the
    // next message one byte early and never recover.
    let payload = body(&[
        (35, "W"),
        (34, "101"),
        (52, "20240102-15:04:05.123"),
        (55, "AAPL"),
        (268, "0"),
    ]);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"8=FIX.4.4\x01");
    bytes.extend_from_slice(format!("9={}\x01", payload.len() - 1).as_bytes());
    bytes.extend_from_slice(&payload);
    let checksum = bytes.iter().fold(0u32, |acc, byte| acc + u32::from(*byte)) % 256;
    bytes.extend_from_slice(format!("10={checksum:03}\x01").as_bytes());

    let error = decoder()
        .decode(&bytes, at())
        .expect_err("a body length that points at the wrong place must be refused");
    assert!(
        error.message().contains("BodyLength"),
        "the reason must name the field that disagreed: {error}"
    );
}

#[test]
fn a_message_that_has_not_finished_arriving_yields_nothing_and_consumes_nothing() {
    let bytes = snapshot();
    for prefix in 0..bytes.len() {
        let mut decoder = decoder();
        let messages = decoder
            .decode(&bytes[..prefix], at())
            .unwrap_or_else(|error| panic!("prefix of {prefix} bytes errored: {error}"));
        assert!(
            messages.is_empty(),
            "a {prefix}-byte prefix produced {} messages; half a message is worse than none",
            messages.len()
        );
        assert_eq!(
            decoder.consumed(),
            0,
            "a {prefix}-byte prefix consumed bytes it could not decode"
        );
    }
}

#[test]
fn a_complete_message_followed_by_a_partial_one_consumes_exactly_the_complete_one() {
    let first = snapshot();
    let second = incremental();
    let bytes = [first.clone(), second[..second.len() / 2].to_vec()].concat();

    let mut decoder = decoder();
    let messages = decoder.decode(&bytes, at()).expect("the first message decodes");
    assert_eq!(messages.len(), 3);
    assert_eq!(
        decoder.consumed(),
        first.len(),
        "the partial second message must stay in the caller's buffer"
    );
}

#[test]
fn an_unknown_message_type_is_recorded_and_the_rest_of_the_batch_still_decodes() {
    let bytes = [
        framed(&[(35, "0"), (34, "99"), (52, "20240102-15:04:00.000")]),
        snapshot(),
    ]
    .concat();

    let mut decoder = decoder();
    let messages = decoder.decode(&bytes, at()).expect("the batch survives");
    assert_eq!(messages.len(), 3, "the snapshot after the heartbeat decodes");
    let skips = &decoder.diagnostics().recent_skips;
    assert_eq!(skips.len(), 1);
    assert!(matches!(
        skips[0].reason,
        SkipReason::UnknownMessageType { .. }
    ));
}

#[test]
fn an_instrument_this_feed_does_not_carry_is_skipped_rather_than_given_a_partition() {
    let bytes = framed(&[
        (35, "W"),
        (34, "200"),
        (52, "20240102-15:04:05.123"),
        (55, "TSLA"),
        (268, "1"),
        (269, "0"),
        (270, "240.10"),
        (271, "100"),
    ]);
    let mut decoder = decoder();
    let messages = decoder.decode(&bytes, at()).expect("the frame is well formed");
    assert!(messages.is_empty());
    assert!(matches!(
        decoder.diagnostics().recent_skips[0].reason,
        SkipReason::UnmappedInstrument { .. }
    ));
}

#[test]
fn decoding_the_same_bytes_twice_produces_identical_messages_including_identifiers() {
    let bytes = [snapshot(), incremental()].concat();
    let first = decoder().decode(&bytes, at()).expect("decodes");
    let second = decoder().decode(&bytes, at()).expect("decodes");
    assert_eq!(
        first, second,
        "replay must reproduce the run exactly, identifiers included"
    );
    assert!(first.iter().all(|message| message.object_id.as_str().len() == 26));
}

#[test]
fn the_same_message_on_two_different_feeds_gets_different_identifiers() {
    // The identifier commits to the feed, so a fact carried by two unrelated
    // feeds is never mistaken for one fact delivered twice.
    let bytes = snapshot();
    let mut other = FixDecoder::new(
        VenueId::new("XNAS"),
        "fix-md-b",
        InstrumentPartitions::new().with("AAPL", 7),
    );
    let a = decoder().decode(&bytes, at()).expect("decodes");
    let b = other.decode(&bytes, at()).expect("decodes");
    assert_eq!(a.len(), b.len());
    assert!(
        a.iter().zip(&b).all(|(x, y)| x.object_id != y.object_id),
        "two feeds must not mint the same identifier"
    );
}

#[test]
fn a_message_with_no_sending_time_is_refused_rather_than_stamped_with_the_capture_clock() {
    let bytes = framed(&[(35, "W"), (34, "300"), (55, "AAPL"), (268, "0")]);
    let error = decoder()
        .decode(&bytes, at())
        .expect_err("a fabricated venue timestamp would corrupt every bitemporal read");
    assert!(error.message().contains("SendingTime"), "{error}");
}
