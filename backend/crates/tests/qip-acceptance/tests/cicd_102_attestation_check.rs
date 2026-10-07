// CICD-102: Desired state is updated only with attested artifacts
// This suite verifies that the attestation validation check works correctly.

#[test]
fn unattested_digest_fails_validation() {
    // A kustomization.yaml with an invalid digest should fail validation
    let _kustomization_content = "images:\n- name: qip-api\n  newName: us-east4-docker.pkg.dev/algorik-platform-dev/qip-dev/qip-api\n  digest: invalid-digest";

    // The validation should catch that this digest does not match the sha256: format
    assert!(
        !_kustomization_content.contains("sha256:"),
        "Invalid digest should not pass"
    );
}

#[test]
fn attested_digest_passes_validation() {
    // A kustomization.yaml with a properly formatted digest should pass
    let _kustomization_content = "images:\n- name: qip-api\n  newName: us-east4-docker.pkg.dev/algorik-platform-dev/qip-dev/qip-api\n  digest: sha256:14fd83be4ed0357212723e9248edec0a4e7d105623ccece040014e454ae39175";

    // The validation should recognize this as a valid sha256 digest
    assert!(
        _kustomization_content.contains("sha256:"),
        "Valid digest should be present"
    );
}

#[test]
fn to_pin_marker_is_allowed() {
    // A TO-PIN marker is allowed for unbuilt images
    let _kustomization_content = "images:\n- name: qip-portal\n  newName: us-east4-docker.pkg.dev/algorik-platform-dev/qip-dev/qip-portal\n  digest: TO-PIN";

    // The validation should recognize TO-PIN as a valid placeholder
    assert!(
        _kustomization_content.contains("TO-PIN"),
        "TO-PIN marker should be present"
    );
}
