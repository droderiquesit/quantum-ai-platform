//! SBE decoding against an in-tree schema, with the version tolerance that
//! decides whether a venue's schema roll-out is an outage.

use qip_contracts::{BookSide, MessageBody, VenueId, VenueStatus};
use qip_core::{Decimal, Timestamp};
use qip_protocols::decoder::SkipReason;
use qip_protocols::sbe::{frame_message, put_ascii, put_uint};
use qip_protocols::{
    ByteOrder, Decoder, FieldRole, InstrumentPartitions, SbeDecoder, SbeEncoding, SbeField,
    SbeGroup, SbeMessage, SbeMessageKind, SbeSchema,
};

const ORDER: ByteOrder = ByteOrder::Little;
const BOOK_TEMPLATE: u16 = 1;
const STATUS_TEMPLATE: u16 = 2;
/// Nine implied decimals, the widest SBE price convention in use.
const PRICE_EXPONENT: u32 = 9;

fn schema() -> SbeSchema {
    let header = vec![
        SbeField::new(
            "TransactTime",
            0,
            SbeEncoding::Uint { width: 8 },
            FieldRole::VenueTimeNanos,
        ),
        SbeField::new(
            "MsgSeqNum",
            8,
            SbeEncoding::Uint { width: 4 },
            FieldRole::Sequence,
        ),
    ];
    let entries = vec![
        SbeField::new(
            "MDEntryPx",
            0,
            SbeEncoding::Fixed {
                width: 8,
                signed: true,
                exponent: PRICE_EXPONENT,
            },
            FieldRole::Price,
        ),
        SbeField::new(
            "MDEntrySize",
            8,
            SbeEncoding::Uint { width: 4 },
            FieldRole::Quantity,
        ),
        SbeField::new("Symbol", 12, SbeEncoding::Ascii { width: 8 }, FieldRole::Symbol),
        SbeField::new("Side", 20, SbeEncoding::Uint { width: 1 }, FieldRole::Side),
        SbeField::new(
            "UpdateAction",
            21,
            SbeEncoding::Uint { width: 1 },
            FieldRole::UpdateAction,
        ),
        SbeField::new(
            "NumberOfOrders",
            22,
            SbeEncoding::Uint { width: 2 },
            FieldRole::OrderCount,
        ),
    ];

    SbeSchema::new(88, 1, ORDER)
        .with_message(SbeMessage {
            template_id: BOOK_TEMPLATE,
            name: "MarketDataIncrementalRefresh".to_string(),
            kind: SbeMessageKind::BookUpdate,
            fields: header.clone(),
            group: Some(SbeGroup {
                name: "MDEntries".to_string(),
                fields: entries,
            }),
        })
        .with_message(SbeMessage {
            template_id: STATUS_TEMPLATE,
            name: "SecurityStatus".to_string(),
            kind: SbeMessageKind::Status,
            fields: {
                let mut fields = header;
                fields.push(SbeField::new(
                    "Symbol",
                    12,
                    SbeEncoding::Ascii { width: 8 },
                    FieldRole::Symbol,
                ));
                fields.push(SbeField::new(
                    "SecurityTradingStatus",
                    20,
                    SbeEncoding::Uint { width: 1 },
                    FieldRole::Status,
                ));
                fields
            },
            group: None,
        })
}

fn decoder() -> SbeDecoder {
    SbeDecoder::new(
        VenueId::new("XCME"),
        "sbe-1",
        InstrumentPartitions::new().with("ESZ4", 11),
        schema(),
    )
    .expect("the schema is well formed")
}

fn at() -> Timestamp {
    Timestamp::from_nanos(1_704_207_845_500_000_000)
}

fn header_block(venue_nanos: u64, sequence: u32, padding: usize) -> Vec<u8> {
    let mut block = Vec::new();
    put_uint(&mut block, ORDER, 8, venue_nanos);
    put_uint(&mut block, ORDER, 4, u64::from(sequence));
    block.resize(block.len() + padding, 0);
    block
}

