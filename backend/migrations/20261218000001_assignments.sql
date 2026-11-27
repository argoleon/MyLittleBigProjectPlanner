-- Assignments of resources to tickets (specification §8, L08-US01): one
-- assignment per resource and ticket, made of successive segments, each
-- with its own period and mode. The daily load derived from them comes
-- with L08-US02.

-- Equality on uuid in the exclusion constraint of segments.
CREATE EXTENSION IF NOT EXISTS btree_gist;

CREATE TABLE assignment (
    id          uuid PRIMARY KEY,
    tenant_id   uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    issue_id    uuid NOT NULL,
    resource_id uuid NOT NULL,
    -- Hatched in the heatmap; counted or not in the overload check (L08-US09).
    provisional boolean NOT NULL DEFAULT false,
    created_by  uuid REFERENCES account (id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    version     bigint NOT NULL DEFAULT 1,
    UNIQUE (tenant_id, id),
    -- One assignment per resource and ticket: a new period is a new segment.
    UNIQUE (tenant_id, issue_id, resource_id),
    FOREIGN KEY (tenant_id, issue_id) REFERENCES issue (tenant_id, id) ON DELETE CASCADE,
    -- Resources are never deleted, only deactivated.
    FOREIGN KEY (tenant_id, resource_id) REFERENCES resource (tenant_id, id)
);
CREATE INDEX assignment_resource_idx ON assignment (tenant_id, resource_id);
SELECT app_setup_business_table('assignment');

CREATE TABLE assignment_segment (
    id            uuid PRIMARY KEY,
    tenant_id     uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    assignment_id uuid NOT NULL,
    -- Order in which the segments were given.
    position      smallint NOT NULL CHECK (position >= 0),
    start_date    date NOT NULL,
    end_date      date NOT NULL CHECK (end_date >= start_date),
    mode          text NOT NULL CHECK (mode IN ('percentage', 'hours_per_day', 'total_effort')),
    -- A percentage (1 to 100), minutes per working day (up to a day), or
    -- minutes over the whole period.
    value         integer NOT NULL CHECK (
        value > 0 AND CASE mode
            WHEN 'percentage' THEN value <= 100
            WHEN 'hours_per_day' THEN value <= 1440
            ELSE value <= 599940
        END
    ),
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, assignment_id, position),
    FOREIGN KEY (tenant_id, assignment_id) REFERENCES assignment (tenant_id, id) ON DELETE CASCADE,
    -- Segments of an assignment never take a same day.
    CONSTRAINT assignment_segment_no_overlap EXCLUDE USING gist (
        tenant_id WITH =,
        assignment_id WITH =,
        daterange(start_date, end_date, '[]') WITH &&
    )
);
SELECT app_setup_business_table('assignment_segment');
