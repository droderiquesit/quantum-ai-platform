variable "project_id" {
  type = string
}

variable "environment" {
  type = string
}

variable "labels" {
  type = map(string)
}

variable "notification_channels" {
  type = list(string)
}
