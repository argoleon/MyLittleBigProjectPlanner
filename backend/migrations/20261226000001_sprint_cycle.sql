-- Starting and closing sprints (specification §6, L09-US02).
--
-- * A board has one active sprint at most (partial unique index).
-- * Starting a sprint snapshots its committed scope: each stay then in the
--   sprint is `committed`, with the ticket's estimate at that moment; the
--   sprint keeps the totals. Tickets added afterwards are not committed.
-- * Closing it snapshots what was completed: each stay still in the sprint
--   gets `completed` (its status of the `done` category) and the estimate
--   at that moment; the sprint keeps the totals. The unfinished tickets
--   start a new stay in the sprint chosen, or go back to the backlog; the
--   closed sprint's rows stay as they were (`sprint_open` cleared).
-- Estimates are in the board's estimation (points or hours).

ALTER TABLE sprint
    ADD COLUMN committed_count    integer CHECK (committed_count >= 0),
    ADD COLUMN committed_estimate double precision CHECK (committed_estimate >= 0),
    ADD COLUMN completed_count    integer CHECK (completed_count >= 0),
    ADD COLUMN completed_estimate double precision CHECK (completed_estimate >= 0),
    ADD CONSTRAINT sprint_started_check CHECK (
        state = 'future' OR (start_date IS NOT NULL AND started_at IS NOT NULL AND committed_count IS NOT NULL)),
    ADD CONSTRAINT sprint_closed_check CHECK (
        state <> 'closed' OR (closed_at IS NOT NULL AND completed_count IS NOT NULL));

CREATE UNIQUE INDEX sprint_one_active_idx ON sprint (tenant_id, board_id) WHERE state = 'active';

ALTER TABLE sprint_issue
    ADD COLUMN committed         boolean NOT NULL DEFAULT false,
    ADD COLUMN estimate_at_start double precision,
    ADD COLUMN completed         boolean,
    ADD COLUMN estimate_at_close double precision;
