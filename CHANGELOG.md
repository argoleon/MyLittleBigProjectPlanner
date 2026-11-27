# Changelog

All notable changes of My Little Big Project Planner. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses [Semantic Versioning](https://semver.org/). Each release finishes a work package (lot) of the MVP (specification §20).

## [Unreleased]

### Added

- **Scrum backlog and sprint planning** (#261)
  - A board is now **Kanban** or **Scrum**, chosen at creation. A Scrum board opens on its backlog: its open sprints, each with its tickets and total in the board's estimation (points or hours), then the backlog of the tickets of its filter in no open sprint, finished ones left out. Sub-tasks follow their parent and show as a count on its row.
  - A ticket is in one open sprint at most. Another board whose filter holds it shows that sprint read-only, with the board planning it.
  - Members with **Manage sprints** on the board's node create, change and delete (while not started) sprints: a name ("PLAT Sprint 4" when left empty), a goal and dates. They drag tickets between the backlog and the sprints, or reorder them; each ticket's menu does the same with the keyboard (to a sprint or the backlog, up, down, top, bottom). Others see the changes as they happen.
  - The rank is kept per board, as lexicographic ranks (a move writes one row). **Kanban** cards follow it too: planners reorder the cards of a column, dragged or through the card's menu.
  - Every stay of a ticket in a sprint is kept (added, removed), for the burndown and the velocity to come.
  - The query language runs `sprint = "PLAT Sprint 4"`, `sprint IN openSprints()`, `sprint IS EMPTY`; a sub-task follows its parent. `fixVersion` still answers `query.unsupported` until the versions.
  - `GET /api/v1/boards/{id}/backlog`, `POST /api/v1/boards/{id}/sprints`, `/api/v1/sprints/{id}` (read, change, delete), `POST /api/v1/boards/{id}/backlog/moves`, `POST /api/v1/boards/{id}/rank`; boards carry their `kind` and `canPlan`; problems `board.not_scrum`, `sprint.closed`, `sprint.not_future`, `sprint.foreign`, `sprint.subtask`.

## [0.8.0] - 2026-11-27

Work package **L08 Assignments, workload**: assignments of resources to tickets by segments, the daily load, the overload check, absence conflicts, the workload heatmap, load against capacity by portfolio, program, project and team, replacing a generic resource with a person, provisional booking and the availability search.

### Added

- **Assignments** (#220)
  - A resource is assigned to a ticket through one or more successive segments, each with its period and mode: a percentage of each day's actual capacity (50% of a 3.5 h Friday is 1 h 45, a public holiday 0), hours per working day, or a total effort spread evenly over the working days. One assignment per resource and ticket; segments never overlap (`assignment.segments_overlap`) and stay within the ticket's dates when it has them (`assignment.outside_issue_dates`); a resource is assigned once per ticket (`assignment.resource_taken`), never a team.
  - The ticket page shows an **Assignments** card: one block per resource with its segments, their load and the ticket's planned total; members with **Assign resources** on the project add, change and remove assignments in the **Assign a resource** dialog. Segments left outside dates changed since are flagged.
  - Every change is recorded in the event outbox (`assignment.changed`, with the days concerned) for the daily load of L08.
  - `/api/v1/issues/{idOrKey}/assignments` (list, create) and `/api/v1/assignments/{id}` (read, change, delete).
- **Daily load** (#225)
  - The load of each resource is kept day by day: one row per assignment and day, recomputed in the same transaction when an assignment is written, and by the worker within seconds when a capacity period, a calendar exception or holiday set, an absence or the default capacity changes (job `workload.recompute`, for the resources and days concerned). Deleting an assignment, or its ticket, removes its rows. The overload check and the heatmap read it.
  - The table is partitioned by month; the worker creates the partitions of the year before and the two years after, and a write creates the ones it needs.
- **Overload check** (#229)
  - Every assignment write compares the resulting load of each day with the resource's capacity (public holidays, closures and absences deducted) plus the organization's tolerance, under a lock per resource: two project managers assigning the same person at the same time never exceed their capacity together. A day is in conflict when the assignment adds load to it.
  - Blocking mode (the default) refuses the assignment with `workload.overload`, listing each day with the hours planned and available. Members with **Force overload** on the project (organization admins always) force it with `force: true`; the write is traced in the ticket's history (**forced an overload of…**) and in **Administration › Forced overloads**. Warning mode saves it; either way the ticket's **Assignments** card flags the resources taking part in an overload, with their days.
  - The **Assign a resource** dialog previews, for each day, the existing load, this assignment's and the total, overloaded days marked in text and color, with the blocking message and the **Force overload** button for those allowed. It now also has the **Provisional assignment** switch.
  - A new workload setting tells whether provisional assignments count in the check (they do by default).
  - `POST /api/v1/assignments/preview`, `GET /api/v1/tenant/forced-overloads`; assignments carry their `overloads`; the workload settings gain `overloadCountsProvisional`.
- **Absence conflicts** (#234)
  - Hours planned in hours per day or total effort that an absence leaves no room for are a conflict, per assignment and day: all the hours of a day off, only those beyond the capacity left on a half day; percentages never conflict. Conflicts follow the daily load: opened when an absence or a segment puts hours there, resolved by themselves when no longer, reopened if they come back.
  - A new notification event, **Absence conflict**, in the rules of every project configuration: by default the ticket's assignee and the Project manager role ("7 h to reschedule on PROJ-42 (absence of Nathan Roux)"), in the application and by email unless the member chooses otherwise. Only days still to come are told.
  - The resource page shows the hours in conflict on each absence; the ticket's **Assignments** card lists the days and hours to reschedule.
  - `GET /api/v1/workload/conflicts` (by resource, project and status; the tickets the member sees); assignments carry their open `conflicts`, absences their `conflictMinutes`; notifications gain `absence_conflict` and its `conflict`.
- **Assignments of a resource** (#239)
  - The resource page has two tabs: **Capacity and calendar**, and **Assignments**, which lists its assignments by project with their tickets, periods, values and load, provisional ones and those in overload or absence conflict marked; ongoing and upcoming ones first, past ones on demand. Members who assign resources on a project change an assignment there through the **Assign a resource** dialog (with the same overload check) or remove it after confirmation.
  - Time on tickets the member does not see, on a confidential project or one not granted to them, shows as **Busy**: its periods and hours, without ticket nor project.
  - `GET /api/v1/resources/{id}/assignments` (period, pages, `expand=issue`).
- **Workload heatmap** (#242)
  - The **Workload** menu opens on a heatmap of the active resources by day, week or month, each cell colored by occupation (under 40%, 40 to 70%, 70 to 95%, 95 to 100%, overload beyond the capacity plus the tolerance), absences and non-working periods shown apart, provisional load and load on tickets the member does not see hatched. Filters by team, and by portfolio, program or project (the resources with load there, their whole load shown); provisional load in or out; a team average row.
  - A cell, clicked or opened with the keyboard (arrows, Home/End, Page Up/Down, Enter, the focused cell said by screen readers), shows in a side panel the capacity, the planned hours and the gap, the overloads forced there, and the tickets behind the load, those the member does not see as **Busy (confidential)**. A **Table** view gives the same data in an accessible table.
  - The heatmap is drawn on a canvas, only what is in view: 200 resources over 3 months read in about 150 ms. Weekly and monthly sums are precomputed for a year back and two years ahead, kept up to date by the worker within seconds of a change of load or capacity.
  - `GET /api/v1/workload/heatmap`, `GET /api/v1/workload/cell`.
- **Load against capacity by node and team** (#248)
  - Portfolio, program and project pages, and team pages, show a **Load and capacity** card: per month (or week), the hours planned against the capacity, with the share used and overloaded periods said in words.
  - A node sums its distinct projects, a project reached twice once, each with the weighting of its link; confidential projects count without detail. A project's capacity is its resources' capacity in proportion to their load there, so that a person shared between two programs is never counted twice. A team sums its active members.
  - The planned load and capacity of the hierarchy's indicators are now fed by the assignments, and kept up to date within seconds.
  - `GET /api/v1/nodes/{id}/workload`, `GET /api/v1/teams/{id}/workload`.
- **Replacing a generic resource with a person** (#252)
  - Organization admins and resource managers replace a generic resource with a person from its page (**Replace with a person**). Its assignments move to the person from today on: days before today stay on the generic resource, a segment across today is cut (a total effort shared in proportion to its working days), and segments join the person's assignment of the same ticket when there is one. Tickets of archived projects stay.
  - Before replacing, the dialog previews the assignments that move, the tickets where the person's segments and the moved ones would take a same day (which refuse the replacement, `assignment.segments_overlap`) and the person's days it would overload, checked as for an assignment: in blocking mode, forcing needs **Force overload** on every project concerned, and is traced.
  - Each ticket's history records it (**replaced … with …**). The generic resource is deactivated afterwards, unless unchecked.
  - `POST /api/v1/resources/{id}/replacement`, `POST /api/v1/resources/{id}/replacement/preview`.
- **Confirming a provisional assignment** (#255)
  - A provisional assignment (hatched in the heatmap, counted in the overload check or not by the organization's setting) gets a **Confirm** button on the ticket's Assignments card and on the resource's Assignments tab: it becomes firm and is checked for overload as a firm assignment. In blocking mode, those with **Force overload** are asked whether to force it (traced); the others are told the days.
  - `POST /api/v1/assignments/{id}/confirm` (with `If-Match`).
- **Availability search** (#258)
  - A new **Availability** tab of the Workload screen finds who has a number of free days (working days entirely free, not necessarily in a row) or free hours over a period of up to 92 days, optionally in a team, most free time first, with their free time, free days and planned load against capacity. Free time is the capacity (absences, public holidays and closures deducted) less the load, day by day; the overload tolerance is not counted as free time. Provisional load counts unless left out. Load on tickets the reader does not see reduces it without naming them.
  - Each resource found opens in the heatmap on its days, with its details; the heatmap now opens from a link on a scale, period, team and resource.
  - `GET /api/v1/workload/availability` (also for the MCP tool `find_availability`, L16).

### Upgrading from 0.7.0

- **Migrations**: they add assignments, the daily load (partitioned by month), the absence conflicts and the precomputed load sums. No existing data is converted; the worker computes the load sums and keeps the partitions, so it must run.
- **Overload check**: it applies from this version, in **blocking** mode with no tolerance unless an organization admin changed them in 0.7.0; provisional assignments count by default. Organization admins review the mode, the tolerance and the provisional setting in Administration, Workload, and give **Force overload** to the roles that may go beyond capacity.
- **Notifications**: every project configuration gets the rule of the new **Absence conflict** event (the ticket's assignee and the Project manager role); adjust it in the configuration's notification rules if needed.
- **Indicators**: the planned load and capacity of portfolios, programs and projects are now fed by the assignments; they fill in within seconds of the first assignments.

## [0.7.0] - 2026-11-24

Work package **L07 Resources and capacity**: resources (persons and generic roles) and teams, weekly capacity historized by effective date, calendars with French public holidays and sites, absences by whole or half days, and the capacity of each day, read for one resource or for hundreds at once.

### Added

- **Resources** (#194)
  - Persons, linked to a member's account or not (a subcontractor who never signs in), and generic resources (an unnamed role such as "Rust developer"), with an optional job title. A linked person shows the member's name; a member is linked to one resource at most (`resource.account_taken`), and only an internal member of the organization. Resources are deactivated, never deleted.
  - A member who leaves the organization leaves their resource behind, unlinked and inactive, under the name it showed; erasing the member (in the organization or everywhere) replaces that name by their pseudonym.
  - A new organization role, **Resource manager**, manages resources as the organization admin does; every internal member reads them.
  - The **Workload** menu opens on the resources (filters by kind and status, search, pages), with a dialog to create and edit them and a page per resource.
  - `/api/v1/resources` (list, create, read, change) and `GET /api/v1/resources/linkable-accounts`. `TenantRole` gains `resource_manager`.
- **Teams** (#198)
  - Teams group resources for views and filters; a resource may belong to several teams, and leaving one keeps its assignments. A team has a translatable name, unique in the organization, and an optional description. Organization admins and resource managers manage them; every internal member reads them.
  - The **Workload** screen has two tabs, Resources and Teams. The resource list filters by team, a resource's page lists its teams, and each team has a page where its resources are added (found by name) and removed.
  - `/api/v1/teams` (list, create, read, change, delete) and `PUT`/`DELETE /api/v1/teams/{id}/members/{resourceId}`; `GET /api/v1/resources?team=`. A resource now tells its `teams`.
- **Weekly capacity** (#202)
  - A resource's hours per weekday, historized by effective date: "Part time 80% from 1 March". Before its first period, or without any, the organization's default applies as it stands. One period per resource and date (`capacity_pattern.date_taken`); durations are kept in minutes.
  - The resource page shows a **Weekly capacity** card: each period with its hours per day ("7 h", "3 h 30") and weekly total, its label, and "planned" when it starts later. Hours are typed as 7, 3.5, 3,5 or 3:30. Organization admins and resource managers add, change and delete periods.
  - A **Workload** tab in Administration lets organization admins change the default capacity, the overload tolerance and the overload mode (used from L08).
  - Every change is recorded in the event outbox (`resource.capacity_changed` with the days concerned, `tenant.capacity_changed`) for the load computation of L08.
  - `/api/v1/resources/{id}/capacity-patterns` (list, create, change, delete) and `GET`/`PUT /api/v1/tenant/workload-settings`.
- **Calendars and public holidays** (#207)
  - Every organization has its calendar, with the French public holidays computed for any year; sites ("Paris", "Strasbourg") have theirs, with the organization's holidays, the Alsace-Moselle ones or none. A resource belongs to a site, or to none.
  - Dated exceptions at three levels — the organization, a site, a resource — the closest deciding: a holiday added, a closure, or a day worked that cancels a holiday. Working weekdays stay those of the capacity.
  - A **Calendars** tab in Workload lists the calendars; each has a page with its holidays and exceptions by year. The resource page shows its site and a month grid (holidays, closures, days not worked, its own exceptions), navigable with the keyboard; managers add an exception by choosing a day.
  - Every change is recorded in the event outbox (`calendar.changed`, `resource.capacity_changed`) for the load computation of L08.
  - `/api/v1/calendars` (list, create a site, read, change, delete), `GET /api/v1/calendars/{id}/year`, `/api/v1/calendars/{id}/exceptions` and `/api/v1/resources/{id}/exceptions` (create, change, delete), `GET /api/v1/resources/{id}/days` (each day's status and capacity). A resource takes a `calendarId` and tells its `site`.
- **Absences** (#212)
  - Leave, training or other time off, from a first to a last day, possibly starting or ending at noon; a half day removes half of the day's capacity, a whole day all of it. Absences of a resource never overlap (`absence.overlap`).
  - The person linked to a resource declares their own, from the resource page or **My absences** in My account; organization admins and resource managers declare anyone's and are recorded as the declarer. Everyone sees the dates and kind; the comment is for the person and the managers, and goes when the person is erased.
  - The resource page shows an **Absences** card and the absences on its month grid; `GET /api/v1/resources/{id}/days` tells each day's absence and remaining capacity.
  - Every change is recorded in the event outbox (`resource.absence_changed`) for the load computation of L08.
  - `/api/v1/resources/{id}/absences` (list, declare, change, delete) and `GET /api/v1/resources/mine`.
- **Capacity of each day** (#216)
  - A single computation of a day's capacity — the weekly pattern, less the public holidays and closures of the resource's calendars, less its absences — shared by every screen and, from L08, by the load. A morning off and an afternoon off on the same day take the whole day.
  - The resource page shows the hours left on a half day off, the month's total under its grid, and a **Capacity over the next 4 weeks** card, week by week.
  - `GET /api/v1/capacity`: the minutes of each day and the days off of many resources at once — named ones, a team's, or every active resource —, 200 per page, up to 366 days; 200 resources over 3 months are read in under 200 ms.

### Fixed

- The API and the worker listen for SIGTERM and Ctrl+C from the start of their startup: a stop requested while they start (migrations, connections, storage check) is now a graceful shutdown, no longer an abrupt kill. This also fixes the worker test `stops_cleanly_on_sigterm`, which failed now and then under load.

### Security

- **cookie 0.18.3 and url 2.5.9** (backend dependencies): fix RUSTSEC-2026-0594 and RUSTSEC-2026-0592, published after 0.6.0.

### Upgrading from 0.6.0

- **Resources**: the migrations add resources, teams, capacity periods, calendars and absences; no existing data is converted, and the workload screens start empty. Organization admins create the resources (linking members' accounts where relevant) and the sites; to let someone else manage them, give them the new **Resource manager** role in Administration, Members.
- **Calendars**: every existing organization gets its calendar with the French public holidays. Organizations in Alsace-Moselle, or with closures or bridge days, adjust it in Workload, Calendars.
- **Default capacity**: resources without a capacity period work the organization's default week (7 h × 5 days unless changed). Organization admins review it in Administration, Workload, with the overload tolerance and mode that L08 will use.


## [0.6.0] - 2026-11-19

Work package **L06 Search and Kanban**: a query language with a visual builder and suggestions, full-text search with typo correction, saved and shared filters, Kanban boards, CSV and Excel exports of list views, and bulk actions on a search result.

### Added

- **Search with the query language** (#168)
  - A JQL-like language (specification §6): comparisons, `IN`, `IS EMPTY`, `~` on text, `AND` / `OR` / `NOT` and parentheses, durations (`created > -7d`), `currentUser()`, `now()`, `startOf…()` and `endOf…()` (day, week, month, year), `descendantsOf("PORTF-HR")`, and `ORDER BY`. Fields and values are stable keys; dates are read in the member's time zone.
  - `POST /api/v1/search` with `{query, fields, expand, cursor, limit}`: the tickets of the projects the member sees, the total on the first page, a cursor for the next ones. A query that cannot run answers `query.invalid` with the reason, the character position and what was expected.
  - The Search screen (mockup 15): the query runs on Enter and stays in the address (`/search?q=…`); results in a virtualized table with the total, the next page loaded while scrolling, columns chosen from a menu (kept in the browser), and sorting a column sorts the query. A refused query is explained at its position, the part concerned selected in the field.
  - `sprint` and `fixVersion` are understood but answer `unsupported` until sprints and versions come (L09); `text ~` matches the summary and the description until full text comes (#177).
- **Writing queries without knowing the syntax** (#173)
  - A visual builder: projects, types, statuses, assignees ("Me" included), priorities, due date and creation date (presets such as "Overdue" or "Last 7 days", or a custom range), text; it writes the query, and reads back a query made of such criteria. Any other query stays in the editor, with a word why. The screen opens in the last mode used (the builder the first time).
  - The query editor suggests what may come at the cursor: fields (custom ones included), the operators of a field, keywords, functions, and values with their labels (statuses, types, priorities, members, the projects and nodes the member sees). Arrows, Enter and Escape, or Ctrl+Space to ask.
  - `POST /api/v1/search/suggestions` (`{query, cursor}`) and `POST /api/v1/search/parse` (the query as a tree).
- **Full-text search** (#177)
  - `text ~ "outage"` searches the title, the description and the comments, with French and English stemming, accents ignored. Without `ORDER BY`, the most relevant tickets come first, those holding the words in their own text before those holding them in a comment only.
  - Typos are corrected against the words already written in the organization, by edit distance (swapping two letters counts as one edit): none under 4 letters, one edit up to 7, two beyond. Corrected matches rank below exact ones.
- **Saved filters** (#180)
  - A query is saved under a name and an icon, private or shared with groups or the whole organization; the panel of the Search screen lists the member's own and those shared with them. Its owner alone changes it; an organization admin may delete a shared one. A member who leaves takes their private filters with them; shared ones stay.
  - `/api/v1/saved-filters` (list, create, read, change, delete) and `GET /api/v1/saved-filters/share-groups`.
- **Kanban boards** (#183)
  - A board belongs to a portfolio, program or project, and shows the tickets of its filter in columns mapped to statuses (by key, across configurations). A column may cap its work in progress: reached, then highlighted once exceeded. Cards are grouped in swimlanes by assignee or by epic, narrowed by the board's quick filters, and show their type, blockers, epic, estimate (points or hours, per board), priority and assignee.
  - Dragging a card to another column runs the transition of its workflow that leads there, with its conditions, validators and post-functions; a transition with fields opens its form first, several leading there are offered to choose from. Where none leads, or its condition is not met, the card goes back and the reason shows. Each card's menu moves it with the keyboard alone. Moves by others show as they happen.
  - Whoever sees the node sees its boards, each seeing only the tickets they may see; the new permission **Manage boards** on the node creates, configures and deletes them. A new board starts with a column per status category and the quick filters "My tickets" and "Bugs". The boards of a node are a link away from its page.
  - `/api/v1/boards` (list, create, read, change, delete), `GET /api/v1/boards/statuses`, `GET /api/v1/boards/{id}/cards` and `POST /api/v1/boards/{id}/moves`.

- **CSV export of list views** (#188)
  - An **Export** menu on the Search screen and on My tickets: the tickets of the query as the member sees them, with the columns displayed, headers in their language. UTF-8 with a byte order mark, ISO dates and times, dot decimals; a text starting like a formula (`=`, `+`, `-`, `@`) is neutralized.
  - Up to 1,000 tickets, the file downloads at once. Beyond, the worker writes it and the member is notified when it is ready: a new **Export ready** notification, in the bell and by email as the member chooses (in the application only by default; emailed at once, never in the digest). Up to 100,000 tickets; the file is kept 24 hours, for its requester only.
  - `POST /api/v1/exports` (`201` ready, `202` queued, `422` `export.too_large`), `GET /api/v1/exports`, `GET /api/v1/exports/{id}` and `GET /api/v1/exports/{id}/file` (a short-lived link to the storage). Notifications may now be about something else than a ticket: `MemberNotification` gains `export`, and its `issueId`, `issueKey` and `issueSummary` are `null` for an export. Member notifications and preferences use the event type `MemberNotificationEvent` (the ticket events and `export_ready`); configuration rules keep `NotificationEvent`.
- **Excel export of list views** (#191)
  - The Export menu offers an Excel file (`.xlsx`) beside CSV, with the same columns, headers and limits, in the background beyond 1,000 tickets. Cells are typed: numbers as numbers, due and start dates as dates, creation and change times as date-times in UTC (said in their header); texts stay texts, never run as formulas. `POST /api/v1/exports` takes `"format": "xlsx"`.
- **Bulk actions on a search result** (#528)
  - The Search screen gains a selection column: tick tickets, or every loaded one, then all the results of the query. A bar shows the count and the actions **Modify**, **Change status** and **Move**.
  - Modify sets the priority, assignee, start and due dates, and the custom fields shared by every selected ticket. Change status offers only the statuses a transition reaches, each ticket running its own workflow's transition. Move uses one type and status mapping for the batch; sub-tasks follow their parent.
  - Each ticket is changed in its own transaction, with its version check, history line and notifications. Tickets the member may not change, or that a transition or validator refuses, are left unchanged; the report lists what became of each, with the reason or the new key.
  - Up to 100 tickets, the action runs within the request and its report shows at once. Beyond, the worker runs it (at most 10,000 tickets) and the member is notified when it is done: a new **Bulk action done** notification, in the bell and by email as the member chooses (in the application only by default; emailed at once, never in the digest), leading to the report's page. Reports are kept 7 days, for their requester only.
  - `POST /api/v1/bulk-actions/options`, `POST /api/v1/bulk-actions/move-plan`, `POST /api/v1/bulk-actions` (`200` done, `202` running, `422` `bulk.too_many`) and `GET /api/v1/bulk-actions/{id}`. `MemberNotification` gains `bulk`; `MemberNotificationEvent` gains `bulk_done`.

### Changed

- **Every node has a key** (#168): portfolios and programs now carry one too (2 to 20 capitals, digits and dashes, such as `PORTF-HR`), unique in the organization together with project keys, so that queries can name them (`descendantsOf("PORTF-HR")`). It is entered in the creation dialog and may be changed in the General tab; a project's key still never changes. API: `key` is required to create any node and always present in node responses; `PUT /nodes/{id}` accepts a new `key` for a portfolio or program.

### Fixed

- The owners field of a node's General tab no longer breaks the accessibility rules once an owner is chosen (the search input sat inside the chip grid).

### Upgrading from 0.5.1

- **Manage boards**: the migration gives the new permission to every role that manages sprints (the default "Project manager" and "Node admin" roles among them). Review the roles that should manage boards in Administration, Roles. Permissions cached before the upgrade expire within 5 minutes.
- **Full text**: the migration indexes the text of every existing ticket and comment, and builds each organization's word list for typo correction; on large organizations, it takes a few minutes.
- **Exports and bulk actions**: large exports and bulk actions over 100 tickets run in the worker, which must be running. Export files go to the existing object storage bucket (under `<organization id>/exports/`) and are deleted after 24 hours; bulk action reports are deleted after 7 days. Members get the two new notifications, **Export ready** and **Bulk action done**, in the application only until they choose email in their preferences.
- **Node keys**: the migration gives every existing portfolio and program a key made of its type and name (`PORTF-RESSOURCES-HUM`, `PROG-MODERNISATION-D`, numbered when taken). Administrators should review them in each node's General tab before saving filters that use them.

## [0.5.1] - 2026-11-18

Security fix of 0.5.0: the email client had known vulnerabilities, and the dependency checks no longer passed.

### Security

- **lettre 0.11.24** (#537), the SMTP client sending every email (invitations, password resets, notifications): fixes RUSTSEC-2026-0560 to 0565. These cover STARTTLS response injection by a man in the middle, SMTP command injection through bare CR or LF, a quoted local part sent to another address, panics on an `EHLO` reply or a display name holding CR or LF, and unbounded buffering of an SMTP reply.

### Fixed

- **The status menu of a ticket** no longer closes by itself right after a transition (#526). The page reads the ticket again on the real-time echo of the member's own change; meanwhile the menu turned into a plain status for an instant, which closed it when it was open. It now keeps the transitions shown until the new ones are read.
- **The up and down buttons of the configuration lists** (fields of a screen, post-functions of a transition, issue types) move the item pressed, even pressed again before the list is drawn anew (#532). They used the row's position as last drawn: two quick presses could move another item, or reorder the issue types twice from the same order.

### Changed

- **Dates and times** are handled by `time` instead of `chrono` (#531), which is no longer maintained (RUSTSEC-2026-0543). The API answers exactly as before: times in RFC 3339 (UTC), dates as `YYYY-MM-DD`, the same OpenAPI contract.
- **npm audit** (#535): a high or critical advisory with no fixed version may be accepted in the build and test tools only, until a review date (`web/audit-exceptions.json`). What ships still has no exception. Accepted until 2027-01-15: picomatch (GHSA-fc5q-39xc-vjpf, ReDoS on crafted glob patterns), which never ships and only reads the glob patterns of the project's own configuration.

### Upgrading from 0.5.0

- Nothing to do besides pulling the 0.5.1 images: no migration, no new secret, no configuration change.
- Signed-in members stay signed in: sessions written by 0.5.0 are read as they are.

## [0.5.0] - 2026-11-16

Work package **L05 Tickets and collaboration**: tickets with their fields, workflow and relations, rich comments with mentions, attachments scanned by an antivirus, the history of every change, notifications in the app and by email, real-time updates, and the migration of tickets when a configuration change is published.

### Added

- **Tickets** (#130)
  - Created from a project page ("New ticket") or from "My tickets", with the type's creation screen; keys per project (`PLAT-142`) allocated without gaps under concurrency; a fixed priority list of five levels.
  - The description is rich text (Tiptap editor), stored as a document and rendered by the server to sanitized HTML.
  - The ticket page (mockup 03): type and key, summary, status, description, details and custom fields, project.
- **Editing and moving** (#136)
  - An "Edit" dialog shows the type's view screen for the current status; saving checks the version read (`If-Match`, 412 when someone changed it meanwhile).
  - A ticket moves to another project with its sub-tasks: new keys, the former ones still open it; values of fields the target configuration lacks are listed, then dropped.
- **Links and sub-tasks** (#140)
  - Blocks, depends on, duplicates and relates to, each read from both tickets; sub-tasks added from their parent; the parent changes within the project, the type hierarchy enforced. A minimal ticket search (key, former key or summary) finds what to link.
- **Status transitions** (#515)
  - The transitions the workflow offers from the current status, as buttons on the ticket page; who may take each, the fields it asks for (transition screen), the fields required at the target status, then its post-functions (assign, set or clear a field, add a comment).
- **Comments** (#144)
  - Rich text with @mentions, internal by default or public; the author edits and deletes theirs, moderators delete any (a placeholder stays).
- **Attachments** (#148)
  - Uploaded and downloaded directly from the S3 storage with presigned URLs, on the application's own origin; every file is scanned by ClamAV before it can be downloaded, an infected one deleted. Image thumbnails, a size limit and a storage quota per tenant, one stored copy of a file uploaded twice.
- **History** (#153)
  - Every change of a ticket is recorded with who made it, when and from where (interface, API, MCP, import, system): the Activity tab tells it in sentences, a description before and after on demand, deleted comments for moderators only.
- **Notifications** (#157)
  - Ticket created, assigned, status changed, commented and mentioned notify the people the configuration's rules name, watchers included (Watch / Unwatch on the ticket page). A bell with the unread count, a notifications page, and preferences per event for the app and email, with an optional daily digest.
- **Real time** (#163)
  - The ticket page and the bell follow changes as they happen (Server-Sent Events); while a form is open, someone else's change shows a warning instead, the input kept.
- **Migrating tickets on a configuration change** (#125, #127)
  - The mapping wizard counts the tickets of every project of the configuration; once confirmed, the worker moves them batch by batch, each change in their history by the system, then publishes the draft. A migration interrupted midway resumes without moving a ticket twice. Publishing directly is refused while tickets use a removed status or type (`config.migration_required`).

### Upgrading from 0.4.0

- **Migrations** run at start-up (`migrate` service): tickets, links, comments, the history (`audit_log`) and the event outbox (both partitioned by month), attachments, watchers and notifications, all protected by Row-Level Security.
- **New secrets** `s3_access_key` and `s3_secret_key`: run `deploy/scripts/generate-secrets.sh` again. An installation from 0.4.0 already has `seaweedfs_s3_config`: the script takes both keys from it, so the storage keeps its identity.
- **Attachments** use the `seaweedfs` and `clamav` services and Traefik's route of the `/mlbpp-attachments/` prefix (all in `deploy/compose.yaml`). A proxy in front of Traefik must keep the Host header. On its first start ClamAV downloads its signatures for a few minutes: uploads stay "Scanning" meanwhile. Back up the `seaweedfs-data` volume besides the database (see "Attachments" in `deploy/README.md`).
- **New variables** (optional): `ATTACHMENT_MAX_BYTES` (50 MB by default, at most 100 MB); the worker now reads `PUBLIC_URL` for the links of notification emails, and `CLAMAV_ADDR` (set by `compose.yaml`).
- **Real time** goes through Valkey (one stream per tenant) and long-lived `GET /api/v1/events` connections: a proxy in front of Traefik must not buffer them.

## [0.4.0] - 2026-11-14

Work package **L04 Project configuration**: shared project configurations with their issue types, workflows, fields, transition rules, screens and notification rules, edited in a draft and published to every project at once, with the status and type mapping wizard.

### Added

- **Project configurations** (#95)
  - A configuration gathers a project's issue types, fields, workflows, screens and notifications; several projects share one, each project has exactly one. Every tenant gets a default configuration, which existing and new projects use.
  - Changes go to a draft shared by the configuration admins (tenant admins and configuration admins); publishing applies them to every project at once, after the list of the projects affected. A project detaches by duplicating its configuration.
  - Administration › Project configurations, the configuration page with its tabs, and the Configuration tab of a project's settings. Labels are translatable (French and English) everywhere.
- **Issue types** (#100)
  - Epic, Story, Task, Bug and Sub-task by default, each with its icon, color and the types it may sit under; sub-tasks always have a parent. Types are reordered by drag and drop or with the keyboard.
- **Workflows** (#109)
  - Statuses in three categories (To do, In progress, Done) with a portal label, and transitions from a status or from any; a standard workflow (mockup 11) for every type, more workflows added or copied.
  - Edited in tables, shown as a read-only graph. What keeps a workflow from being published (no initial status, a status nothing leads to…) is listed, and publishing waits for it (`workflow.invalid`).
- **Custom fields** (#104)
  - The 14 field types of the MVP with their options; the system fields Points and Remaining work.
  - Hidden, visible or required per issue type and per workflow step, in one grid per workflow; a published list value is deactivated rather than removed.
- **Transition rules** (#114)
  - Who may take a transition (roles on the project or groups), the fields it requires, and post-functions run in order: assign, set or clear a field, add a comment. Tickets (L05) apply them.
- **Screens** (#118)
  - General creation and view screens, screens for one type, and named transition screens attached to transitions; the field rules win over a screen (hidden fields left out, required ones added).
- **Notification rules** (#122)
  - Whom ticket created, assigned, status changed, commented and mentioned notify: the reporter, the assignee, watchers, the people mentioned, roles, groups or members; never the author of the change. Jira-style defaults; tickets (L05) send them.
- **Status and type mapping wizard** (#126, #128)
  - When a draft removes statuses or types that tickets use, publishing maps each to one the draft keeps (mockup 13; `mapping_incomplete` until all are) and migrates in the background, publishing once done.
  - Moving the tickets themselves comes with tickets in L05 (#125, #127, #129): until then no ticket needs moving.

### Upgrading from 0.3.0

- **Migrations** run at start-up (`migrate` service): the configuration tables (versions, types, workflows, fields and their rules, screens, notification rules, migrations), all protected by Row-Level Security. Every tenant gets its default configuration, which its projects use.
- No new secret nor variable. The worker runs the new `config.migration` job; nothing to configure.

## [0.3.0] - 2026-11-13

Work package **L03 Hierarchy**: portfolios, programs and projects, several weighted parents, roles granted on nodes and inherited, confidential projects, aggregated indicators, and archiving.

### Added

- **Portfolios, programs and projects** (#71)
  - Nodes with a name, a project key unique per tenant, description, objectives, dates, health (on track, at risk, off track) and owners, under `/api/v1/nodes`.
  - The attachment matrix and cycle prevention are enforced in the database; a closure table answers ancestors and descendants.
  - Settings of a node at `/nodes/:id/general`, and "Add a child", which creates the node and its link.
- **Several weighted parents** (#75)
  - A node is linked to several parents, each link with a weighting (0 to 100 %, default 100).
  - The "Attachments and permissions" tab: Parents card with the weighting and detach, a parent search that says why a candidate is refused, Children card.
  - Errors `node.parent_not_allowed` and `node.cycle` (409); two concurrent links closing a cycle are serialized.
- **Roles granted on nodes** (#80)
  - A role granted to a member or a group on a node applies to it and to everything below; with several parents, access is the union of every path.
  - Tenant admins, and members who manage the members of a node, grant roles within their own permissions only.
  - The "Effective access" card says where each role comes from (direct, or inherited from which node), and grants or revokes roles.
- **Confidential projects** (#84)
  - A confidential project only hears the roles granted on it, which still apply below it; the switch is audited.
  - A member who sees a parent without access to the project finds it veiled: "Confidential project", with no name, key, owner or real id. No name of a node a member cannot see leaves the API.
  - `node.confidential_lockout` (409): an administrator whose role is only inherited cannot shut themselves out.
- **Aggregated indicators** (#87)
  - A node shows its planned load, consumed capacity, open and late tickets over its distinct descendant projects: a project reached through several paths counts once, the weighting applies at its parent only, archived projects are left out, confidential ones counted apart.
  - Precomputed by the worker a couple of seconds after a change (`node.aggregates` job, changes in a burst share one job). See `docs/en/developer-guide/aggregates.md`.
  - `/nodes/:id` is now the view of a node (indicator cards and the tree of what is under it); the Portfolios page shows the top of the hierarchy as the same tree. `GET /api/v1/hierarchy` browses it one level at a time.
  - Tickets (L05) and workload (L08) will feed the figures; until then, indicators show the number of projects and zeros.
- **Archiving** (#92)
  - A project is archived or restored from the "Archiving" tab of its settings, after a confirmation; the change is audited.
  - An archived project is read-only (`node.archived`, 409, on every write on it, its links and its children), leaves the active lists and its ancestors' indicators, and comes back once restored. Sub-projects are archived first and restored after.
  - The Portfolios page switches between the hierarchy and the archived projects (`GET /api/v1/nodes?archived=true`).

### Fixed

- Deleting a group now forgets the cached permissions of its members at once (#498).

### Security

- OIDC sign-in: `jsonwebtoken` 10.4 (GHSA-h395-gr6q-cpjc, CVE-2026-25537: a time claim of the wrong type skipped its check). Its signatures are checked with `ring` through the application's own provider, keeping the `rsa` crate and `aws-lc` out; ID tokens not valid yet (`nbf`) are refused too.
- Development tools: `minimatch` 11 forced under eslint and typescript-eslint (denial of service through glob patterns); nothing of it ships.

### Upgrading from 0.2.0

- **Migrations** run at start-up (`migrate` service): the hierarchy, node grants and aggregates tables, all protected by Row-Level Security.
- No new secret nor variable. The worker runs the new `node.aggregates` job; nothing to configure.

## [0.2.0] - 2026-11-12

Work package **L02 Identities, roles, permissions**: accounts and tenants, sign-in (password, two-step verification, the organization's identity provider), members and groups, node roles and the permission engine, My account, and the GDPR rights.

### Added

- **Tenants** (#34)
  - Platform super admins create, change and delete tenants through `/api/v1/platform/tenants`. A deleted tenant stops resolving at once and is purged in the background.
  - Settings per tenant: language, time zone, default capacity, overload tolerance and mode, quotas.
  - The first tenant and its super admin are bootstrapped at the first start (`BOOTSTRAP_*`).
- **Local sign-in** (#38)
  - Global accounts with Argon2id passwords, and memberships per tenant (internal or external, tenant role).
  - Sessions in Valkey, behind `HttpOnly` cookies with a CSRF token.
  - Password reset by email, through the worker's job queue; sign-in and reset requests are rate-limited.
- **Two-step verification** (#44): authenticator app (TOTP) with 8 single-use backup codes; a tenant can require it.
- **Single sign-on** (#48)
  - OIDC providers per tenant (Entra ID, Google, Keycloak…), with encrypted client secrets.
  - Members are matched by their verified email or a directory claim.
  - A tenant can require SSO for its internal members; local accounts (contractors, the bootstrapped admin) keep their password.
- **Several tenants per account** (#52): the organization selector switches tenants through a one-time link, under the target tenant's policy. External accounts never reach the internal application.
- **Members and groups** (#55)
  - Administration › Members: search, filters, invitations by email (accepted on a page that creates or joins the account), access changes and removal. The last tenant admin always stays.
  - Administration › Groups.
- **Node roles and permissions** (#60)
  - A fixed permission catalog, and roles as sets of permissions edited in Administration › Roles. Four default roles per tenant.
  - The permission check answers 403 `permission.denied`; effective permissions are cached in Valkey and invalidated by changes. Grants on hierarchy nodes come with L03.
- **My account** (#64)
  - Language and time zone of the account, applied in every tenant.
  - Password change, which ends the other sessions.
  - Two-step verification, and the open sessions with their device and latest activity, ended one by one.
- **GDPR** (#67)
  - Export of one's personal data as JSON.
  - Erasure by anonymization: the account itself (with its password, or a recent SSO sign-in, and its email typed) or a platform super admin erases a global account; a tenant admin erases a member from their tenant only. Erased members appear as "Deleted user #N"; every erasure is audited.
  - Technical logs, which hold client IP addresses, are rotated and kept 6 months by Loki.
- **Development** (#487, #489, #491): `just clean-cache` frees the build caches; the local stack also answers on https://127.0.0.1:18443.

### Changed

- Debug builds keep line tables only, and no debug information for dependencies: the build caches are about three times smaller.

### Upgrading from 0.1.0

- **New secrets**, created by `deploy/scripts/generate-secrets.sh`:
  - `data_encryption_key`: encrypts the TOTP and OIDC client secrets; keep a copy with the backups;
  - `bootstrap_admin_password`;
  - `smtp_url`: a placeholder, to replace with your SMTP server.
- **New variables:** `PUBLIC_URL` (links of the emails) and `MAIL_FROM`; `BOOTSTRAP_ADMIN_EMAIL` to bootstrap the first super admin. The worker now also needs `VALKEY_URL` (compose provides it).
- **Migrations** run at start-up (`migrate` service). See `deploy/README.md` and `deploy/.env.example`.

## [0.1.0] - 2026-11-10

Work package **L01 Technical foundation**: the base every later work package builds on. It ships no business feature yet: the screens are placeholders, and the only API resource is the reference resource that new endpoints copy.

### Added

- **Backend** (Rust workspace: `domain`, `infra`, `api`, `worker`, `mcp`; #1)
  - Configuration from environment variables, with secrets read from files (`*_FILE`); graceful shutdown.
  - PostgreSQL 18 with migrations, two roles (owner, runtime without `BYPASSRLS`) and row-level security on every business table; the tenant is set per transaction and cannot be forgotten (`TenantTx`); single- or multi-tenant mode (#5).
  - RFC 9457 Problem Details errors with a catalog of stable codes, including for framework errors; reference resource `/api/v1/reference-items` showing every API convention: validation with field errors, `ETag`/`If-Match`, `Idempotency-Key`, cursor pagination, field selection, translated labels (#10).
  - OpenAPI 3.1 contract generated from the code, served at `/api/v1/openapi.json`, with self-hosted interactive documentation at `/api/v1/docs/` (#14).
  - Health endpoints `/health/live` and `/health/ready` in every service; JSON logs with a correlation ID (`x-request-id`); OpenTelemetry traces and metrics exported in OTLP when configured (#31).
- **Front end** (Angular 22 workspace: `app`, `portal`, `ui`; #14, #17)
  - Typed API client generated from the contract, with errors turned into translated `ApiProblem`s.
  - Material 3 theme with light and dark modes from tokens only; self-hosted Roboto and Material Symbols.
  - Application shell with the navigation of the mockups, translations in French and English (Transloco), locale-aware date and number formatting (planning dates never converted through UTC).
  - Shared status and type chips, empty, error and loading states.
- **Deployment** (#22)
  - Production images: non-root, distroless for the Rust services, nginx with a strict Content-Security-Policy for the front end, PostgreSQL with pgBackRest; an SBOM for each.
  - Docker Compose stack: Traefik (TLS, load balancing), web, api, worker, mcp, PostgreSQL, Valkey, SeaweedFS, ClamAV; profiles `sso` (Keycloak) and `observability` (Prometheus, Loki, Tempo, Alloy, Grafana with a dashboard); `api` and `worker` scale horizontally.
  - Scheduled, encrypted pgBackRest backups with WAL archiving; technical log retention of 6 months by default.
- **Quality** (#26)
  - Unit, integration (real PostgreSQL), end-to-end (Playwright) and accessibility (axe-core) tests.
  - License policy enforced on Rust crates and npm packages, RustSec and npm advisories, secret scan of the git history.
  - CI: every step a `just` recipe (`just ci` locally), GitHub Actions only calls them; release workflow publishing the images to GitHub Container Registry.

[0.8.0]: https://github.com/argoleon/MyLittleBigProjectPlanner/releases/tag/v0.8.0
[0.7.0]: https://github.com/argoleon/MyLittleBigProjectPlanner/releases/tag/v0.7.0
[0.6.0]: https://github.com/argoleon/MyLittleBigProjectPlanner/releases/tag/v0.6.0
[0.5.1]: https://github.com/argoleon/MyLittleBigProjectPlanner/releases/tag/v0.5.1
[0.5.0]: https://github.com/argoleon/MyLittleBigProjectPlanner/releases/tag/v0.5.0
[0.4.0]: https://github.com/argoleon/MyLittleBigProjectPlanner/releases/tag/v0.4.0
[0.3.0]: https://github.com/argoleon/MyLittleBigProjectPlanner/releases/tag/v0.3.0
[0.2.0]: https://github.com/argoleon/MyLittleBigProjectPlanner/releases/tag/v0.2.0
[0.1.0]: https://github.com/argoleon/MyLittleBigProjectPlanner/releases/tag/v0.1.0
