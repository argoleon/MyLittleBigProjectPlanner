# Project configurations

A project configuration gathers what a project works with: issue types, fields, workflows, screens and notifications (specification §5). Several projects share one; each project has exactly one. This page explains how its content is stored, for the stories that add that content (L04-US02 onwards).

## Versions

What a configuration holds lives in **versions** (`project_config_version`), never in the configuration row itself:

| State | How many | Who uses it |
| --- | --- | --- |
| `published` | exactly one | the projects and their tickets |
| `draft` | at most one | the configuration admins, who edit it together |
| `retired` | any | nobody: former published versions, kept for history |

- `infra::configs::open_draft` copies the published version into a new draft (`copy_content`).
- `infra::configs::publish` retires the published version and publishes the draft, in the caller's transaction: a failure leaves the published version as it was.
- `infra::configs::discard_draft` deletes the draft; the published version stays.
- The name of a configuration is not content: renaming applies at once.

## Adding a kind of content

Issue types (`issue_type`, L04-US02: `infra::issue_types`, `api::issue_types`) are the first kind of content and the model to follow. A content table (`field_def`, `workflow`…) follows these rules:

1. **It belongs to a version**: a `version_id` referencing `project_config_version (tenant_id, id)` with `ON DELETE CASCADE`, plus `tenant_id` and row-level security like every business table.
2. **Its items keep a stable identifier across versions** (`item_id`), distinct from the row's own `id`: the copy in a new draft gets new rows with the same `item_id`. Tickets reference the stable identifier, so that a publication does not orphan them, and the migration wizard (L04-US08) maps removed items to others.
3. **`copy_content` copies its rows** from one version to the next, keeping `item_id`.
4. **Every edit goes to the draft**: the endpoint opens it if needed (`open_draft`), changes the draft's rows only, then calls `record_change(draft, Item { kind, id })`. The draft chip counts each changed item once, however many edits it had.
5. **Removals tell the publish impact**: `domain::configuration::publish_impact` takes the items the draft removes with the tickets that still use them (`Removal`). Only a removed type or workflow that tickets use calls for the migration wizard (L04-US08); until tickets exist (L05) the count is 0, so nothing blocks a publication.
6. **Defaults**: a new configuration (`infra::configs::insert`) and the tenant's default configuration (`app_seed_default_config`) get the default content of the story, through a SQL function such as `app_seed_default_issue_types(version)`, kept in step with a list in the domain (`domain::issue_types::DEFAULT_TYPES`) by an integration test. A duplicate copies its source instead.

## Workflows

Workflows (L04-US04: `infra::workflows`, `api::workflows`) hold statuses and transitions, all with stable identities; each issue type follows one (`issue_type.workflow_id`). Rules worth knowing:

- **Copies go workflows first, then types**: the types refer to the workflows of their version (`copy_content`), and the seed creates the standard workflow before the default types.
- **A draft may be incomplete**: a new status before its transitions, no initial status for a moment. `domain::workflows::problems` lists what is missing (no status, no initial status, a status nothing leads to, a repeated transition); `GET …/workflows` returns it with each workflow, the publish impact lists it as `workflowProblems`, and publishing answers 409 `workflow.invalid` with each problem as a field error (`workflows.standard.statuses.in_review`).
- **A removed status counts like a removed type**: the publish impact lists it with its tickets, and only statuses in use call for the migration wizard.
- Conditions, validators and post-functions of transitions come with L04-US05, transition screens with US06.

## Fields

Fields (L04-US03: `domain::fields`, `infra::fields`, `api::config_fields`) are the system fields (Points, Remaining work) and the custom ones, with whether each is hidden, visible or required per issue type and workflow step. Rules worth knowing:

- **Copies go workflows, types, then fields**: a field lists its types (`field_def.issue_type_ids`, or `all_types` for every type, the ones added later included), and its rules (`field_rule`) name a type and a status of that type's workflow, or the creation (`status_id` null).
- **Only Hidden and Required cells are stored**: a cell nobody set is Visible, statuses added later included. `domain::fields::mode` resolves a cell; a type the field does not apply to is Hidden everywhere. Tickets (L05) and portal forms (L13) read it, and check their values with `domain::fields::validate_value`.
- **The rules follow the types and statuses they name**: a status or a type removed takes its cells (foreign keys); a type removed also leaves the types of its fields (`infra::fields::forget_type`); a type moved to another workflow loses its cells on the statuses of the former one (`infra::fields::prune_rules`).
- **List values keep their identity**, in `field_def.options` with an `active` flag. A value of the published version that a change leaves out is put back, deactivated (`domain::fields::keep_published`): tickets may hold it, and `Options::value` still resolves it. A value added in the draft and never published goes for good. The API marks each value `published` so the interface knows which.
- **The key and the type of a field are set once.** A system field keeps its label and options too (409 `config_field.system`), and cannot be removed; its types and rules change like any field's.

## Who does what

- Tenant admins and configuration admins (`domain::configuration::manages_configurations`) manage configurations; the others get 403.
- Whoever sees a project reads which configuration it uses (`GET /nodes/{id}/configuration`).
- In the lists of a configuration, a project the member does not see comes veiled: counted, without name or real id.
