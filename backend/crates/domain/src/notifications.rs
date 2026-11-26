//! Notification rules of a project configuration (specification §5 and
//! §12, L04-US07): which ticket events notify whom, for the projects using
//! the configuration.
//!
//! One rule per event lists its recipients: people of the ticket (its
//! reporter, assignee, watchers, the people mentioned), the holders of
//! roles on the ticket's project (granted there or above), groups and
//! chosen members. [`resolve`] turns a rule into the accounts to notify
//! when the event happens; the author of the change is never one of them.
//! Each member's own preferences filter them further (L05).

use std::collections::BTreeSet;

use crate::validation::{FieldError, Violations};

/// A ticket event that notifies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Event {
    Created,
    Assigned,
    StatusChanged,
    Commented,
    Mentioned,
    /// Hours planned in hours per day or total effort fall on an absence of
    /// their resource (L08-US04): to reschedule.
    AbsenceConflict,
}

impl Event {
    pub const ALL: [Self; 6] = [
        Self::Created,
        Self::Assigned,
        Self::StatusChanged,
        Self::Commented,
        Self::Mentioned,
        Self::AbsenceConflict,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Assigned => "assigned",
            Self::StatusChanged => "status_changed",
            Self::Commented => "commented",
            Self::Mentioned => "mentioned",
            Self::AbsenceConflict => "absence_conflict",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|event| event.as_str() == raw)
    }

    /// A stable number, to count the changes of a draft by event.
    pub const fn index(self) -> u128 {
        match self {
            Self::Created => 1,
            Self::Assigned => 2,
            Self::StatusChanged => 3,
            Self::Commented => 4,
            Self::Mentioned => 5,
            Self::AbsenceConflict => 6,
        }
    }

    /// Whether people can be mentioned when it happens.
    pub const fn has_mentions(self) -> bool {
        matches!(self, Self::Commented | Self::Mentioned)
    }
}

/// People of the ticket a rule may notify.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RecipientKind {
    Reporter,
    Assignee,
    Watchers,
    /// The people mentioned: comments and mentions only.
    Mentioned,
}

impl RecipientKind {
    pub const ALL: [Self; 4] = [Self::Reporter, Self::Assignee, Self::Watchers, Self::Mentioned];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reporter => "reporter",
            Self::Assignee => "assignee",
            Self::Watchers => "watchers",
            Self::Mentioned => "mentioned",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == raw)
    }
}

/// Whom an event notifies.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rule {
    pub kinds: Vec<RecipientKind>,
    pub roles: Vec<u128>,
    pub groups: Vec<u128>,
    pub accounts: Vec<u128>,
}

impl Rule {
    /// Whether it notifies nobody.
    pub fn is_off(&self) -> bool {
        self.kinds.is_empty() && self.roles.is_empty() && self.groups.is_empty() && self.accounts.is_empty()
    }
}

/// What a rule may name in the tenant.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct References {
    pub roles: BTreeSet<u128>,
    pub groups: BTreeSet<u128>,
    /// Active members.
    pub members: BTreeSet<u128>,
}

/// The rule of `event` an admin sets, checked: each recipient once, the
/// people mentioned only where mentions happen, roles, groups and members
/// of the tenant. Every error at once.
pub fn validate_rule(event: Event, rule: &Rule, refs: &References) -> Result<(), Vec<FieldError>> {
    let mut v = Violations::new();
    let mut kinds = BTreeSet::new();
    for (i, kind) in rule.kinds.iter().enumerate() {
        if !kinds.insert(*kind) {
            v.add(format!("recipientKinds[{i}]"), "duplicate");
        } else if *kind == RecipientKind::Mentioned && !event.has_mentions() {
            v.add(format!("recipientKinds[{i}]"), "not_applicable");
        }
    }
    ids_errors(&mut v, "roleIds", &rule.roles, &refs.roles, "not_found");
    ids_errors(&mut v, "groupIds", &rule.groups, &refs.groups, "not_found");
    ids_errors(
        &mut v,
        "accountIds",
        &rule.accounts,
        &refs.members,
        "unknown_member",
    );
    v.into_result()
}

fn ids_errors(v: &mut Violations, path: &str, ids: &[u128], known: &BTreeSet<u128>, missing: &'static str) {
    let mut seen = BTreeSet::new();
    for (i, id) in ids.iter().enumerate() {
        if !known.contains(id) {
            v.add(format!("{path}[{i}]"), missing);
        } else if !seen.insert(*id) {
            v.add(format!("{path}[{i}]"), "duplicate");
        }
    }
}

