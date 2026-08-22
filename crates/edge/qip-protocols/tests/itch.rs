//! ITCH 5.0 decoding: fixed-width framing, order-state tracking and refusal.

use qip_contracts::{BookSide, MessageBody, TradeCondition, VenueId, VenueStatus};
use qip_core::{Decimal, Timestamp};
use qip_protocols::decoder::SkipReason;
use qip_protocols::{Decoder, InstrumentPartitions, ItchDecoder};

fn put(buffer: &mut Vec<u8>, width: usize, value: u64) {
    for index in (0..width).rev() {
        buffer.push(((value >> (8 * index)) & 0xFF) as u8);
    }
}

fn put_stock(buffer: &mut Vec<u8>, stock: &str) {
    let bytes = stock.as_bytes();
    for index in 0..8 {
        buffer.push(bytes.get(index).copied().unwrap_or(b' '));
    }
}

fn header(kind: u8, nanos: u64) -> Vec<u8> {
    let mut out = vec![kind];
    put(&mut out, 2, 1); // stock locate
    put(&mut out, 2, 0); // tracking number
    put(&mut out, 6, nanos);
    out
}

fn add_order(order_ref: u64, side: u8, shares: u32, stock: &str, price_ticks: u32) -> Vec<u8> {
    let mut out = header(b'A', 100);
    put(&mut out, 8, order_ref);
    out.push(side);
    put(&mut out, 4, u64::from(shares));
    put_stock(&mut out, stock);
    put(&mut out, 4, u64::from(price_ticks));
    out
}

fn executed(order_ref: u64, shares: u32) -> Vec<u8> {
    let mut out = header(b'E', 200);
    put(&mut out, 8, order_ref);
    put(&mut out, 4, u64::from(shares));
    put(&mut out, 8, 55); // match number
    out
}

fn executed_with_price(order_ref: u64, shares: u32, price_ticks: u32, printable: bool) -> Vec<u8> {
    let mut out = header(b'C', 210);
    put(&mut out, 8, order_ref);
    put(&mut out, 4, u64::from(shares));
    put(&mut out, 8, 56);
    out.push(if printable { b'Y' } else { b'N' });
    put(&mut out, 4, u64::from(price_ticks));
    out
}

fn cancel(order_ref: u64, shares: u32) -> Vec<u8> {
    let mut out = header(b'X', 220);
    put(&mut out, 8, order_ref);
    put(&mut out, 4, u64::from(shares));
    out
}

fn delete(order_ref: u64) -> Vec<u8> {
    let mut out = header(b'D', 230);
    put(&mut out, 8, order_ref);
    out
}

fn replace(original: u64, new: u64, shares: u32, price_ticks: u32) -> Vec<u8> {
    let mut out = header(b'U', 240);
    put(&mut out, 8, original);
    put(&mut out, 8, new);
    put(&mut out, 4, u64::from(shares));
    put(&mut out, 4, u64::from(price_ticks));
    out
}

fn trading_action(stock: &str, state: u8) -> Vec<u8> {
    let mut out = header(b'H', 250);
    put_stock(&mut out, stock);
    out.push(state);
    out.push(b' ');
    out.extend_from_slice(b"    ");
    out
}

fn stock_directory(stock: &str) -> Vec<u8> {
    let mut out = header(b'R', 50);
    put_stock(&mut out, stock);
    out.resize(39, 0);
    out
}

fn decoder() -> ItchDecoder {
    ItchDecoder::new(
        VenueId::new("XNAS"),
        "itch-a",
        InstrumentPartitions::new().with("AAPL", 3),
        Timestamp::from_civil(2024, 1, 2),
    )
}

fn at() -> Timestamp {
    Timestamp::from_nanos(1_704_153_600_000_000_500)
}

#[test]
fn an_add_carries_the_price_the_venue_scaled_by_four_decimals() {
    let mut decoder = decoder();
    let messages = decoder
        .decode(&add_order(1, b'B', 500, "AAPL", 1_002_500), at())
        .expect("a whole add decodes");

    assert_eq!(messages.len(), 1);
    let MessageBody::OrderAdded {
        side,
        price,
        quantity,
        ..
    } = &messages[0].body
    else {
        panic!("expected an add");
    };
    assert_eq!(*side, BookSide::Bid);
    assert_eq!(*price, Decimal::parse("100.25").expect("literal"));
    assert_eq!(*quantity, Decimal::from_int(500));
    assert_eq!(messages[0].origin.partition, 3);
    assert_eq!(
        messages[0].venue_time,
        Timestamp::from_civil(2024, 1, 2).saturating_add(qip_core::Duration::from_nanos(100)),
        "the session date comes from the caller, never from a clock"
    );
}

