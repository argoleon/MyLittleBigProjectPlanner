-- Comments of tickets (L05-US04): rich text (Tiptap JSON, rendered to
-- sanitized HTML when written), internal or public, the accounts it
-- mentions. A deleted comment stays as a placeholder without its content.

CREATE TABLE comment (
    id           uuid PRIMARY KEY,
    tenant_id    uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    issue_id     uuid NOT NULL,
    -- None: an erased account.
    author_id    uuid REFERENCES account (id) ON DELETE SET NULL,
    visibility   text NOT NULL DEFAULT 'internal' CHECK (visibility IN ('internal', 'public')),
    -- Written by a member, or by a transition's "add a comment" (L05-US09).
    source       text NOT NULL DEFAULT 'member' CHECK (source IN ('member', 'transition')),
    content      jsonb CHECK (content IS NULL OR jsonb_typeof(content) = 'object'),
    content_html text,
    mentions     uuid[] NOT NULL DEFAULT '{}',
    edited_at    timestamptz,
    deleted_at   timestamptz,
    deleted_by   uuid REFERENCES account (id) ON DELETE SET NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    version      bigint NOT NULL DEFAULT 1,
    -- Content while it exists, none once deleted.
    CONSTRAINT comment_content_or_deleted CHECK (
        (deleted_at IS NULL AND content IS NOT NULL AND content_html IS NOT NULL)
        OR (deleted_at IS NOT NULL AND content IS NULL AND content_html IS NULL AND mentions = '{}')
    ),
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, issue_id) REFERENCES issue (tenant_id, id) ON DELETE CASCADE
);

CREATE INDEX comment_tenant_issue_idx ON comment (tenant_id, issue_id, created_at, id);

SELECT app_setup_business_table('comment');
