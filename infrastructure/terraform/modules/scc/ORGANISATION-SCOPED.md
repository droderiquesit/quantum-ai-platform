# What Security Command Center cannot do from here

This configuration is project-scoped. Every environment is a separate Google
project, because a blast radius that stops at a project boundary is the only
kind that reliably stops, and `infrastructure/environments/README.md` says so.
There is no `org_id`
variable and this module deliberately does not add one: a variable that exists
so a module can claim organisation-level reach, in a repository that has no
organisation-level anything else, would be the beginning of a shell rather than
a capability.

So the honest division is below. The left-hand column is what this module
builds. Everything else is arranged with whoever administers the organisation.

## Built here

**Two Security Health Analytics custom modules.** Project-scoped resources,
evaluated against this project's assets, raising findings with a severity and a
recommendation. They detect a GKE cluster whose Binary Authorization
enforcement has been turned off, and one whose control-plane endpoint has been
made public. Both are properties the acceptance suite already refuses in the
repository and neither is a property anything watches in the project — Terraform
notices at the next plan, which may be a week away and may be run by nobody.

**Mute configurations, if any are declared.** Project-scoped, and empty by
default. They exist so that a decision to stop looking at a class of finding
carries an author, a date and an argument, instead of being a click in a console
that is indistinguishable a year later from a finding nobody ever saw.

**The API.** `securitycenter.googleapis.com`, enabled by `modules/services` when
this module is switched on.

## Not built here, and why not

**Activation, and the tier.** Security Command Center is turned on for an
organisation, not for a project, and the tier — Standard, Premium, Enterprise —
decides which services run underneath it. Custom Security Health Analytics
modules require Premium or Enterprise. Nothing in this module evaluates at
Standard, or at no activation at all, and nothing here can detect which it is:
the resources are accepted and stored either way. That asymmetry is the whole
reason `enable_security_command_center` defaults to false.

**Every built-in detector.** The Security Health Analytics library itself, Event
Threat Detection, Container Threat Detection, VM Threat Detection, Web Security
Scanner. These are organisation-level services with organisation-level
enablement. A project cannot turn one on, and the provider pinned here has no
project-level resource for the service-enablement API either — the
`google_scc_management_*_security_center_service` resources that would express
"this project uses Event Threat Detection" are not in it.

**Getting findings out of SCC.** A notification config streams findings to
Pub/Sub and a BigQuery export writes them to a dataset, and there are
project-scoped forms of both in the provider. They are deliberately not here.

Both publish as a Security Command Center service agent, and that agent is
created by the organisation's activation of SCC. It must be granted
`roles/pubsub.publisher` on the topic, or the dataset role on the export target,
*before* the config is created, or creation fails. This configuration cannot
name that agent: its identity depends on how and at what scope SCC was
activated, which is exactly the fact this module does not have. Guessing the
address of a service account and writing an IAM binding to it produces a
configuration that passes `validate`, passes `plan`, and fails at apply on
something with no obvious connection to what was being built.

So the export path is arranged with the organisation, at the organisation, and
the platform's own alerting continues to come from `modules/observability`,
which does not depend on SCC at all.

**Posture management and organisation-wide mute rules.** Both apply across
projects by definition.

## If you are the organisation administrator

The order that works:

1. Activate SCC at Premium or Enterprise on the organisation.
2. Confirm this project is in scope — activation covers the organisation, but a
   project moved between folders after the fact is worth re-checking.
3. Set `enable_security_command_center = true` here and apply.
4. **Confirm findings actually appear for this project.** "No findings" and "no
   detector running" look identical from a project, and the second one is what
   this module looks like when step 1 has not happened. Break something
   deliberately in a scratch project if you want a positive control.
5. Arrange the export — notification config or BigQuery — at the organisation,
   with the service agent grant, so findings reach somewhere people read.