#[test]
fn an_execution_states_what_is_left_of_the_order_rather_than_what_was_taken() {
    // The contract expresses a reduction absolutely. ITCH publishes a delta, so
    // the decoder has to hold the resting size; this is the property that says
    // it does.
    let mut decoder = decoder();
    let bytes = [
        add_order(1, b'B', 500, "AAPL", 1_002_500),
        executed(1, 200),
        executed(1, 100),
    ]
    .concat();

    let messages = decoder.decode(&bytes, at()).expect("decodes");
    let remaining: Vec<Decimal> = messages
        .iter()
        .filter_map(|message| match &message.body {
            MessageBody::OrderReduced { remaining, .. } => Some(*remaining),
            _ => None,
        })
        .collect();
    assert_eq!(
        remaining,
        vec![Decimal::from_int(300), Decimal::from_int(200)],
        "500 - 200 = 300, then 300 - 100 = 200"
    );
}

#[test]
fn an_execution_reduces_the_book_before_it_prints_the_trade() {
    let mut decoder = decoder();
    let bytes = [add_order(1, b'B', 500, "AAPL", 1_002_500), executed(1, 200)].concat();
    let messages = decoder.decode(&bytes, at()).expect("decodes");

    assert_eq!(messages.len(), 3);
    assert!(matches!(messages[1].body, MessageBody::OrderReduced { .. }));
    let MessageBody::Trade {
        price,
        quantity,
        aggressor,
        ..
    } = &messages[2].body
    else {
        panic!("an execution prints");
    };
    assert_eq!(*price, Decimal::parse("100.25").expect("literal"));
    assert_eq!(*quantity, Decimal::from_int(200));
    assert_eq!(
        *aggressor,
        Some(BookSide::Ask),
        "the resting order was a bid, so the aggressor sold"
    );
    assert_eq!(
        messages[1].origin.sequence, messages[2].origin.sequence,
        "both facts came from one wire message and share its position"
    );
}

#[test]
fn a_non_printable_execution_moves_the_book_without_moving_the_last_price() {
    let mut decoder = decoder();
    let bytes = [
        add_order(1, b'B', 500, "AAPL", 1_002_500),
        executed_with_price(1, 200, 1_002_400, false),
    ]
    .concat();
    let messages = decoder.decode(&bytes, at()).expect("decodes");

    assert_eq!(messages.len(), 2, "the reduction, and no print");
    assert!(
        messages
            .iter()
            .all(|message| !matches!(message.body, MessageBody::Trade { .. }))
    );
}

#[test]
fn an_order_fully_executed_leaves_the_book_rather_than_resting_at_zero() {
    let mut decoder = decoder();
    let bytes = [add_order(1, b'S', 100, "AAPL", 1_003_000), executed(1, 100)].concat();
    let messages = decoder.decode(&bytes, at()).expect("decodes");

    assert!(matches!(messages[1].body, MessageBody::OrderRemoved { .. }));
    assert_eq!(
        decoder.open_order_count(),
        0,
        "a fully executed order must not be remembered, or the table grows all session"
    );
}

#[test]
fn a_cancel_reduces_and_a_delete_removes() {
    let mut decoder = decoder();
    let bytes = [
        add_order(1, b'B', 500, "AAPL", 1_002_500),
        cancel(1, 200),
        delete(1),
    ]
    .concat();
    let messages = decoder.decode(&bytes, at()).expect("decodes");

    assert!(matches!(
        messages[1].body,
        MessageBody::OrderReduced {
            remaining: q,
            ..
        } if q == Decimal::from_int(300)
    ));
    assert!(matches!(messages[2].body, MessageBody::OrderRemoved { .. }));
    assert_eq!(decoder.open_order_count(), 0);
}

#[test]
fn a_replace_removes_the_old_reference_and_adds_the_new_one() {
    // Nasdaq's replace assigns a new reference and loses queue priority, so a
    // book that treated it as an amendment would hold a place in the queue the
    // order no longer has.
    let mut decoder = decoder();
    let bytes = [
        add_order(1, b'B', 500, "AAPL", 1_002_500),
        replace(1, 2, 400, 1_002_600),
        executed(2, 100),
    ]
    .concat();
    let messages = decoder.decode(&bytes, at()).expect("decodes");

    assert!(matches!(
        messages[1].body,
        MessageBody::OrderRemoved { order_ref: 1 }
    ));
    assert!(matches!(
        messages[2].body,
        MessageBody::OrderAdded { order_ref: 2, .. }
    ));
    assert!(
        matches!(
            messages[3].body,
            MessageBody::OrderReduced {
                order_ref: 2,
                remaining: q
            } if q == Decimal::from_int(300)
        ),
        "the replacement's size has to be tracked under its new reference"
    );
}

