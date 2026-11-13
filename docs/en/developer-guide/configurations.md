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

## Who does what

- Tenant admins and configuration admins (`domain::configuration::manages_configurations`) manage configurations; the others get 403.
- Whoever sees a project reads which configuration it uses (`GET /nodes/{id}/configuration`).
- In the lists of a configuration, a project the member does not see comes veiled: counted, without name or real id.