/// The people of the ticket when the event happens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TicketPeople {
    pub reporter: Option<u128>,
    /// The assignee after the event (the new one for `assigned`).
    pub assignee: Option<u128>,
    pub watchers: BTreeSet<u128>,
    /// The people the comment or the mention names.
    pub mentioned: BTreeSet<u128>,
}

/// The accounts the event notifies: the people of the ticket the rule
/// names, the holders of its roles on the ticket's project (`role_holders`,
/// granted there or above, as L05 finds them), the members of its groups
/// (`group_members`) and its chosen members; never `author`, who made the
/// change.
pub fn resolve(
    rule: &Rule,
    people: &TicketPeople,
    author: u128,
    role_holders: &dyn Fn(u128) -> BTreeSet<u128>,
    group_members: &dyn Fn(u128) -> BTreeSet<u128>,
) -> BTreeSet<u128> {
    let mut recipients = BTreeSet::new();
    for kind in &rule.kinds {
        match kind {
            RecipientKind::Reporter => recipients.extend(people.reporter),
            RecipientKind::Assignee => recipients.extend(people.assignee),
            RecipientKind::Watchers => recipients.extend(people.watchers.iter().copied()),
            RecipientKind::Mentioned => recipients.extend(people.mentioned.iter().copied()),
        }
    }
    for role in &rule.roles {
        recipients.extend(role_holders(*role));
    }
    for group in &rule.groups {
        recipients.extend(group_members(*group));
    }
    recipients.extend(rule.accounts.iter().copied());
    recipients.remove(&author);
    recipients
}

/// The rule every new configuration has for an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultRule {
    pub event: Event,
    pub kinds: &'static [RecipientKind],
    /// Default roles of the tenant, by English label, whose holders it
    /// notifies too (a role renamed or deleted since is left out).
    pub roles: &'static [&'static str],
}

/// The English label of the default role managing projects.
pub const PROJECT_MANAGER_ROLE: &str = "Project manager";

