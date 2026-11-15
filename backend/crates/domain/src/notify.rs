//! Notifying members (specification §12, L05-US07): which ticket changes
//! are which events, who gets one notification per change, through which
//! channels their preferences allow, and when the daily digest goes.
//!
//! The rules of the project's configuration (`crate::notifications`) say
//! whom an event notifies. Here a change becomes its events
//! ([`events_of`]); each recipient gets one notification for it, the one
//! that concerns them most ([`one_per_recipient`]), in the application or
//! by email as their preferences say ([`Preferences`]). Preferences belong
//! to the account and apply in every organization.

use std::collections::{BTreeMap, BTreeSet};

use crate::notifications::Event;

/// Where a notification goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Channel {
    /// The notification center of the application.
    InApp,
    Email,
}

/// A change of a ticket, as the outbox tells it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Change {
    /// The ticket was created.
    pub created: bool,
    /// The assignee changed to someone (not cleared).
    pub assigned_to: Option<u128>,
    /// The status changed.
    pub status_changed: bool,
    /// A comment was written.
    pub commented: bool,
    /// The people a comment newly mentions (written or edited).
    pub mentioned: BTreeSet<u128>,
}

/// The events of `change`, most specific first.
pub fn events_of(change: &Change) -> Vec<Event> {
    let mut events = Vec::new();
    if !change.mentioned.is_empty() {
        events.push(Event::Mentioned);
    }
    if change.assigned_to.is_some() && !change.created {
        events.push(Event::Assigned);
    }
    if change.status_changed {
        events.push(Event::StatusChanged);
    }
    if change.commented {
        events.push(Event::Commented);
    }
    if change.created {
        events.push(Event::Created);
    }
    events
}

/// How much an event concerns its recipient: one notification per change
/// and person, the most specific (a mention before a comment).
const fn priority(event: Event) -> u8 {
    match event {
        Event::Mentioned => 0,
        Event::Assigned => 1,
        Event::Created => 2,
        Event::StatusChanged => 3,
        Event::Commented => 4,
    }
}

/// For each recipient of `events` (each with its recipients), the one
/// event they are notified of.
pub fn one_per_recipient(events: &[(Event, BTreeSet<u128>)]) -> BTreeMap<u128, Event> {
    let mut chosen: BTreeMap<u128, Event> = BTreeMap::new();
    for (event, recipients) in events {
        for recipient in recipients {
            chosen
                .entry(*recipient)
                .and_modify(|current| {
                    if priority(*event) < priority(*current) {
                        *current = *event;
                    }
                })
                .or_insert(*event);
        }
    }
    chosen
}

/// Whether an event reaches a member in the application and by email.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Choice {
    pub in_app: bool,
    pub email: bool,
}

/// A member's preferences: per event and channel, and the daily digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preferences {
    choices: BTreeMap<Event, Choice>,
    /// One email each working day instead of one per notification.
    pub daily_digest: bool,
}

impl Default for Preferences {
    /// What a member gets until they choose (mockup 16): everything in the
    /// application; by email, what is addressed to them (mentions,
    /// assignments, new tickets), not status changes nor comments; no
    /// digest.
    fn default() -> Self {
        let choices = Event::ALL
            .into_iter()
            .map(|event| {
                let email = matches!(event, Event::Mentioned | Event::Assigned | Event::Created);
                (event, Choice { in_app: true, email })
            })
            .collect();
        Self {
            choices,
            daily_digest: false,
        }
    }
}

impl Preferences {
    /// The defaults, with `chosen` instead where the member chose.
    pub fn with(chosen: impl IntoIterator<Item = (Event, Choice)>, daily_digest: bool) -> Self {
        let mut preferences = Self::default();
        preferences.choices.extend(chosen);
        preferences.daily_digest = daily_digest;
        preferences
    }

    pub fn choice(&self, event: Event) -> Choice {
        self.choices.get(&event).copied().unwrap_or(Choice {
            in_app: true,
            email: false,
        })
    }

    /// Every event with its choice, in the order of the events.
    pub fn choices(&self) -> Vec<(Event, Choice)> {
        Event::ALL
            .into_iter()
            .map(|event| (event, self.choice(event)))
            .collect()
    }

    /// Whether `event` reaches the member through `channel`.
    pub fn wants(&self, event: Event, channel: Channel) -> bool {
        let choice = self.choice(event);
        match channel {
            Channel::InApp => choice.in_app,
            Channel::Email => choice.email,
        }
    }