#[test]
fn a_trading_action_becomes_a_status_change_the_router_can_read() {
    let mut decoder = decoder();
    let messages = decoder
        .decode(&trading_action("AAPL", b'H'), at())
        .expect("decodes");
    assert!(matches!(
        messages[0].body,
        MessageBody::StatusChange {
            status: VenueStatus::Halted
        }
    ));

    let messages = decoder
        .decode(&trading_action("AAPL", b'Q'), at())
        .expect("decodes");
    let MessageBody::StatusChange { status } = messages[0].body else {
        panic!("expected a status change");
    };
    assert!(
        status.accepts_orders() && !matches!(status, VenueStatus::Open),
        "a quotation-only period accepts orders but is not continuous trading"
    );
}

#[test]
fn a_cross_prints_as_an_auction_so_it_cannot_drag_the_continuous_mark() {
    let mut out = header(b'Q', 300);
    put(&mut out, 8, 10_000);
    put_stock(&mut out, "AAPL");
    put(&mut out, 4, 1_005_000);
    put(&mut out, 8, 77);
    out.push(b'O');

    let mut decoder = decoder();
    let messages = decoder.decode(&out, at()).expect("decodes");
    let MessageBody::Trade {
        condition,
        aggressor,
        ..
    } = &messages[0].body
    else {
        panic!("a cross prints");
    };
    assert_eq!(*condition, TradeCondition::Auction);
    assert!(condition.updates_last());
    assert_eq!(*aggressor, None, "a cross has no aggressor");
}

#[test]
fn a_message_that_has_not_finished_arriving_yields_nothing_and_consumes_nothing() {
    let bytes = add_order(1, b'B', 500, "AAPL", 1_002_500);
    for prefix in 0..bytes.len() {
        let mut decoder = decoder();
        let messages = decoder
            .decode(&bytes[..prefix], at())
            .unwrap_or_else(|error| panic!("prefix of {prefix} bytes errored: {error}"));
        assert!(
            messages.is_empty(),
            "{prefix}-byte prefix produced a message"
        );
        assert_eq!(decoder.consumed(), 0, "{prefix}-byte prefix consumed bytes");
    }
}

#[test]
fn a_known_message_type_with_no_market_meaning_is_stepped_over_by_its_length() {
    let bytes = [
        stock_directory("AAPL"),
        add_order(1, b'B', 500, "AAPL", 1_002_500),
    ]
    .concat();
    let mut decoder = decoder();
    let messages = decoder.decode(&bytes, at()).expect("the batch survives");

    assert_eq!(
        messages.len(),
        1,
        "the add after the directory entry decodes"
    );
    assert_eq!(decoder.consumed(), bytes.len());
    assert!(matches!(
        decoder.diagnostics().recent_skips[0].reason,
        SkipReason::NoMarketFact { .. }
    ));
}

#[test]
fn a_type_that_is_not_itch_at_all_is_refused_because_the_next_boundary_is_unknowable() {
    // The contrast with the case above: an unmapped type of known length costs
    // one message, an unknown type costs the session, so they cannot be handled
    // the same way.
    let mut bytes = add_order(1, b'B', 500, "AAPL", 1_002_500);
    bytes[0] = b'z';
    let error = decoder().decode(&bytes, at()).expect_err("refused");
    assert!(error.message().contains('z'), "{error}");
}

#[test]
fn an_execution_against_an_order_the_session_never_saw_is_recorded_as_lost_information() {
    let mut decoder = decoder();
    let messages = decoder
        .decode(&executed(999, 100), at())
        .expect("framing is fine");
    assert!(messages.is_empty());
    assert!(matches!(
        decoder.diagnostics().recent_skips[0].reason,
        SkipReason::UnknownOrderReference { order_ref: 999 }
    ));
    assert!(
        decoder.diagnostics().lost_information(),
        "a book is now missing an update and the operator has to be able to see that"
    );
}

#[test]
fn each_wire_message_advances_the_sequence_by_exactly_one() {
    let mut decoder = decoder();
    decoder.set_sequence(1_000);
    let bytes = [
        add_order(1, b'B', 500, "AAPL", 1_002_500),
        add_order(2, b'S', 200, "AAPL", 1_003_000),
        executed(1, 100),
    ]
    .concat();
    let messages = decoder.decode(&bytes, at()).expect("decodes");
    let sequences: Vec<u64> = messages
        .iter()
        .map(|message| message.origin.sequence)
        .collect();
    assert_eq!(
        sequences,
        vec![1_000, 1_001, 1_002, 1_002],
        "the execution's two facts share the position of the packet that carried them"
    );
}

#[test]
fn decoding_the_same_capture_twice_produces_identical_messages() {
    let bytes = [
        add_order(1, b'B', 500, "AAPL", 1_002_500),
        executed(1, 200),
        replace(1, 2, 300, 1_002_600),
        delete(2),
    ]
    .concat();
    assert_eq!(
        decoder().decode(&bytes, at()).expect("decodes"),
        decoder().decode(&bytes, at()).expect("decodes")
    );
}