/// The default rules (`app_seed_default_notifications`; an integration
/// test keeps both in step), as Jira's default scheme.
pub const DEFAULT_RULES: [DefaultRule; 6] = [
    DefaultRule {
        event: Event::Created,
        kinds: &[
            RecipientKind::Reporter,
            RecipientKind::Assignee,
            RecipientKind::Watchers,
        ],
        roles: &[],
    },
    DefaultRule {
        event: Event::Assigned,
        kinds: &[RecipientKind::Assignee],
        roles: &[],
    },
    DefaultRule {
        event: Event::StatusChanged,
        kinds: &[
            RecipientKind::Reporter,
            RecipientKind::Assignee,
            RecipientKind::Watchers,
        ],
        roles: &[],
    },
    DefaultRule {
        event: Event::Commented,
        kinds: &[
            RecipientKind::Reporter,
            RecipientKind::Assignee,
            RecipientKind::Watchers,
        ],
        roles: &[],
    },
    DefaultRule {
        event: Event::Mentioned,
        kinds: &[RecipientKind::Mentioned],
        roles: &[],
    },
    // Who plans the work reschedules it (L08-US04).
    DefaultRule {
        event: Event::AbsenceConflict,
        kinds: &[RecipientKind::Assignee],
        roles: &[PROJECT_MANAGER_ROLE],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    const ALICE: u128 = 1;
    const BOB: u128 = 2;
    const CAROL: u128 = 3;
    const DAN: u128 = 4;
    const ERIN: u128 = 5;
    const MANAGER: u128 = 10;
    const QUALITY: u128 = 20;

    fn people() -> TicketPeople {
        TicketPeople {
            reporter: Some(ALICE),
            assignee: Some(BOB),
            watchers: [CAROL, ALICE].into(),
            mentioned: [DAN].into(),
        }
    }

    fn holders(role: u128) -> BTreeSet<u128> {
        if role == MANAGER {
            [ERIN].into()
        } else {
            BTreeSet::new()
        }
    }

    fn members(group: u128) -> BTreeSet<u128> {
        if group == QUALITY {
            [CAROL, DAN].into()
        } else {
            BTreeSet::new()
        }
    }

    #[test]
    fn events_and_kinds_round_trip() {
        for event in Event::ALL {
            assert_eq!(Event::parse(event.as_str()), Some(event));
        }
        for kind in RecipientKind::ALL {
            assert_eq!(RecipientKind::parse(kind.as_str()), Some(kind));
        }
        let indexes: BTreeSet<u128> = Event::ALL.iter().map(|e| e.index()).collect();
        assert_eq!(indexes.len(), Event::ALL.len());
        assert_eq!(Event::parse("updated"), None);
    }

    #[test]
    fn a_rule_notifies_the_people_of_the_ticket_it_names() {
        // "Ticket created": the reporter and the project managers; Bob created it.
        let created = Rule {
            kinds: vec![RecipientKind::Reporter],
            roles: vec![MANAGER],
            ..Rule::default()
        };
        assert_eq!(
            resolve(&created, &people(), BOB, &holders, &members),
            [ALICE, ERIN].into()
        );
        let everyone = Rule {
            kinds: RecipientKind::ALL.to_vec(),
            roles: vec![MANAGER],
            groups: vec![QUALITY],
            accounts: vec![ALICE, 99],
        };
        assert_eq!(
            resolve(&everyone, &people(), 0, &holders, &members),
            [ALICE, BOB, CAROL, DAN, ERIN, 99].into()
        );
    }

    #[test]
    fn the_author_is_never_notified_of_their_own_change() {
        let commented = Rule {
            kinds: vec![
                RecipientKind::Reporter,
                RecipientKind::Assignee,
                RecipientKind::Watchers,
            ],
            ..Rule::default()
        };
        // Alice comments: she is the reporter and a watcher.
        assert_eq!(
            resolve(&commented, &people(), ALICE, &holders, &members),
            [BOB, CAROL].into()
        );
        // Even when chosen by name.
        let chosen = Rule {
            accounts: vec![ALICE],
            ..Rule::default()
        };
        assert!(resolve(&chosen, &people(), ALICE, &holders, &members).is_empty());
        // Nobody to notify, or an unassigned ticket.
        assert!(Rule::default().is_off());
        let assigned = Rule {
            kinds: vec![RecipientKind::Assignee],
            ..Rule::default()
        };
        let unassigned = TicketPeople {
            assignee: None,
            ..people()
        };
        assert!(resolve(&assigned, &unassigned, BOB, &holders, &members).is_empty());
    }

    #[test]
    fn a_rule_names_each_recipient_once_and_what_exists() {
        let refs = References {
            roles: [MANAGER].into(),
            groups: [QUALITY].into(),
            members: [ALICE, BOB].into(),
        };
        let fine = Rule {
            kinds: vec![RecipientKind::Reporter, RecipientKind::Mentioned],
            roles: vec![MANAGER],
            groups: vec![QUALITY],
            accounts: vec![ALICE],
        };
        assert_eq!(validate_rule(Event::Commented, &fine, &refs), Ok(()));
        let errors: Vec<(String, &str)> = validate_rule(
            Event::Created,
            &Rule {
                kinds: vec![
                    RecipientKind::Reporter,
                    RecipientKind::Reporter,
                    RecipientKind::Mentioned,
                ],
                roles: vec![77],
                groups: vec![QUALITY, QUALITY],
                accounts: vec![CAROL],
            },
            &refs,
        )
        .unwrap_err()
        .into_iter()
        .map(|e| (e.field, e.code))
        .collect();
        assert_eq!(
            errors,
            [
                ("recipientKinds[1]".to_owned(), "duplicate"),
                ("recipientKinds[2]".to_owned(), "not_applicable"),
                ("roleIds[0]".to_owned(), "not_found"),
                ("groupIds[1]".to_owned(), "duplicate"),
                ("accountIds[0]".to_owned(), "unknown_member"),
            ]
        );
    }

    #[test]
    fn the_default_rules_cover_every_event_and_mention_only_where_mentions_happen() {
        let events: Vec<Event> = DEFAULT_RULES.iter().map(|r| r.event).collect();
        assert_eq!(events, Event::ALL);
        for rule in DEFAULT_RULES {
            let as_rule = Rule {
                kinds: rule.kinds.to_vec(),
                ..Rule::default()
            };
            assert_eq!(
                validate_rule(rule.event, &as_rule, &References::default()),
                Ok(()),
                "{:?}",
                rule.event
            );
            assert!(!as_rule.is_off());
        }
        let conflict = DEFAULT_RULES.iter().find(|r| r.event == Event::AbsenceConflict);
        assert_eq!(conflict.map(|r| r.roles), Some(&[PROJECT_MANAGER_ROLE][..]));
        assert!(!Event::AbsenceConflict.has_mentions());
    }
}
