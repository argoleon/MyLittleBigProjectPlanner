-- Replacing a generic resource with a person (specification §8, L08-US08):
-- each ticket whose assignment moves records it in its history
-- (`resource_replaced`: the generic resource, the person, the first day).

ALTER TABLE audit_log DROP CONSTRAINT audit_log_action_check;
ALTER TABLE audit_log ADD CONSTRAINT audit_log_action_check CHECK (action IN (
    'created', 'updated', 'moved', 'link_added', 'link_removed',
    'comment_created', 'comment_updated', 'comment_deleted',
    'attachment_added', 'attachment_removed', 'overload_forced', 'resource_replaced'));
