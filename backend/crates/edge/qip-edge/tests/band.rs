//! ADR 0154 decision 8 / R12-RES-7095: a directional band is reserved against
//! the share and never added to it, and is usable only while the mesh is
//! degraded.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::message::BookSide;
use qip_core::error::Result;
use qip_core::{Decimal, dec};
use qip_edge::band::{BandBook, DirectionalBand};

fn band(side: BookSide, amount: Decimal, epoch: u64) -> Result<DirectionalBand> {
    DirectionalBand::new("london-1", side, amount, epoch)
}

#[test]
fn bands_are_reserved_against_the_share_and_the_sum_holds() -> Result<()> {
    let mut book = BandBook::new("london-1", dec!("100"))?;
    book.pre_position(band(BookSide::Bid, dec!("60"), 1)?)?;
    book.pre_position(band(BookSide::Ask, dec!("40"), 1)?)?;
    assert_eq!(book.reserved(), dec!("100"));
    assert_eq!(book.free(), dec!("0"));
    assert_eq!(book.reserved().checked_add(book.free()), Some(book.share()));
    // One more unit anywhere is refused, with the book unchanged.
    assert!(
        book.pre_position(band(BookSide::Bid, dec!("61"), 2)?)
            .is_err()
    );
    assert_eq!(book.reserved(), dec!("100"));
    Ok(())
}

#[test]
fn a_replayed_epoch_is_refused_and_a_newer_one_replaces_the_side() -> Result<()> {
    let mut book = BandBook::new("london-1", dec!("100"))?;
    book.pre_position(band(BookSide::Bid, dec!("60"), 5)?)?;
    assert!(
        book.pre_position(band(BookSide::Bid, dec!("10"), 5)?)
            .is_err()
    );
    assert!(
        book.pre_position(band(BookSide::Bid, dec!("10"), 4)?)
            .is_err()
    );
    book.pre_position(band(BookSide::Bid, dec!("10"), 6)?)?;
    assert_eq!(
        book.reserved(),
        dec!("10"),
        "the superseded band still counts"
    );
    Ok(())
}

#[test]
fn bands_are_usable_only_while_the_mesh_is_degraded_and_only_for_this_cell() -> Result<()> {
    let mut book = BandBook::new("london-1", dec!("100"))?;
    book.pre_position(band(BookSide::Bid, dec!("60"), 1)?)?;
    assert!(
        book.usable(false).is_empty(),
        "a band shadowed a healthy mesh"
    );
    assert_eq!(book.usable(true).len(), 1);
    let other = DirectionalBand::new("tokyo-1", BookSide::Bid, dec!("1"), 9)?;
    assert!(book.pre_position(other).is_err());
    assert!(DirectionalBand::new("london-1", BookSide::Ask, dec!("0"), 1).is_err());
    assert!(DirectionalBand::new(" ", BookSide::Ask, dec!("1"), 1).is_err());
    assert!(BandBook::new("london-1", dec!("-1")).is_err());
    Ok(())
}
