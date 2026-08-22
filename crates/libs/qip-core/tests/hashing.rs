//! SHA-256 against FIPS 180-4 and HMAC-SHA256 against RFC 4231.

use qip_core::hash::{Hasher256, constant_time_eq, from_hex, hmac_sha256, sha256_hex, to_hex};

#[test]
fn sha256_matches_fips_180_4_vectors() {
    let cases: [(&[u8], &str); 4] = [
        (b"", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
        (b"abc", "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
        (
            b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
        ),
        (
            b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu",
            "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1",
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(sha256_hex(input), expected, "sha256({input:?})");
    }
}

#[test]
fn sha256_of_a_million_a_characters() {
    // The FIPS long-message vector, which also exercises multi-block streaming.
    let mut h = Hasher256::new();
    let chunk = vec![b'a'; 1000];
    for _ in 0..1000 {
        h.update(&chunk);
    }
    assert_eq!(
        to_hex(&h.finish()),
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
    );
}

#[test]
fn streaming_matches_one_shot_for_every_chunk_boundary() {
    let data: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
    let expected = sha256_hex(&data);
    // Chunk sizes around the 64-byte block boundary are where padding bugs hide.
    for chunk in [1usize, 7, 63, 64, 65, 127, 128, 200] {
        let mut h = Hasher256::new();
        for part in data.chunks(chunk) {
            h.update(part);
        }
        assert_eq!(to_hex(&h.finish()), expected, "chunked by {chunk}");
    }
}

#[test]
fn hmac_matches_rfc_4231_vectors() {
    // Case 1
    let key = vec![0x0b; 20];
    assert_eq!(
        to_hex(&hmac_sha256(&key, b"Hi There")),
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
    // Case 2
    assert_eq!(
        to_hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
    // Case 3
    let key = vec![0xaa; 20];
    let data = vec![0xdd; 50];
    assert_eq!(
        to_hex(&hmac_sha256(&key, &data)),
        "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe"
    );
    // Case 6 — key longer than the block size, exercising the key-hashing path.
    let key = vec![0xaa; 131];
    assert_eq!(
        to_hex(&hmac_sha256(
            &key,
            b"Test Using Larger Than Block-Size Key - Hash Key First"
        )),
        "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
    );
}

#[test]
fn hex_round_trips() {
    let data: Vec<u8> = (0..=255u8).collect();
    assert_eq!(from_hex(&to_hex(&data)).unwrap(), data);
    assert!(from_hex("abc").is_none(), "odd length must fail");
    assert!(from_hex("zz").is_none(), "non-hex must fail");
}

#[test]
fn constant_time_eq_agrees_with_equality() {
    assert!(constant_time_eq(b"secret", b"secret"));
    assert!(!constant_time_eq(b"secret", b"secrer"));
    assert!(!constant_time_eq(b"secret", b"secre"));
    assert!(constant_time_eq(b"", b""));
}
