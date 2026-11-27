-- Scrum boards, sprints and the backlog rank (specification §6, L09-US01).
--
-- * A board is now `kanban` or `scrum`; only a Scrum board has sprints.
-- * `sprint`: belongs to one Scrum board, `future` until started, then
--   `active`, then `closed` (L09-US02). `sequence` orders the sprints of a
--   board and numbers the default names ("PLAT Sprint 4").
-- * `sprint_issue`: the history of a sprint's scope, kept for the burndown
--   and the velocity: a row per stay of a ticket in a sprint, `removed_at`
--   set when it leaves before the sprint closes. A ticket is in at most one
--   open sprint (future or active) of the tenant: `sprint_open` is cleared
--   on the sprint's rows when it closes, and a partial unique index holds
--   the rule. Sub-tasks have no rows: they follow their parent.
-- * `board_rank`: the manual order of a board's tickets, as lexicographic
--   ranks (domain::rank) compared byte by byte (`COLLATE "C"`), so that a
--   move writes one row. Tickets without a rank come after the ranked ones,
--   in the order of the board's filter.

ALTER TABLE board DROP CONSTRAINT board_kind_check;
ALTER TABLE board ADD CONSTRAINT board_kind_check CHECK (kind IN ('kanban', 'scrum'));

CREATE TABLE sprint (
    id         uuid PRIMARY KEY,
    tenant_id  uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    board_id   uuid NOT NULL,
    sequence   integer NOT NULL CHECK (sequence > 0),
    name       text NOT NULL CHECK (length(name) BETWEEN 1 AND 100 AND name = btrim(name)),
    goal       text CHECK (goal IS NULL OR (length(goal) BETWEEN 1 AND 1000 AND goal = btrim(goal))),
    -- Planning dates, local to the team; both or none.
    start_date date,
    end_date   date,
    state      text NOT NULL DEFAULT 'future' CHECK (state IN ('future', 'active', 'closed')),
    started_at timestamptz,
    closed_at  timestamptz,
    created_by uuid REFERENCES account (id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    version    bigint NOT NULL DEFAULT 1,
    CHECK ((start_date IS NULL) = (end_date IS NULL)),
    CHECK (end_date IS NULL OR end_date >= start_date),
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, board_id, sequence),
    FOREIGN KEY (tenant_id, board_id) REFERENCES board (tenant_id, id) ON DELETE CASCADE
);
-- Sprint names are what `sprint = "…"` looks for.
CREATE INDEX sprint_tenant_name_idx ON sprint (tenant_id, lower(name));
SELECT app_setup_business_table('sprint');

CREATE TABLE sprint_issue (
    id          uuid PRIMARY KEY,
    tenant_id   uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    sprint_id   uuid NOT NULL,
    issue_id    uuid NOT NULL,
    sprint_open boolean NOT NULL DEFAULT true,
    added_at    timestamptz NOT NULL DEFAULT now(),
    added_by    uuid REFERENCES account (id) ON DELETE SET NULL,
    removed_at  timestamptz,
    removed_by  uuid REFERENCES account (id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    CHECK (removed_at IS NULL OR removed_at >= added_at),
    FOREIGN KEY (tenant_id, sprint_id) REFERENCES sprint (tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, issue_id) REFERENCES issue (tenant_id, id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX sprint_issue_one_open_idx ON sprint_issue (tenant_id, issue_id)
    WHERE removed_at IS NULL AND sprint_open;
CREATE INDEX sprint_issue_sprint_idx ON sprint_issue (tenant_id, sprint_id) WHERE removed_at IS NULL;
CREATE INDEX sprint_issue_issue_idx ON sprint_issue (tenant_id, issue_id);
SELECT app_setup_business_table('sprint_issue');

CREATE TABLE board_rank (
    tenant_id  uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    board_id   uuid NOT NULL,
    issue_id   uuid NOT NULL,
    rank       text COLLATE "C" NOT NULL CHECK (rank ~ '^[0-9a-z]*[1-9a-z]$' AND length(rank) <= 200),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, board_id, issue_id),
    UNIQUE (tenant_id, board_id, rank),
    FOREIGN KEY (tenant_id, board_id) REFERENCES board (tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, issue_id) REFERENCES issue (tenant_id, id) ON DELETE CASCADE
);
SELECT app_setup_business_table('board_rank');
