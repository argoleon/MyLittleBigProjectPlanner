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

A content table (`issue_type`, `field_def`, `workflow`…) follows these rules:

1. **It belongs to a version**: a `version_id` referencing `project_config_version (tenant_id, id)` with `ON DELETE CASCADE`, plus `tenant_id` and row-level security like every business table.
2. **Its items keep a stable identifier across versions** (`item_id`), distinct from the row's own `id`: the copy in a new draft gets new rows with the same `item_id`. Tickets reference the stable identifier, so that a publication does not orphan them, and the migration wizard (L04-US08) maps removed items to others.
3. **`copy_content` copies its rows** from one version to the next, keeping `item_id`.
4. **Every edit goes to the draft**: the endpoint opens it if needed (`open_draft`), changes the draft's rows only, then calls `record_change(draft, Item { kind, id })`. The draft chip counts each changed item once, however many edits it had.
5. **Removals tell the publish impact**: `domain::configuration::publish_impact` takes the items the draft removes. Removing an issue type or a workflow status calls for the migration wizard before publishing.
6. **Defaults**: a new configuration and the tenant's default configuration (`app_seed_default_config`) get the default content of the story (default types, fields, standard workflow).

## Who does what

- Tenant admins and configuration admins (`domain::configuration::manages_configurations`) manage configurations; the others get 403.
- Whoever sees a project reads which configuration it uses (`GET /nodes/{id}/configuration`).
- In the lists of a configuration, a project the member does not see comes veiled: counted, without name or real id.
