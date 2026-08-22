variable "project_id" {
  type = string
}

variable "region" {
  type = string
}

variable "environment" {
  type = string
}

variable "labels" {
  type = map(string)
}

variable "network_id" {
  type = string
}

variable "subnet_id" {
  type = string
}

variable "pod_range" {
  type = string
}

variable "service_range" {
  type = string
}

variable "authorised_networks" {
  type = list(object({
    cidr_block   = string
    display_name = string
  }))
}

variable "node_count" {
  type = number
}

variable "machine_type" {
  type = string
}

variable "kms_key_id" {
  type = string
}

variable "service_account" {
  type = string
}
