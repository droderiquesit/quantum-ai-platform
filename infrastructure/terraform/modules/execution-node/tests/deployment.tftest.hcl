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
  project_id            = "deployment-plan-harness"
  environment           = "stage"
  node_id               = "harness"
  region                = "us-east4"
  zone                  = "us-east4-a"
  network_id            = "projects/deployment-plan-harness/global/networks/harness"
  subnet_cidr           = "10.0.48.0/24"
  boot_image            = "projects/deployment-plan-harness/global/images/qip-node-harness"
  venues                = { "sim" = { cidr = "192.0.2.0/24", port = 443 } }
  egress_endpoints      = { "gcp" = "http://127.0.0.1:9105" }
  trust_bundle_path     = "/etc/qip/trust/harness.json"
  google_tls_roots_path = "/etc/qip/trust/google-roots.pem"
  workload_certificates = {
    ca_pool              = "projects/harness-project/locations/us-east4/caPools/qip-harness-internal"
    certificate_template = "projects/harness-project/locations/us-east4/certificateTemplates/qip-harness-workload"
    lifetime_secs        = 86400
    trust_roots_pem      = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n"
  }
  region_allocation = "0.25"
  egress_bootstrap  = "static_resources:\n${join("\n", [for i in range(60) : "  # bootstrap line ${i} keeps this fixture past the one-kilobyte floor the module refuses below"])}\n"
  machine_type      = "c4-highcpu-8"
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

# Items 10 and 12: the node verifies against the baked trust bundle, speaks
# mutual TLS on a CA Service workload certificate, and holds no shared key —
# neither fetched nor named.
run "the_node_is_given_the_trust_bundle_and_its_workload_certificate_and_no_shared_key" {
  command = plan

  assert {
    condition = alltrue([
      for line in [
        "\nQIP_TRUST_BUNDLE_PATH=/etc/qip/trust/harness.json\n",
        "\nQIP_TLS_MODE=cas\n",
        "\nQIP_CAS_CA_POOL=projects/harness-project/locations/us-east4/caPools/qip-harness-internal\n",
        "\nQIP_CAS_CERTIFICATE_TEMPLATE=projects/harness-project/locations/us-east4/certificateTemplates/qip-harness-workload\n",
        "\nQIP_CAS_LIFETIME_SECS=86400\n",
        "\nQIP_TLS_TRUST_ROOTS_PATH=$CONF_DIR/tls-trust-roots.pem\n",
        "\nQIP_GOOGLE_TLS_ROOTS_PATH=/etc/qip/trust/google-roots.pem\n",
      ] : strcontains(google_compute_instance_template.node.metadata["startup-script"], line)
    ])
    error_message = "node.env does not carry the trust bundle, the CA Service settings and the pinned roots the binary refuses to start without."
  }

  assert {
    condition = (
      !strcontains(google_compute_instance_template.node.metadata["startup-script"], "QIP_CAPITAL_ENVELOPE_KEY")
      && !strcontains(google_compute_instance_template.node.metadata["startup-script"], "capital-envelope-key")
    )
    error_message = "The node still fetches or names the shared capital-envelope key; holding it beside the bundle is refused by the binary and keeps the secret ADR 0110 removes."
  }
}

run "a_trust_bundle_outside_the_baked_directory_is_refused" {
  command = plan

  variables {
    trust_bundle_path = "/tmp/bundle.json"
  }

  expect_failures = [var.trust_bundle_path]
}

run "a_workload_certificate_living_longer_than_a_day_is_refused" {
  command = plan

  variables {
    workload_certificates = {
      ca_pool              = "projects/harness-project/locations/us-east4/caPools/qip-harness-internal"
      certificate_template = "projects/harness-project/locations/us-east4/certificateTemplates/qip-harness-workload"
      lifetime_secs        = 2592000
      trust_roots_pem      = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n"
    }
  }

  expect_failures = [var.workload_certificates]
}

run "a_ca_pool_name_carrying_shell_syntax_is_refused" {
  command = plan

  variables {
    workload_certificates = {
      ca_pool              = "projects/harness-project/locations/us-east4/caPools/x$(reboot)"
      certificate_template = "projects/harness-project/locations/us-east4/certificateTemplates/qip-harness-workload"
      lifetime_secs        = 86400
      trust_roots_pem      = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n"
    }
  }

  expect_failures = [var.workload_certificates]
}
