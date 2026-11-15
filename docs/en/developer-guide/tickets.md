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
| Rules | `backend/crates/domain/src/audit.rs` | History: origins, actions, fields, `diff` |
| Tables | `backend/migrations/*_audit_log.sql` | `audit_log`, `app_member_name` |
| SQL | `backend/crates/infra/src/audit.rs` | `record`, `of_issue`, `names`, `ensure_partitions` |
| Job | `backend/crates/worker/src/lib.rs` | `keep_partitions` (outbox and history), every 12 hours |
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

## Editing (L05-US02)

`PATCH /api/v1/issues/{idOrKey}` (`api::issue_changes`) takes only what changes; `null` clears a nullable value (`present` tells an absent field from `null`). It requires `If-Match` (428 without it): the ticket row is locked (`infra::issues::lock`, `FOR UPDATE`) and its version compared with the one read (412 when stale), so two edits never overwrite each other silently.

- The rules are those of the type **at the ticket's status** (`domain::issues::validate_changes`, step `Status`): a hidden field takes no value, a required one is not cleared, a deactivated list value is accepted only if the ticket already holds it. Only the fields given are checked: a rule added later does not block other edits.
- Type, status and parent are not edited here: status changes are transitions (L05-US09, #515), sub-tasks are L05-US03.
- `issue.updated` goes to the outbox with each changed field, `{field, old, new}` (`summary`, `priority`, `assigneeId`, `fields.{id}`…): the history (L05-US06) records the same changes in `audit_log`. Nothing changed: nothing written, same version.
- `GET …/edit-form` returns the fields of the type's **view** screen at the ticket's status (`screen_fields`), with the deactivated values the ticket holds still offered.
- Field values carry `references` (id and name of the members, groups or tickets they name), so the client can show and edit them without another lookup.

## Moving (L05-US02)

`POST /api/v1/issues/{idOrKey}/move` (with `If-Match`) moves a ticket to another project; `POST …/move-plan` (same body, changes nothing) tells the dialog what it takes.

- `edit` on the ticket's project and `create_ticket` on the target; both active.
- A sub-task does not move alone (409 `issue.subtask_moves_with_parent`): its parent moves, **with its sub-tasks**. The ticket moved leaves the one it sat under (an epic stays in its project).
- Each ticket gets the next key of the target project (`infra::issues::move_to`, same counter as creation); its former key goes to `issue_key_alias`, so every former key keeps resolving.
- Types and statuses are stable identities: those the target configuration has keep their identity (same configuration: nothing to map). Others are mapped (`typeMappings`, `statusMappings`), checked by `domain::issues::plan_move`: the ticket lands on a type that is no sub-task, its sub-tasks on sub-task types allowed under it, each status in the workflow of its new type. The plan suggests a mapping (same key, else same category) and reports what the move would refuse.
- Values of fields the target lacks, that do not apply to the new type, or no longer fit, are dropped (`kept_values`); the plan lists them, and `issue.moved` (one per ticket) carries `droppedFieldIds` with the former and new keys and projects.

## Links, sub-tasks and parents (L05-US03)

`issue_link` stores a link from a source ticket to a target, across projects of the tenant (`api::issue_links`, `infra::links`, `domain::links`).

- **Four kinds, each read two ways**: blocks / blocked by, depends on / required by, duplicates / duplicated by, relates to (the same both ways). The API speaks in relations as read from the ticket at hand (`relation: "blocked_by"`); `Link::new` turns one into the stored source, target and type. The Gantt dependencies of L12 are another thing.
- **Each link once**: never a ticket to itself (`issue_link_not_self`), each kind once per direction (`issue_link_unique`), "relates to" once per pair whatever its direction (`issue_link_relates_once` on `LEAST`/`GREATEST`). `domain::links::validate_link` checks the same before writing, for a clear error (`issue` `self`, 409 `issue_link.exists`).
- **Rights and sight**: adding or removing a link takes `edit` on **both** tickets' projects. A link shows only to those who see both tickets: `link_views` leaves out the others, and a target out of sight is `not_found`.
- `GET /issues/{idOrKey}?expand=parent,children,links` returns the parent, the sub-tasks (`children`) and the links, the other ticket each time with its project, type and status (`RelatedView`, read in its own project's configuration).
- **Parents**: `PUT /issues/{idOrKey}/parent` (with `If-Match`) changes the parent within the project: a type the ticket's type may sit under, never the ticket itself or one below it (`infra::issues::is_below` walks up the parents), and a sub-task always keeps one. It writes `issue.updated` with `{field: "parentId", old, new}`.
- **Finding a ticket**: `GET /issues?search=&projectKey=` returns at most ten tickets of projects the member sees: its key or a former key first, then summaries holding the text, most recently updated first. It is a stopgap until the query language of L06.

## Status changes (L05-US09)

A ticket changes status only through a transition of its type's workflow (`api::issue_transitions`); `PATCH` never changes it.

- `GET /issues/{idOrKey}/transitions` lists the transitions that leave the ticket's status (or any status, `fromId` null) and whose condition the member meets: one of its roles on the project (`GrantSource::roles_of`, inheritance included) or one of its groups. Tenant admins meet every condition. Each comes with the fields it takes, resolved at the **target** status: those of its transition screen, its validators (marked required), then those required at the target status. With none, the client runs it at once; otherwise it shows them first, so that a missing value is named next to its field.
- `POST /issues/{idOrKey}/transitions` (`edit` and `If-Match`) runs one, in this order:
  1. The values given: only fields the transition takes (`not_applicable` otherwise), checked with the rules of the target status (`validate_changes`).
  2. `domain::transitions::evaluate`: the condition (403 if not met), the validators (`fields.{id}` `required`), then the post-functions in order.
  3. `domain::issues::missing_required`: the fields required at the target status must hold a value once the screen and post-functions have applied.
  4. The status, assignee and values are written (`infra::issues::transition`), and `issue.transitioned` goes to the outbox with `fromStatusId`, `toStatusId`, the changes (`{field, old, new}`) and the comments of "add a comment" post-functions.
- "Add a comment" post-functions write internal comments (`source = 'transition'`) in the same transaction, the member who ran the transition as their author (`issue_comments::write`).
- The board card transition (L06, #185) uses this endpoint.

## Comments (L05-US04)

`comment` (`api::issue_comments`, `infra::comments`, `domain::comments`) holds the comments of a ticket: the document (Tiptap JSON), its HTML rendered when written, the accounts it mentions, internal or public.

- **Rights**: writing takes `comment` on the project (tenant admins always). The author edits and deletes their own while they still hold `comment`; those who administer the project (`administer_node`, tenant admins) delete anyone's, for moderation, never edit them. Each comment tells the reader `canEdit` and `canDelete`.
- **Deleted**: a placeholder stays (author, date), its content goes; the database keeps that consistent (`comment_content_or_deleted`).
- **Visibility**: internal by default. Members see both; the portal (L13) must only ever read public ones (`infra::comments::of_issue(…, public_only: true)`).
- **Mentions and references**: `mention` nodes name accounts, which must be active members (`content` `unknown_member`). Ticket keys typed in text (`PLAT-120`, whole words, not in code nor in a link) or picked (`issueRef` node) become links to `/issues/{key}` (`domain::rich_text::key_spans`).
- **Events**: `comment.created` (mentions, ticket references, text), `comment.updated` (text before and after, `addedMentions`: only those newly mentioned are to be notified), `comment.deleted` (the text it had). Notifications (L05-US07) read them; the history (L05-US06) records each with its content.

## History (L05-US06)

`audit_log` (`infra::audit`, `api::issue_history`) records who changed what on a ticket, when and from where, **in the transaction of the write**: a refused or failed write records nothing.

- **Entries**: `updated`, one per changed field, with its old and new values (`field`: `status`, `summary`, `dueDate`, `fields.{id}`…); `created`; `moved` (old and new keys); `link_added` / `link_removed` (on both tickets, each with its own reading: `blocks` on one, `blocked_by` on the other); `comment_created` / `comment_updated` / `comment_deleted` (the comment's id and content). Entries of one write share `change_id`.
- **Diff**: a write takes a snapshot of the ticket before and after (`issue_history::snapshot`) and records what `domain::audit::diff` finds; a field absent or null is the same. Types and statuses are stored with their labels of the time, since the configuration may change or the ticket move; other values as the ticket holds them, resolved when read (names, keys, list labels of the current configuration).
- **Origin**: `TenantTx::author()` gives the actor and the origin: `interface` for a session today; API tokens (L15) and the MCP server (L16) will set `api` and `mcp`; jobs record `system` with no actor.
- **Every write path calls it**: creation, edit, move (each ticket moved, sub-tasks included), transition, parent, links, comments. A new write path must call `issue_history::record` too.
- **Reading**: `GET /api/v1/issues/{idOrKey}/activity`, newest first, keyset pagination on `(created_at, id)` (ids of one change are generated in reverse so that its fields read in order). Links to tickets the reader does not see are left out; the content of a deleted comment shows to moderators only.
- **Names and erasure**: names come from `app_member_name(account)`: "Deleted user #N" once erased in the tenant, null for an account no longer a member. Tickets (reporter, assignee) and comments use it too. The erasure job's `comments` and `history` steps empty the member's comments and strip their content from the history when `purgeComments` is asked; their other entries stay, under the pseudonym.
- **Partitions**: by month on `created_at`, like the outbox (`app_ensure_audit_partitions`, a default partition), with the index `(tenant_id, entity_id, created_at)`.

## Rich text

The description is a Tiptap document. The server never stores HTML it received:

1. `infra::rich_text::parse` reads the JSON into `domain::rich_text::Doc`, refusing any node or mark outside the allow-list (paragraphs, headings 1 to 4, lists, quotes, code blocks, rules; bold, italic, strike, code, links). Links are limited to `http`, `https` and `mailto`.
2. `Doc::validate` limits the text to 100,000 characters and the nesting to 10 levels.
3. The document is stored as it is written back (`to_json`), so what is stored is exactly what is rendered.
4. `Doc::to_html` renders escaped HTML, links with `rel="noopener noreferrer nofollow"`; the API returns it as `descriptionHtml`.

Comments (L05-US04) reuse the same pipeline, with two more inline nodes: `mention` (`attrs.id`: an account) rendered as `<span class="mention">@Name</span>`, and `issueRef` (`attrs.id`: a ticket key) rendered as a link. Ticket keys typed as text are linked too.

## The event outbox

Every change writes its event with `infra::outbox::record` **in its own transaction** (`issue.created` today). Real time, notifications and webhooks will read this one stream (L05-US07, US08), so it never misses nor invents a change.

`outbox_event` is partitioned by month on `created_at`:

- `app_ensure_outbox_partitions(months_ahead)` creates the missing partitions of the current month and of the next ones, each secured like a business table (`app_secure_partition`: row-level security forced, `tenant_isolation` policy). It is `SECURITY DEFINER` so the application role can run it.
- The migration creates three months ahead; the worker checks again at start and every 12 hours (`keep_partitions`). A default partition takes the events if a month is ever missing.
- Old partitions will be dropped once their events are processed.

## Adding a ticket endpoint

Follow [Adding an endpoint](new-endpoint.md), and:

- Load the ticket with its project through `visible_issue`: a ticket of a project the member does not see answers 404, like one of another tenant.
- Read the configuration through `Content::of(project)` (published version only).
- Write the change's event to the outbox and its history entries (`issue_history::record`) in the same transaction.

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
