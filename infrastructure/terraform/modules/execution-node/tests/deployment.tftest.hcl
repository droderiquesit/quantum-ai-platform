# ADR 0110 decision 5 and ADR 0112, planned rather than read.
#
# The node's environment name is written into node.env as QIP_DEPLOYMENT,
# where the binary reads it to decide whether a shared signing key may stand
# and whether a fabric link may be plaintext. It is a substitution into a
# root-run startup script, so the module holds it to the four Terraform
# environment names. Per ADR 0069 the harness proves both halves: a real
# environment plans and the rendered script carries the line; `local` --
# the one value the binary relaxes anything for, and never a node -- and a
# value carrying a newline are refused at plan time.

mock_provider "google" {}

variables {
  project_id                 = "deployment-plan-harness"
  environment                = "stage"
  node_id                    = "harness"
  region                     = "us-east4"
  zone                       = "us-east4-a"
  network_id                 = "projects/deployment-plan-harness/global/networks/harness"
  subnet_cidr                = "10.0.48.0/24"
  boot_image                 = "projects/deployment-plan-harness/global/images/qip-node-harness"
  venues                     = { "sim" = { cidr = "192.0.2.0/24", port = 443 } }
  egress_endpoints           = { "gcp" = "http://127.0.0.1:9105" }
  capital_envelope_secret_id = "qip-capital-envelope-key"
  region_allocation          = "0.25"
  egress_bootstrap           = "static_resources:\n${join("\n", [for i in range(60) : "  # bootstrap line ${i} keeps this fixture past the one-kilobyte floor the module refuses below"])}\n"
  machine_type               = "c3-highcpu-8"
}

run "a_named_environment_is_written_to_the_node_as_its_deployment" {
  command = plan

  assert {
    condition     = strcontains(google_compute_instance_template.node.metadata["startup-script"], "\nQIP_DEPLOYMENT=stage\n")
    error_message = "the startup script does not write QIP_DEPLOYMENT=stage into node.env, so the binary would refuse to start"
  }
}

run "local_is_refused_because_a_node_is_never_a_developer_machine" {
  command = plan

  variables {
    environment = "local"
  }

  expect_failures = [var.environment]
}

run "a_value_that_would_write_a_second_line_into_node_env_is_refused" {
  command = plan

  variables {
    environment = "dev\nQIP_TLS_MODE=plaintext"
  }

  expect_failures = [var.environment]
}
