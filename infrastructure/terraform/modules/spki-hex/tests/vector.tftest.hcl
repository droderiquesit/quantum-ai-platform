# The PEM-to-hex conversion the trust bundle is rendered with, against a
# fixed vector: a P-256 public key generated for this harness (its private
# half discarded) and the hex `openssl pkey -pubin -outform DER` printed for
# it. A wrong table, a dropped byte or an uncleaned line break changes the
# hex, and a bundle carrying the wrong hex is a fleet refusing every frame.
#
# The module declares no provider and calls none; the mock is here because
# every harness in this repository must be unable to reach a project.

mock_provider "google" {}

run "a_p256_public_key_renders_as_the_hex_openssl_prints" {
  command = plan

  variables {
    pem = "-----BEGIN PUBLIC KEY-----\nMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEhgSGCTRclBKPBmT+NfbgfvTNmTt6\nm3te0sseM4JygV6/IgER1Jo9MZjyiYL/gnD1itHPgIAiBiLrUpQXli8y+A==\n-----END PUBLIC KEY-----\n"
  }

  assert {
    condition     = output.hex == "3059301306072a8648ce3d020106082a8648ce3d0301070342000486048609345c94128f0664fe35f6e07ef4cd993b7a9b7b5ed2cb1e338272815ebf220111d49a3d3198f28982ff8270f58ad1cf8080220622eb529417962f32f8"
    error_message = "The PEM did not render as the DER hex openssl prints for it."
  }

  # The prefix `qip_contracts::trust` checks: an EC P-256 SPKI, 91 bytes.
  assert {
    condition     = startswith(output.hex, "3059301306072a8648ce3d020106082a8648ce3d030107034200") && length(output.hex) == 182
    error_message = "The rendered key is not a 91-byte P-256 SubjectPublicKeyInfo."
  }
}

run "carriage_returns_in_the_pem_do_not_reach_the_hex" {
  command = plan

  variables {
    pem = "-----BEGIN PUBLIC KEY-----\r\nMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEhgSGCTRclBKPBmT+NfbgfvTNmTt6\r\nm3te0sseM4JygV6/IgER1Jo9MZjyiYL/gnD1itHPgIAiBiLrUpQXli8y+A==\r\n-----END PUBLIC KEY-----\r\n"
  }

  assert {
    condition     = output.hex == "3059301306072a8648ce3d020106082a8648ce3d0301070342000486048609345c94128f0664fe35f6e07ef4cd993b7a9b7b5ed2cb1e338272815ebf220111d49a3d3198f28982ff8270f58ad1cf8080220622eb529417962f32f8"
    error_message = "A CRLF PEM rendered differently from the LF one."
  }
}

run "something_that_is_not_a_public_key_is_refused" {
  command = plan

  variables {
    pem = "-----BEGIN CERTIFICATE-----\nMFkw\n-----END CERTIFICATE-----\n"
  }

  expect_failures = [var.pem]
}