/// One `MDEntries` entry. `padding` simulates a newer schema's trailing fields;
/// `truncate_to` simulates an older publisher that does not have them all.
fn entry(
    price: &str,
    size: u32,
    symbol: &str,
    side: u8,
    action: u8,
    orders: u16,
    padding: usize,
    truncate_to: Option<usize>,
) -> Vec<u8> {
    let mantissa = Decimal::parse(price).expect("literal").raw();
    let mut bytes = Vec::new();
    put_uint(&mut bytes, ORDER, 8, mantissa as u64);
    put_uint(&mut bytes, ORDER, 4, u64::from(size));
    put_ascii(&mut bytes, 8, symbol);
    put_uint(&mut bytes, ORDER, 1, u64::from(side));
    put_uint(&mut bytes, ORDER, 1, u64::from(action));
    put_uint(&mut bytes, ORDER, 2, u64::from(orders));
    bytes.resize(bytes.len() + padding, 0);
    if let Some(length) = truncate_to {
        bytes.truncate(length);
    }
    bytes
}

fn book_message(version: u16, block_padding: usize, entry_padding: usize) -> Vec<u8> {
    let entries = [
        entry("4512.25", 40, "ESZ4", 0, 0, 3, entry_padding, None),
        entry("4512.50", 25, "ESZ4", 1, 0, 2, entry_padding, None),
    ]
    .concat();
    frame_message(
        &schema(),
        BOOK_TEMPLATE,
        version,
        &header_block(1_704_207_845_000_000_000, 5_000, block_padding),
        Some((24 + entry_padding, 2, &entries)),
    )
}

#[test]
fn a_repeating_group_becomes_one_message_per_entry_sharing_the_venue_sequence() {
    let mut decoder = decoder();
    let messages = decoder.decode(&book_message(1, 0, 0), at()).expect("decodes");

    assert_eq!(messages.len(), 2);
    assert!(messages
        .iter()
        .all(|message| message.origin.sequence == 5_000 && message.origin.partition == 11));
    let MessageBody::LevelSet {
        side,
        price,
        quantity,
        order_count,
    } = &messages[0].body
    else {
        panic!("expected a level");
    };
    assert_eq!(*side, BookSide::Bid);
    assert_eq!(*price, Decimal::parse("4512.25").expect("literal"));
    assert_eq!(*quantity, Decimal::from_int(40));
    assert_eq!(*order_count, Some(3));
}

#[test]
fn a_newer_schema_with_extra_trailing_fields_decodes_identically_rather_than_failing() {
    // The whole point of reading the block length from the message: the venue
    // adds fields on a Sunday and this build keeps running on Monday.
    let baseline = decoder().decode(&book_message(1, 0, 0), at()).expect("decodes");
    let extended = decoder()
        .decode(&book_message(4, 12, 8), at())
        .expect("a message from a newer schema version must still decode");

    assert_eq!(
        baseline, extended,
        "the fields this build knows are in the same place; the rest is stepped over"
    );
}

#[test]
fn a_publisher_still_on_an_older_version_decodes_with_the_fields_it_has() {
    let entries = entry("4512.25", 40, "ESZ4", 0, 0, 0, 0, Some(22));
    let bytes = frame_message(
        &schema(),
        BOOK_TEMPLATE,
        1,
        &header_block(1_704_207_845_000_000_000, 5_001, 0),
        Some((22, 1, &entries)),
    );

    let messages = decoder().decode(&bytes, at()).expect("decodes");
    let MessageBody::LevelSet { order_count, .. } = &messages[0].body else {
        panic!("expected a level");
    };
    assert_eq!(
        *order_count, None,
        "a field the publisher does not send yet is absent, not zero and not an error"
    );
}

