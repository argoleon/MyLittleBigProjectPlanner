-- Rules of a workflow transition (L04-US05). Specification §5.
--
-- `rules` holds who may take the transition (roles and groups of the
-- tenant), the fields it requires and its post-functions in their order
-- (assign, set or clear a field, add a comment): domain::transitions, as
-- infra::transition_rules stores it. Roles, groups and members are the
-- tenant's, fields the version's (by `item_id`); a field removed from a
-- draft leaves the rules of its transitions. The transition screen comes
-- with the screens (L04-US06).
ALTER TABLE workflow_transition
    ADD COLUMN rules jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(rules) = 'object');
