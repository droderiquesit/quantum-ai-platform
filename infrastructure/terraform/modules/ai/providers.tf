# This module is passed `google-beta` explicitly by the root.
#
# A module that uses a non-default provider without declaring it here inherits
# nothing and fails at plan time with a message about implicit inheritance
# being deprecated. Declaring the requirement is what makes the root's
# `providers = {}` block meaningful.
terraform {
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.12"
    }
    google-beta = {
      source  = "hashicorp/google-beta"
      version = "~> 6.12"
    }
  }
}
