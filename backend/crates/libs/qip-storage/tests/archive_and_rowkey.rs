//! DATA-070 hotspot-safe row keys and DATA-072 partitioned immutable segments.

use qip_storage::archive_names::{SegmentWriter, SizeBand, object_name};
use qip_storage::rowkey::{parse_row_key, row_key};
use qip_storage::{BlobStore, MemoryBlobStore};

/// Seeded generator: the dependency policy refuses a property-test crate.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 11
    }
}

#[test]
fn no_generated_row_key_begins_with_a_time_component_and_keys_sort_by_entity_first() {
    let entities = ["AAPL", "MSFT", "2024Q1", "BTC-USD", "0", "9999999999"];
    let mut rng = Lcg(7);
    let mut generated = Vec::new();
    for _ in 0..500 {
        let e = entities[(rng.next() % entities.len() as u64) as usize];
        let ts = rng.next() % 4_000_000_000_000;
        generated.push((e, ts, row_key(e, ts).expect("valid")));
    }
    // Premise: the generator produced every entity and many distinct times.
    let distinct: std::collections::BTreeSet<_> = generated.iter().map(|g| g.0).collect();
    assert_eq!(distinct.len(), entities.len());
    for (e, ts, k) in &generated {
        assert!(
            k.starts_with(&format!("{e}#")),
            "{k} must lead with its entity"
        );
        assert_eq!(parse_row_key(k).expect("round trip"), (e.to_string(), *ts));
    }
    // Sorting by key groups entities contiguously and puts newest first within one.
    let mut by_key = generated.clone();
    by_key.sort_by(|a, b| a.2.cmp(&b.2));
    let mut expected = generated.clone();
    expected.sort_by(|a, b| (a.0, std::cmp::Reverse(a.1)).cmp(&(b.0, std::cmp::Reverse(b.1))));
    let lhs: Vec<_> = by_key.iter().map(|g| (g.0, g.1)).collect();
    let rhs: Vec<_> = expected.iter().map(|g| (g.0, g.1)).collect();
    assert_eq!(lhs, rhs);
}

#[test]
fn a_row_key_entity_containing_the_separator_or_empty_is_refused() {
    assert!(row_key("", 1).is_err());
    assert!(row_key("a#b", 1).is_err());
    assert!(parse_row_key("no-separator").is_err());
}

#[test]
fn object_names_are_partitioned_by_source_then_utc_date_then_instrument() {
    // 2024-02-29T23:59:59Z, a leap day, and the second after it.
    let leap = 1_709_251_199;
    assert_eq!(
        object_name("nws", "KJFK", leap, 3).expect("name"),
        "source=nws/date=2024-02-29/instrument=KJFK/segment-000000000003.seg"
    );
    assert!(
        object_name("nws", "KJFK", leap + 1, 3)
            .expect("name")
            .contains("date=2024-03-01")
    );
    // Month and year boundaries, where an off-by-one in the calendar shows.
    for (ts, date) in [
        (0, "1970-01-01"),
        (951_782_400, "2000-02-29"),
        (1_704_067_199, "2023-12-31"),
        (1_704_067_200, "2024-01-01"),
        (1_740_787_200, "2025-03-01"),
        (1_743_379_200, "2025-03-31"),
        (1_756_598_400, "2025-08-31"),
        (1_746_057_600, "2025-05-01"),
        (1_761_955_200, "2025-11-01"),
    ] {
        let name = object_name("s", "i", ts, 0).expect("name");
        assert!(name.contains(&format!("date={date}/")), "{ts} -> {name}");
    }
    assert!(object_name("a/b", "X", 0, 0).is_err());
    assert!(object_name("a", "", 0, 0).is_err());
    assert!(object_name("a", "..", 0, 0).is_err());
}

#[test]
fn a_segment_is_written_once_and_never_overwritten_and_stays_within_its_size_band() {
    let store = MemoryBlobStore::new();
    let w = SegmentWriter::new(&store, SizeBand { min: 4, max: 8 }).expect("band");
    let name = w
        .write("nws", "KJFK", 0, 1, vec![1; 5])
        .expect("first write");
    // Premise: the object exists with the first write's bytes.
    assert_eq!(store.get(&name).expect("get"), Some(vec![1; 5]));
    assert!(
        w.write("nws", "KJFK", 0, 1, vec![2; 5]).is_err(),
        "overwrite refused"
    );
    assert_eq!(
        store.get(&name).expect("get"),
        Some(vec![1; 5]),
        "original untouched"
    );
    assert!(
        w.write("nws", "KJFK", 0, 2, vec![0; 3]).is_err(),
        "below band"
    );
    assert!(
        w.write("nws", "KJFK", 0, 2, vec![0; 9]).is_err(),
        "above band"
    );
    assert!(
        w.write("nws", "KJFK", 0, 2, vec![0; 4]).is_ok(),
        "band is inclusive"
    );
    assert!(SegmentWriter::new(&store, SizeBand { min: 9, max: 8 }).is_err());
}
