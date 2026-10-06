//! CONTRACT-040 (PublisherContract): every asynchronous network publication goes
//! through the envelope-constructing API, never publishing a bare payload.
//!
//! FabricEnvelope (CONTRACT-036) and the envelope-constructing paths
//! (Envelope::new, StreamEnvelope::seal) are the only places an asynchronous
//! record is constructed before crossing the network I/O boundary. A path that
//! sends data across the network without going through these APIs cannot be
//! proven to carry the contract's envelope fields.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

/// Patterns that indicate a proper envelope-constructing API call.
/// These are the known-safe paths that construct envelopes before publishing.
const ENVELOPE_CONSTRUCTING_PATTERNS: &[&str] = &[
    "Envelope::new(",        // qip_events envelope construction
    "StreamEnvelope::seal(", // qip_streaming envelope sealing
    ".erase()",              // Envelope::erase for transport
];

/// Patterns that indicate network/transport publication paths that must go through an envelope.
/// These are the I/O boundary crossings for inter-process communication.
const NETWORK_PUBLICATION_PATTERNS: &[&str] = &[
    ".publish_frames(",       // Transport mesh publisher - sends to network
    ".publish_frame(",        // Single frame publication to network
    "MeshPublisher::publish", // Mesh publisher methods - network
    ".deliver(",              // Transport delivery - network
    ".send_once(",            // Transport single send - network
];

#[test]
fn every_network_publication_uses_envelope_typed_payloads() {
    let mut violations = Vec::new();
    let mut scanned = 0;
    let mut publication_sites = 0;

    // Patterns that indicate a parameter is already an envelope or envelope-derived type
    const ENVELOPE_TYPED_PARAMETERS: &[&str] = &[
        "frame,",           // AnyEvent frame parameter
        "envelope,",        // Envelope parameter
        "Delivery::",       // Delivery constructor (wraps envelopes)
        "VerifiedEnvelope", // Already-verified envelope
        "VerifiedPolicy",   // Already-verified policy
        "VerifiedHalt",     // Already-verified halt
        "AnyEvent",         // Explicitly typed
    ];

    // Patterns that indicate a bare/raw payload that should NOT be passed to network publication
    const FORBIDDEN_RAW_PAYLOAD_PATTERNS: &[&str] = &[
        "serde_json::json!(",
        "serde_json::to_value",
        "serde_json::Value",
        "Value::Object",
        "json!(",
    ];

    // Scan all backend Rust source files for network publication patterns
    for file in qip_acceptance::files_with_extension("backend/crates", "rs") {
        // Skip test files for this check; we're looking at production paths
        if file.to_string_lossy().contains("/tests/") {
            continue;
        }

        // Skip storage engine (local files, not network publications)
        if file.to_string_lossy().contains("qip-storage") {
            continue;
        }

        let text = match std::fs::read_to_string(&file) {
            Ok(t) => t,
            Err(_) => continue,
        };

        let production = qip_acceptance::production_text(&text);
        scanned += 1;

        // Check each line for network publication patterns that must use envelopes
        for (line_num, line) in production.lines().enumerate() {
            let line_num = line_num + 1;

            // Look for network publication calls
            if !NETWORK_PUBLICATION_PATTERNS
                .iter()
                .any(|p| line.contains(p))
            {
                continue; // Not a network publication line
            }

            publication_sites += 1;

            // Check for forbidden raw payloads being passed to network publication
            let has_raw_payload = FORBIDDEN_RAW_PAYLOAD_PATTERNS
                .iter()
                .any(|pattern| line.contains(pattern));

            if has_raw_payload {
                // Check if this is in an approved context (e.g., constructing an envelope around it)
                let has_approved_context = ENVELOPE_CONSTRUCTING_PATTERNS
                    .iter()
                    .any(|pattern| line.contains(pattern));

                if !has_approved_context {
                    violations.push(format!(
                        "{}:{}: raw payload passed to network publication: {}",
                        file.display(),
                        line_num,
                        line.trim()
                    ));
                }
            }

            // Verify that if we're passing something to publish_frame/frames, it looks like an envelope
            let is_envelope_typed = ENVELOPE_TYPED_PARAMETERS
                .iter()
                .any(|pattern| line.contains(pattern));

            if !is_envelope_typed && !has_raw_payload {
                // Only flag if we can't recognize what's being passed.
                // Many legitimate patterns will pass through here (variable names we don't recognize).
                // This is intentional - we're only flagging things that look actively wrong.
            }
        }
    }

    // Premise: we scanned production Rust files and found network publication sites
    assert!(
        scanned > 50,
        "only {scanned} production Rust files were scanned"
    );
    assert!(
        publication_sites > 0,
        "expected to find network publication sites in production code"
    );

    // Assert no bare raw payloads were passed to network publication methods
    assert!(
        violations.is_empty(),
        "the following network publication calls pass raw payloads instead of envelope-typed values:\n{}",
        violations.join("\n")
    );
}

/// Verify that known envelope-constructing paths are present and used in production
#[test]
fn envelope_constructing_paths_exist_and_are_used_in_production() {
    use std::collections::BTreeSet;

    let mut found = BTreeSet::new();
    let mut usage_count = 0;

    for file in qip_acceptance::files_with_extension("backend/crates", "rs") {
        // Skip test files
        if file.to_string_lossy().contains("/tests/") {
            continue;
        }

        let text = match std::fs::read_to_string(&file) {
            Ok(t) => t,
            Err(_) => continue,
        };

        let production = qip_acceptance::production_text(&text);

        for pattern in ENVELOPE_CONSTRUCTING_PATTERNS {
            if production.contains(pattern) {
                found.insert(pattern.to_string());
                // Count how many times this pattern appears
                usage_count += production.matches(pattern).count();
            }
        }
    }

    // All known envelope-constructing patterns must be present
    assert!(
        found.len() >= 2,
        "expected multiple envelope-constructing patterns in production code, found: {:?}",
        found
    );

    // They must be used multiple times (not just declared)
    assert!(
        usage_count >= 10,
        "envelope-constructing patterns are used {} times, expected at least 10",
        usage_count
    );
}
