# What has to be true before this module can run

This module manages API enablement so that a first apply into a fresh project
does not stop halfway. It cannot manage its own preconditions, and the list is
short enough to be worth stating exactly rather than discovering.

## Two APIs must already be on

`google_project_service` is a call to Service Usage, and Service Usage cannot
enable itself. Cloud Resource Manager is needed to resolve the project the call
names.

    gcloud services enable serviceusage.googleapis.com \
                           cloudresourcemanager.googleapis.com \
                           --project <project_id>

Both are enabled by default on a project created through the console. A project
created by an automated factory, or one that has been hardened, may have
neither — which produces a `SERVICE_DISABLED` error naming `serviceusage`
itself, on the first resource in the first module, and reads as though nothing
works.

Both appear in `local.always` as well. That is deliberate and not a
contradiction: the resource adopts an already-enabled API without a call, and
having them under management is what stops a later change turning them off.

## The identity applying this needs to be allowed to enable services

`roles/serviceusage.serviceUsageAdmin` on the project, or a role containing
`serviceusage.services.enable`. The deploy account created in `modules/cicd`
does **not** hold it, and should not: enabling an API is a change to the
project's attack surface, and a pipeline that can widen that without review is
the thing several other decisions in this repository exist to prevent.

The consequence is that the **first** apply into a new project is done by a
human with project-level rights, and subsequent applies by the pipeline are
no-ops against this module. If the pipeline ever plans a change here, that is a
signal worth reading rather than a permission to grant.

## Billing must be enabled on the project

Several of these APIs — `container`, `cloudkms`, `artifactregistry` — refuse to
enable on a project with no billing account. The error names billing rather
than the API, which is at least honest.

## What this module deliberately does not do

**It does not create the project.** A project is created inside a folder,
under an organisation, with a billing account and a set of org policies, none
of which this configuration has a variable for. `env/README.md` already says
every environment should be a separate project for blast-radius reasons; which
folder it lives in and what constrains it is a landing-zone decision this
repository does not make.

**It does not enable an API for something that is switched off.** The
conditional block mirrors the root's flags. An operator who turns on
`enable_bigquery` gets `bigquery.googleapis.com` in the same plan, and an
operator who turns it off gets neither the dataset nor the API — but see
`disable_services_on_destroy`, which is why the API stays enabled until
somebody removes it deliberately.

**It does not enable `securitycenter.googleapis.com` unconditionally.** The
project half of Security Command Center is reachable from here; the activation
that makes it produce findings is not. `modules/scc/ORGANISATION-SCOPED.md`
draws that line precisely.
