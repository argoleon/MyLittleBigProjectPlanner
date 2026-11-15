-- Attachments of tickets (L05-US05). Specification §12, §18.
--
-- The content lives in the S3 storage, uploaded and downloaded by the
-- browser through presigned URLs; the database keeps what describes it.
-- A file is uploaded to its own key, scanned by the antivirus, then kept
-- if clean: its SHA-256 deduplicates it within the tenant (`object_key`
-- then points to the first object of that content, and its own upload is
-- removed). An infected or rejected file's content is removed.
CREATE TABLE attachment (
    id            uuid PRIMARY KEY,
    tenant_id     uuid NOT NULL REFERENCES tenant (id) ON DELETE CASCADE,
    issue_id      uuid NOT NULL,
    name          text NOT NULL CHECK (length(name) BETWEEN 1 AND 255),
    -- Announced at upload, checked once uploaded.
    size          bigint NOT NULL CHECK (size > 0),
    -- As the browser gave it; downloads are served by what the bytes are.
    media_type    text NOT NULL DEFAULT 'application/octet-stream' CHECK (length(media_type) <= 255),
    status        text NOT NULL DEFAULT 'pending'
                  CHECK (status IN ('pending', 'scanning', 'clean', 'infected', 'rejected')),
    -- Known once scanned: hexadecimal SHA-256 of the content.
    sha256        text CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    -- Where the content is: its upload, or the first object of that content.
    object_key    text NOT NULL,
    -- An image's, once scanned.
    thumbnail_key text,
    -- The type it is served with once clean (an image's, else bytes).
    served_type   text,
    -- The signature the antivirus found.
    signature     text,
    -- None: an erased account.
    uploaded_by   uuid REFERENCES account (id) ON DELETE SET NULL,
    scanned_at    timestamptz,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    version       bigint NOT NULL DEFAULT 1,
    CONSTRAINT attachment_clean_is_known CHECK (
        status <> 'clean' OR (sha256 IS NOT NULL AND served_type IS NOT NULL AND scanned_at IS NOT NULL)
    ),
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, issue_id) REFERENCES issue (tenant_id, id) ON DELETE CASCADE
);

CREATE INDEX attachment_tenant_issue_idx ON attachment (tenant_id, issue_id, created_at, id);
-- The first clean object of a content, for deduplication.
CREATE INDEX attachment_tenant_content_idx ON attachment (tenant_id, sha256) WHERE status = 'clean';
CREATE INDEX attachment_tenant_object_idx ON attachment (tenant_id, object_key);

SELECT app_setup_business_table('attachment');

-- The history records attachments added and removed.
ALTER TABLE audit_log DROP CONSTRAINT audit_log_action_check;
ALTER TABLE audit_log ADD CONSTRAINT audit_log_action_check CHECK (action IN (
    'created', 'updated', 'moved', 'link_added', 'link_removed',
    'comment_created', 'comment_updated', 'comment_deleted',
    'attachment_added', 'attachment_removed'));
