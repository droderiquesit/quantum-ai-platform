# ADR 0082 decision 4, planned rather than read.
#
# `isolcpus` keeps the scheduler off the isolated cores and puts nothing on
# them, so a node whose unit carries no `CPUAffinity=` runs its one thread on
# cores 0–1 beside the OS while the range the boot check refuses without sits
# idle. The Rust acceptance suite asserts the template *text*; this file plans
# the module with the google providers mocked and reads the rendered unit off
# the instance template's metadata, so the line is proven at plan time on the
# shape the deployment would use. Per ADR 0069 a harness proves an admission
# and a refusal: the admission is the pinned core rendered for an admitted
# machine shape; the refusal is the machine allowlist, which refuses the
# four-vCPU shape the assignment cannot fit — a harness that admitted every
# shape would prove the line renders and nothing about the machine it lands on.

mock_provider "google" {}
mock_provider "google-beta" {}

variables {
  project_id            = "affinity-plan-harness"
  environment           = "dev"
  node_id               = "harness"
  region                = "us-east4"
  zone                  = "us-east4-a"
  network_id            = "projects/affinity-plan-harness/global/networks/harness"
  subnet_cidr           = "10.0.48.0/24"
  boot_image            = "projects/affinity-plan-harness/global/images/qip-node-harness"
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
}

# --- the admitting half ------------------------------------------------------

run "an_admitted_shape_pins_the_process_onto_the_first_isolated_core" {
  command = plan

  variables {
    machine_type = "c4-highcpu-8"
  }

  assert {
    condition     = output.isolated_cpus == "2-7"
    error_message = "the isolated range for an eight-vCPU shape is not 2-7, so the pin below is asserted against the wrong range"
  }

  # Read off the rendered unit, not off the template file: the metadata value
  # is what the instance boots with.
  assert {
    condition     = strcontains(google_compute_instance_template.node.metadata["startup-script"], "\nCPUAffinity=2\n")
    error_message = "the rendered qip-execution-node.service does not pin the process onto core 2, the first isolated core; the one thread would run on cores 0-1 beside the OS and the isolated range would idle (ADR 0082 decision 4)"
  }

  # The pin is only meaningful beside the refusal it makes true.
  assert {
    condition     = strcontains(google_compute_instance_template.node.metadata["startup-script"], "isolcpus=2-7")
    error_message = "the rendered startup script no longer checks the kernel command line for the isolated range, so the pin above targets cores nothing isolated"
  }
}

# --- the refusing half -------------------------------------------------------

run "a_shape_the_assignment_cannot_fit_is_refused_before_anything_is_planned" {
  command = plan

  variables {
    machine_type = "c4-highcpu-4"
  }

  expect_failures = [var.machine_type]
}

# R12-REFLEX-8002: "a C3 or N2 shape must be refused at plan". One run per
# family the blueprint names, because a list that dropped C3 and kept C3D
# (or the reverse) would pass a single case. Each fails if its shape is put
# back on the permitted list.
run "a_c3_shape_is_refused" {
  command = plan

  variables {
    machine_type = "c3-highcpu-8"
  }

  expect_failures = [var.machine_type]
}

run "a_c3d_shape_is_refused" {
  command = plan

  variables {
    machine_type = "c3d-highcpu-16"
  }

  expect_failures = [var.machine_type]
}

run "an_n2_shape_is_refused" {
  command = plan

  variables {
    machine_type = "n2-highcpu-16"
  }

  expect_failures = [var.machine_type]
}

# And the admitting half on the other C4 family, sixteen vCPU: the shape
# §41.3's core assignment fits, isolating cores 2–15.
run "a_c4d_sixteen_vcpu_shape_is_admitted_and_isolates_cores_2_to_15" {
  command = plan

  variables {
    machine_type = "c4d-highcpu-16"
  }

  assert {
    condition     = google_compute_instance_template.node.machine_type == "c4d-highcpu-16" && strcontains(google_compute_instance_template.node.metadata["startup-script"], "isolcpus=2-15")
    error_message = "A c4d-highcpu-16 node was not planned on that shape with cores 2-15 isolated."
  }
}
