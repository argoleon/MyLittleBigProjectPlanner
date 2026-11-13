# Hierarchy aggregates

A portfolio, program or project shows its planned load, consumed capacity, progress, open and late tickets, summed over its **distinct descendant projects** (specification §4). These figures are precomputed: a view reads one row per node, whatever the size of the tenant.

| Layer | File | Holds |
| --- | --- | --- |
| Rules | `backend/crates/domain/src/aggregates.rs` | The aggregation over the graph, the debounce, no I/O |
| Tables | `backend/migrations/*_node_aggregates.sql` | `project_figure`, `node_aggregate`, `node_aggregate_stale` |
| SQL | `backend/crates/infra/src/aggregates.rs` | `mark_stale`, `set_figures`, `recompute`, `find` |
| Job | `backend/crates/worker/src/jobs.rs` | `node.aggregates` runs `recompute` in its tenant |
| HTTP | `backend/crates/api/src/hierarchy.rs` | `GET /hierarchy`, `GET /nodes/{id}/aggregate` |

## Rules

- A project reached through several paths counts once.
- The weighting of a link applies to the aggregates of **that link's parent only**. Higher up, the project counts whole. A project that is both a direct child and reached through a longer path counts whole too. A project never counts for more than 100 %.
- The weighting applies to amounts (planned load, capacity). Tickets are counted whole.
- A project aggregates itself and its sub-projects.
- Archived projects leave the aggregates of their ancestors.
- Confidential projects are counted like the others. Their number and their open tickets are also kept apart, so that a view can say what is anonymous. A member who sees a node sees its totals; a confidential project they have no access to shows as a veiled row with its planned load only.

## Where the figures come from

`project_figure` holds what is measured on a project by itself: planned minutes, capacity minutes, tickets by status category, late tickets. Nothing writes it yet: tickets (L05) and workload (L08) will, through `infra::aggregates::set_figures`. A project without a row counts as a project with nothing measured.

## Keeping aggregates up to date

A change that may alter aggregates calls `infra::aggregates::mark_stale` **in its own transaction**: a link added or removed, a weighting, the confidentiality of a project, its archiving or restoring, its figures. The repository functions of `infra::nodes` already do it; a new kind of change (tickets, workload) must do the same.

`mark_stale` writes the changed nodes to `node_aggregate_stale` and makes sure a `node.aggregates` job is queued for the tenant:

- no job queued: one is queued `RECOMPUTE_DELAY` (2 s) later;
- a job already queued within that delay: nothing more, it will take this change too. Changes made in a burst share one recompute;
- the queued job is locked (`FOR SHARE`) until the transaction ends, so that no worker claims it before the change is committed;
- a job already running does not count: it may have read the hierarchy before the change, so another one is queued.

The job takes every mark of its tenant, loads the links and the projects, and recomputes the marked nodes **and all their ancestors**. It only writes the rows that changed. Recomputes of a tenant run one at a time (advisory lock). With the worker polling every second, aggregates follow a change within about three seconds.

`infra::aggregates::mark_all_stale` asks for a full recompute of a tenant.

## Measures

Measured by the integration tests, on a developer machine, debug build:

| What | Test | Result |
| --- | --- | --- |
| Full recompute of a tenant of 1,000 projects (1,110 nodes, 2,100 links, each project under two programs) | `infra/tests/aggregates.rs` | about 40 ms |
| Recompute after one project changed (5 nodes) in that tenant | same | about 10 ms |
| `GET /hierarchy` and `GET /nodes/{id}/aggregate` on a portfolio of 1,000 projects, 95th percentile | `api/tests/hierarchy.rs` | under 20 ms (target: 200 ms) |

The tests fail above 3 s for the recompute and above 200 ms for the endpoints.
