# A PEM `PUBLIC KEY` (the `SubjectPublicKeyInfo` Cloud KMS's getPublicKey
# returns) as the lowercase hex of its DER — the form
# `qip_contracts::trust::public_point` reads out of a trust bundle.
#
# A module of its own for one reason: the conversion is the one piece of the
# trust bundle a plan can check against a fixed vector. The keys themselves
# are unknown until KMS creates them, so a harness of the root can never see
# a real conversion; this module's harness feeds a fixed PEM and asserts the
# hex `openssl pkey -pubin -outform DER` printed for it.
#
# HCL has no hex encoder. The DER is decoded as ISO-8859-1 — one character per
# byte, all 256 defined — and each character is looked up in a table built the
# same way from the 256 byte values in order. No provider, no resource.

locals {
  # base64 of the bytes 0x00 through 0xff, in order.
  byte_values_base64 = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKissLS4vMDEyMzQ1Njc4OTo7PD0+P0BBQkNERUZHSElKS0xNTk9QUVJTVFVWV1hZWltcXV5fYGFiY2RlZmdoaWprbG1ub3BxcnN0dXZ3eHl6e3x9fn+AgYKDhIWGh4iJiouMjY6PkJGSk5SVlpeYmZqbnJ2en6ChoqOkpaanqKmqq6ytrq+wsbKztLW2t7i5uru8vb6/wMHCw8TFxsfIycrLzM3Oz9DR0tPU1dbX2Nna29zd3t/g4eLj5OXm5+jp6uvs7e7v8PHy8/T19vf4+fr7/P3+/w=="

  hex_of_byte = {
    for index, character in split("", textdecodebase64(local.byte_values_base64, "ISO-8859-1")) :
    character => format("%02x", index)
  }

  body = replace(replace(replace(replace(var.pem,
    "-----BEGIN PUBLIC KEY-----", ""),
    "-----END PUBLIC KEY-----", ""),
  "\n", ""), "\r", "")

  hex = join("", [
    for character in split("", textdecodebase64(local.body, "ISO-8859-1")) :
    local.hex_of_byte[character]
  ])
}

