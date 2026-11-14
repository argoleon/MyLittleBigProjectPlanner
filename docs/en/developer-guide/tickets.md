# Tickets

A ticket (`issue`) belongs to one project and follows the **published** version of that project's configuration (specification §5, §15). This page explains how tickets are stored and created (L05-US01), for the stories that edit, link, comment on and notify about them.

| Layer | File | Holds |
| --- | --- | --- |
| Rules | `backend/crates/domain/src/issues.rs` | Keys, priority, `validate_new` (summary, type, parent, dates, custom fields) |
| Rules | `backend/crates/domain/src/rich_text.rs` | The rich text document, its limits and its HTML rendering |
| Tables | `backend/migrations/*_issues.sql` | `issue`, `project_issue_counter`, `issue_key_alias`, `external_ref`, `outbox_event` |
| SQL | `backend/crates/infra/src/issues.rs` | `create` (with its key), `find`, `id_of_key`, `of_account` |
| SQL | `backend/crates/infra/src/rich_text.rs` | Tiptap JSON read into the document, with an allow-list |
| SQL | `backend/crates/infra/src/outbox.rs` | `record`, `ensure_partitions` |
| Job | `backend/crates/worker/src/lib.rs` | `keep_outbox_partitions`, every 12 hours |
| HTTP | `backend/crates/api/src/issues.rs` | Creation, reading, the creation form, "my tickets", assignable people |

## What a ticket references

- **Type and status** are the stable `item_id`s of the configuration (see [Project configurations](configurations.md)), never the rows of a version: a publication does not orphan tickets. A type or status the published version no longer has is the job of the migration wizard (#125); until then, ticket counts in the publish impact and the migration plan are still 0.
- **Custom field values** are in the `fields` JSONB column, keyed by the field's `item_id`, as the field takes them (`infra::fields::value_of`): text, number, boolean, list value ids, account ids. Only values given are stored. A GIN index (`jsonb_path_ops`) serves the search of L06.
- **Priority** is a fixed list of five levels (`highest` … `lowest`, `medium` by default), not part of configurations.
- **The parent** is a ticket of the same project; the type hierarchy of the configuration says which types a type may sit under, and a sub-task needs one.
- **People** (`reporter_id`, `assignee_id`) reference accounts with `ON DELETE SET NULL`: an erased account leaves its tickets.

## Keys

A ticket's key is its project key and a number (`PLAT-142`), unique in the tenant. `infra::issues::create` takes the next number from `project_issue_counter` with an `INSERT … ON CONFLICT DO UPDATE … RETURNING`: the counter row stays locked until the transaction ends, so two creations in one project never share a number. A rolled-back creation leaves a gap, never a duplicate.

When a ticket changes key (a move to another project, L05-US02), its former key goes to `issue_key_alias`: `id_of_key` finds a ticket by its key or a former one, and `GET /issues/{idOrKey}` answers for both.

`external_ref` holds identifiers of the ticket in another tool, for the imports after the MVP.

## Validation at creation

`POST /api/v1/projects/{key}/issues` checks, and reports every problem at once as field errors (400 `validation_failed`):

1. The project is visible (404 otherwise), the member has `create_ticket` on it (tenant admins always), it is not archived (409 `node.archived`).
2. The type exists in the published configuration; a sub-task has a parent; the parent is in the same project and its type is allowed (`parentId`: `required`, `not_found`, `not_allowed`, `other_project`).
3. Custom fields follow the rules of the type at creation (`domain::fields::mode`, step `Creation`): `fields.{id}`: `not_found`, `not_applicable`, `required`, or the value code of the field (`wrong_type`, `too_long`, `unknown_value`…; `unknown_member`, `unknown_group`, `unknown_issue` for users, groups and tickets that do not exist).
4. The description is a valid document (`description`: `invalid`, `too_long`, `too_deep`); the assignee is an active member (`assigneeId`: `unknown_member`); the due date is not before the start date.

The ticket gets the initial status of its type's workflow. The creation is idempotent with an `Idempotency-Key` header.

`GET /projects/{key}/issue-form?typeId=` returns what the creation dialog shows: the types, and the fields of the type's creation screen resolved with its rules (`domain::screens::resolve`), hidden ones left out, required ones marked.

## Rich text

The description is a Tiptap document. The server never stores HTML it received:

1. `infra::rich_text::parse` reads the JSON into `domain::rich_text::Doc`, refusing any node or mark outside the allow-list (paragraphs, headings 1 to 4, lists, quotes, code blocks, rules; bold, italic, strike, code, links). Links are limited to `http`, `https` and `mailto`.
2. `Doc::validate` limits the text to 100,000 characters and the nesting to 10 levels.
3. The document is stored as it is written back (`to_json`), so what is stored is exactly what is rendered.
4. `Doc::to_html` renders escaped HTML, links with `rel="noopener noreferrer nofollow"`; the API returns it as `descriptionHtml`.

Comments (L05-US04) reuse the same pipeline.

## The event outbox

Every change writes its event with `infra::outbox::record` **in its own transaction** (`issue.created` today). Real time, notifications and webhooks will read this one stream (L05-US06, US07), so it never misses nor invents a change.

`outbox_event` is partitioned by month on `created_at`:

- `app_ensure_outbox_partitions(months_ahead)` creates the missing partitions of the current month and of the next ones, each secured like a business table (`app_secure_partition`: row-level security forced, `tenant_isolation` policy). It is `SECURITY DEFINER` so the application role can run it.
- The migration creates three months ahead; the worker checks again at start and every 12 hours (`keep_outbox_partitions`). A default partition takes the events if a month is ever missing.
- Old partitions will be dropped once their events are processed (L05-US06).

## Adding a ticket endpoint

Follow [Adding an endpoint](new-endpoint.md), and:

- Load the ticket with its project through `visible_issue`: a ticket of a project the member does not see answers 404, like one of another tenant.
- Read the configuration through `Content::of(project)` (published version only).
- Write the change's event to the outbox in the same transaction.

## In the application

| What | Where |
| --- | --- |
| Rich text editor (form control, Tiptap) | `web/projects/ui/src/lib/components/rich-text-editor.ts`, imported from `ui/rich-text` |
| Rich text shown (`descriptionHtml`) | `ui-rich-text` in `ui` |
| Priority label | `ui-priority` in `ui` |
| Creation dialog, opened by `createIssueFlow` | `web/projects/app/src/app/issues/issue-dialog.ts`, `issue-flow.ts` |
| A custom field by kind, people and groups | `field-input.ts`, `people-picker.ts`, values in `issue-fields.ts` |
| Ticket page, "My tickets" | `issue-page.ts` (`/issues/:key`), `my-tickets-page.ts` (`/tickets`) |

- **Keep Tiptap out of the initial bundle.** The editor is not exported from `ui` but from its own entry point, `ui/rich-text`: a barrel export would pull Tiptap and ProseMirror (about 430 kB) into every page. The creation dialog is loaded on first use by `createIssueFlow`; a new screen that edits rich text (comments, L05-US04) imports `ui/rich-text` from a lazily loaded component too.
- The editor only offers what the server accepts (no underline, headings 1 to 4, links to `http`, `https` and `mailto`), so a document is never refused for its form.
- The dialog reads the form for the type chosen (`GET …/issue-form?typeId=`) and keeps a value when the next type has the same field. Errors of the API go to their control (`fields.{id}` included) through `placeApiErrors`.