    /// Whether a notification of `event` is emailed on its own, now: by
    /// email, and no digest gathers it.
    pub fn emails_now(&self, event: Event) -> bool {
        self.wants(event, Channel::Email) && !self.daily_digest
    }

    /// Whether a notification of `event` is recorded: shown in the
    /// application, or kept for the digest.
    pub fn records(&self, event: Event) -> bool {
        self.wants(event, Channel::InApp) || self.wants(event, Channel::Email)
    }
}

/// The hour of the member's day the digest goes at.
pub const DIGEST_HOUR: u8 = 8;

/// Whether the digest is due for a member whose local time is `weekday`
/// (1 Monday … 7 Sunday) at `hour`, and who has `sent_today` or not: once
/// each working day, from [`DIGEST_HOUR`].
pub const fn digest_due(weekday: u8, hour: u8, sent_today: bool) -> bool {
    weekday >= 1 && weekday <= 5 && hour >= DIGEST_HOUR && !sent_today
}

#[cfg(test)]
mod tests {
    use super::*;

    const LUCAS: u128 = 1;
    const CAMILLE: u128 = 2;
    const HUGO: u128 = 3;

    #[test]
    fn a_change_is_the_events_it_holds() {
        assert_eq!(
            events_of(&Change {
                created: true,
                assigned_to: Some(LUCAS),
                ..Change::default()
            }),
            [Event::Created],
            "assigned at creation: the creation tells it"
        );
        assert_eq!(
            events_of(&Change {
                assigned_to: Some(LUCAS),
                status_changed: true,
                ..Change::default()
            }),
            [Event::Assigned, Event::StatusChanged]
        );
        assert_eq!(
            events_of(&Change {
                commented: true,
                mentioned: [CAMILLE].into(),
                ..Change::default()
            }),
            [Event::Mentioned, Event::Commented]
        );
        assert!(events_of(&Change::default()).is_empty());
    }

    #[test]
    fn each_recipient_gets_the_event_that_concerns_them_most() {
        let chosen = one_per_recipient(&[
            (Event::Commented, [LUCAS, CAMILLE, HUGO].into()),
            (Event::Mentioned, [CAMILLE].into()),
            (Event::StatusChanged, [HUGO].into()),
        ]);
        assert_eq!(
            chosen,
            [
                (LUCAS, Event::Commented),
                (CAMILLE, Event::Mentioned),
                (HUGO, Event::StatusChanged)
            ]
            .into()
        );
    }

    #[test]
    fn defaults_follow_the_mockup_and_choices_replace_them() {
        let defaults = Preferences::default();
        for event in Event::ALL {
            assert!(defaults.wants(event, Channel::InApp));
        }
        assert!(defaults.wants(Event::Mentioned, Channel::Email));
        assert!(defaults.wants(Event::Assigned, Channel::Email));
        assert!(defaults.wants(Event::Created, Channel::Email));
        assert!(!defaults.wants(Event::StatusChanged, Channel::Email));
        assert!(!defaults.wants(Event::Commented, Channel::Email));
        assert!(!defaults.daily_digest);

        let mine = Preferences::with(
            [
                (
                    Event::StatusChanged,
                    Choice {
                        in_app: true,
                        email: false,
                    },
                ),
                (
                    Event::Commented,
                    Choice {
                        in_app: false,
                        email: false,
                    },
                ),
            ],
            false,
        );
        assert!(!mine.records(Event::Commented));
        assert!(
            mine.emails_now(Event::Mentioned),
            "untouched events keep their default"
        );
        assert_eq!(mine.choices().len(), Event::ALL.len());
    }

    #[test]
    fn the_digest_gathers_the_emails() {
        let digest = Preferences::with([], true);
        assert!(!digest.emails_now(Event::Mentioned));
        assert!(digest.records(Event::Mentioned));
        let email_only = Preferences::with(
            [(
                Event::Commented,
                Choice {
                    in_app: false,
                    email: true,
                },
            )],
            true,
        );
        assert!(email_only.records(Event::Commented), "kept for the digest");
    }

    #[test]
    fn the_digest_goes_once_each_working_day_from_eight() {
        assert!(digest_due(1, 8, false));
        assert!(digest_due(5, 17, false));
        assert!(!digest_due(1, 7, false));
        assert!(!digest_due(3, 9, true), "sent today already");
        assert!(!digest_due(6, 9, false), "Saturday");
        assert!(!digest_due(7, 9, false), "Sunday");
    }
}
