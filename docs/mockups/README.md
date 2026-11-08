# MVP mockups · My Little Big Project Planner

**English** · [Français](README.fr.md)

Interface preview in Material Design 3: 25 screens covering every MVP work package described in the [specification](../en/specification.md).

The screens are in French, the language of the first users. The data shown is fictional but consistent from one screen to the next: ticket PLAT-142, the Platform team, the IT Modernization program.

## Reference version

The up-to-date canvas, with comments and export, is on claude.ai:
<https://claude.ai/artifact/6uvxyw4AxahHgEJvCPKVMx>

This link is private: share the canvas from its **Share** menu to give the team access.

## Previews

PNG captures of every screen are in [`previews/`](previews/), rendered at a width of 1440 px (dialogs at their own size). They are snapshots of the canvas on 8 November 2026: when the canvas changes, export the screens again (**Share › Export**) and replace the files.

## Screens

| Area | Screen | Source file | Preview |
| --- | --- | --- | --- |
| Key screens | Workload heatmap | `source/Main.dc.html` | [PNG](previews/01-workload-heatmap.png) |
| Key screens | Multi-project Gantt | `source/Gantt.dc.html` | [PNG](previews/02-gantt.png) |
| Key screens | Ticket details | `source/Ticket.dc.html` | [PNG](previews/03-ticket.png) |
| Key screens | Kanban board | `source/Kanban.dc.html` | [PNG](previews/04-kanban-board.png) |
| Key screens | Portfolio view | `source/Portefeuille.dc.html` | [PNG](previews/05-portfolio.png) |
| Key screens | Requester portal | `source/Portail.dc.html` | [PNG](previews/06-requester-portal.png) |
| Access and administration | Sign-in and two-step verification | `source/Login.dc.html` | [PNG](previews/07-sign-in-mfa.png) |
| Access and administration | OAuth consent for the MCP client | `source/OAuthConsent.dc.html` | [PNG](previews/08-oauth-consent.png) |
| Access and administration | Members and roles | `source/AdminMembres.dc.html` | [PNG](previews/09-members-roles.png) |
| Access and administration | My account: security, API tokens, GDPR | `source/Compte.dc.html` | [PNG](previews/10-my-account.png) |
| Configuration and hierarchy | Workflows | `source/ConfigWorkflow.dc.html` | [PNG](previews/11-workflows.png) |
| Configuration and hierarchy | Custom fields | `source/ConfigChamps.dc.html` | [PNG](previews/12-custom-fields.png) |
| Configuration and hierarchy | Status migration wizard | `source/MigrationStatuts.dc.html` | [PNG](previews/13-status-migration.png) |
| Configuration and hierarchy | Project parents and permissions | `source/Hierarchie.dc.html` | [PNG](previews/14-project-parents-permissions.png) |
| Search, agile and time | Search and filters | `source/Recherche.dc.html` | [PNG](previews/15-search.png) |
| Search, agile and time | Notifications and preferences | `source/Notifications.dc.html` | [PNG](previews/16-notifications.png) |
| Search, agile and time | Backlog and sprints | `source/Backlog.dc.html` | [PNG](previews/17-backlog-sprints.png) |
| Search, agile and time | Burndown, velocity and versions | `source/AgileVersions.dc.html` | [PNG](previews/18-burndown-velocity-versions.png) |
| Search, agile and time | Time logging | `source/SaisieTemps.dc.html` | [PNG](previews/19-time-logging.png) |
| Resources and planning | Resource: capacity, calendar, absences | `source/Ressource.dc.html` | [PNG](previews/20-resource.png) |
| Resources and planning | New assignment, overload blocked | `source/Affectation.dc.html` | [PNG](previews/21-new-assignment.png) |
| Resources and planning | Recurring task | `source/Recurrence.dc.html` | [PNG](previews/22-recurring-task.png) |
| Portal and dashboards | Portal settings | `source/PortailAdmin.dc.html` | [PNG](previews/23-portal-settings.png) |
| Portal and dashboards | Tracking a request through a link | `source/PortailSuivi.dc.html` | [PNG](previews/24-request-tracking.png) |
| Portal and dashboards | Dashboard | `source/Dashboard.dc.html` | [PNG](previews/25-dashboard.png) |

`source/canvas.json` describes the layout of the screens on the canvas.