#[test]
fn a_delete_action_sets_the_level_to_zero_whatever_size_it_carried() {
    let entries = entry("4512.25", 40, "ESZ4", 0, 2, 3, 0, None);
    let bytes = frame_message(
        &schema(),
        BOOK_TEMPLATE,
        1,
        &header_block(1_704_207_845_000_000_000, 5_002, 0),
        Some((24, 1, &entries)),
    );
    let messages = decoder().decode(&bytes, at()).expect("decodes");
    assert!(matches!(
        messages[0].body,
        MessageBody::LevelSet { quantity, .. } if quantity == Decimal::ZERO
    ));
}

#[test]
fn a_status_template_with_no_group_still_produces_a_message() {
    let mut block = header_block(1_704_207_845_000_000_000, 5_003, 0);
    put_ascii(&mut block, 8, "ESZ4");
    put_uint(&mut block, ORDER, 1, 3);
    let bytes = frame_message(&schema(), STATUS_TEMPLATE, 1, &block, None);

    let messages = decoder().decode(&bytes, at()).expect("decodes");
    assert!(matches!(
        messages[0].body,
        MessageBody::StatusChange {
            status: VenueStatus::Halted
        }
    ));
}

#[test]
fn an_unknown_template_is_skipped_by_its_framed_length_and_the_batch_continues() {
    let unknown = frame_message(
        &schema(),
        999,
        7,
        &header_block(1_704_207_845_000_000_000, 4_999, 16),
        None,
    );
    let bytes = [unknown.clone(), book_message(1, 0, 0)].concat();

    let mut decoder = decoder();
    let messages = decoder.decode(&bytes, at()).expect("the batch survives");
    assert_eq!(messages.len(), 2, "the known template after it decodes");
    assert_eq!(decoder.consumed(), bytes.len());
    assert!(matches!(
        decoder.diagnostics().recent_skips[0].reason,
        SkipReason::UnknownMessageType { .. }
    ));
}

#[test]
fn a_message_that_has_not_finished_arriving_yields_nothing_and_consumes_nothing() {
    let bytes = book_message(1, 0, 0);
    for prefix in 0..bytes.len() {
        let mut decoder = decoder();
        let messages = decoder
            .decode(&bytes[..prefix], at())
            .unwrap_or_else(|error| panic!("prefix of {prefix} bytes errored: {error}"));
        assert!(messages.is_empty(), "{prefix}-byte prefix produced a message");
        assert_eq!(decoder.consumed(), 0, "{prefix}-byte prefix consumed bytes");
    }
}

#[test]
fn a_message_from_a_different_schema_is_refused_rather_than_reinterpreted() {
    let other = SbeSchema::new(99, 1, ORDER);
    let bytes = frame_message(&other, BOOK_TEMPLATE, 1, &header_block(1, 1, 0), None);
    let error = decoder()
        .decode(&bytes, at())
        .expect_err("two schemas that share a template id mean two different messages");
    assert!(error.message().contains("schema"), "{error}");
}

#[test]
fn a_schema_that_cannot_place_a_message_in_time_is_rejected_when_it_is_built() {
    let broken = SbeSchema::new(88, 1, ORDER).with_message(SbeMessage {
        template_id: 1,
        name: "NoClock".to_string(),
        kind: SbeMessageKind::Status,
        fields: vec![SbeField::new(
            "Symbol",
            0,
            SbeEncoding::Ascii { width: 8 },
            FieldRole::Symbol,
        )],
        group: None,
    });
    let error = SbeDecoder::new(
        VenueId::new("XCME"),
        "sbe-1",
        InstrumentPartitions::auto(),
        broken,
    )
    .expect_err("a schema with no venue timestamp cannot produce usable messages");
    assert!(error.message().contains("venue timestamp"), "{error}");
}

#[test]
fn decoding_the_same_bytes_twice_produces_identical_messages() {
    let bytes = [book_message(1, 0, 0), book_message(1, 0, 0)].concat();
    assert_eq!(
        decoder().decode(&bytes, at()).expect("decodes"),
        decoder().decode(&bytes, at()).expect("decodes")
    );
}
