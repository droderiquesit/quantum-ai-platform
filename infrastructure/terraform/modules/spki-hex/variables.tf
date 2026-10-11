variable "pem" {
  description = "A PEM-encoded PUBLIC KEY block."
  type        = string
  nullable    = false

  validation {
    condition     = strcontains(var.pem, "-----BEGIN PUBLIC KEY-----") && strcontains(var.pem, "-----END PUBLIC KEY-----")
    error_message = "pem is a PEM PUBLIC KEY block, as Cloud KMS returns for an asymmetric key version."
  }
}
