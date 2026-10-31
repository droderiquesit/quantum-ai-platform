# The Security Command Center detectors and mute configs, planned
# (R12-SEC-9132).
#
# The paid SCC tier is an organisation activation and stays declined (decision
# 5 of 2026-10-25), so the module ships switched off in every environment.
# What the repository can assert at no cost is what it declares: the two custom
# detectors and the mute configs plan only when the switch is on, and each
# detector watches the cluster field it is named for. A detector whose
# predicate drifted onto another field would read as protection and fire on
# nothing.
#
# Mocked provider, `command = plan`: no credential, no project, nothing billed.
# Both runs carry one muted finding, so "no mute config plans when off" is not
# vacuously true of an empty map.

mock_provider "google" {}

variables {
  project_id = "scc-plan-harness"
  muted_findings = {
    harness-mute = {
      filter      = "category=\"HARNESS\""
      description = "a mute declared so the off run proves the switch, not an empty map"
    }
  }
}

run "the_two_detectors_plan_only_when_switched_on_and_watch_binary_authorization_and_the_private_endpoint" {
  command = plan

  variables {
    enable_security_command_center = true
  }

  assert {
    condition = (
      length(google_scc_management_project_security_health_analytics_custom_module.binary_authorization) == 1
      && length(google_scc_management_project_security_health_analytics_custom_module.private_endpoint) == 1
      && length(google_scc_v2_project_mute_config.muted) == 1
    )
    error_message = "with the switch on, both detectors and the declared mute config must plan"
  }

  assert {
    condition = alltrue([
      for detector in [
        google_scc_management_project_security_health_analytics_custom_module.binary_authorization[0],
        google_scc_management_project_security_health_analytics_custom_module.private_endpoint[0],
      ] : detector.enablement_state == "ENABLED" && detector.custom_config[0].resource_selector[0].resource_types == tolist(["container.googleapis.com/Cluster"])
    ])
    error_message = "each detector must be ENABLED and select GKE clusters"
  }

  assert {
    condition     = strcontains(google_scc_management_project_security_health_analytics_custom_module.binary_authorization[0].custom_config[0].predicate[0].expression, "resource.binaryAuthorization.evaluationMode")
    error_message = "the Binary Authorization detector must read binaryAuthorization.evaluationMode"
  }

  assert {
    condition     = strcontains(google_scc_management_project_security_health_analytics_custom_module.private_endpoint[0].custom_config[0].predicate[0].expression, "resource.privateClusterConfig.enablePrivateEndpoint")
    error_message = "the private-endpoint detector must read privateClusterConfig.enablePrivateEndpoint"
  }
}

run "with_the_switch_off_no_detector_and_no_mute_config_plans" {
  command = plan

  variables {
    enable_security_command_center = false
  }

  assert {
    condition = (
      length(google_scc_management_project_security_health_analytics_custom_module.binary_authorization) == 0
      && length(google_scc_management_project_security_health_analytics_custom_module.private_endpoint) == 0
      && length(google_scc_v2_project_mute_config.muted) == 0
    )
    error_message = "with the switch off, nothing billable may plan, including a declared mute"
  }
}
