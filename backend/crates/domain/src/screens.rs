//! Screens of a project configuration (specification §5, L04-US06): the
//! fields shown, in their order, when a ticket is created, viewed, or
//! moved by a transition.
//!
//! A configuration has a general creation screen and a general view
//! screen; a type may have its own, which then takes over. Transition
//! screens have a name and are attached to transitions. When a screen is
//! shown, the field rules win ([`resolve`]): a field hidden for the type
//! and the step is left out, and a field required there that the screen
//! lacks is added at the end, or the form could not be sent. A creation
//! screen is shown at the creation step, a view screen at the ticket's
//! status, a transition screen at the status the transition leads to.

use std::collections::BTreeSet;

use crate::fields::{Mode, Rules, Step, mode};

/// When a screen is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Creation,
    View,
    Transition,
}

impl Kind {
    pub const ALL: [Self; 3] = [Self::Creation, Self::View, Self::Transition];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Creation => "creation",
            Self::View => "view",
            Self::Transition => "transition",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == raw)
    }
}

/// Longest name of a screen, in characters, per language.
pub const SCREEN_NAME_MAX_CHARS: usize = 60;

/// A screen, as resolution reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    pub id: u128,
    pub kind: Kind,
    /// The type it is for; none: general (creation and view only).
    pub issue_type: Option<u128>,
    /// Its fields, in their order.
    pub fields: Vec<u128>,
}

/// The creation or view screen of a ticket of `issue_type`: its own if it
/// has one, else the general one.
pub fn pick(screens: &[Screen], kind: Kind, issue_type: u128) -> Option<&Screen> {
    screens
        .iter()
        .find(|s| s.kind == kind && s.issue_type == Some(issue_type))
        .or_else(|| screens.iter().find(|s| s.kind == kind && s.issue_type.is_none()))
}

/// A field of the configuration with its rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldRules<'a> {
    pub id: u128,
    pub rules: Rules<'a>,
}

/// A field shown on a resolved screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shown {
    pub field: u128,
    /// Visible or Required.
    pub mode: Mode,
    /// Required at this step but missing from the screen: added at the end.
    pub added: bool,
}

/// The fields a screen shows for a ticket of `issue_type` at `step`: its
/// own in their order except those hidden there, then the fields required
/// there that it lacks, in the order of the configuration's fields.
/// A field of the screen no longer in `fields` is left out.
pub fn resolve(
    screen_fields: &[u128],
    fields: &[FieldRules<'_>],
    issue_type: u128,
    step: Step,
) -> Vec<Shown> {
    let mut shown: Vec<Shown> = screen_fields
        .iter()
        .filter_map(|id| fields.iter().find(|f| f.id == *id))
        .map(|f| (f.id, mode(&f.rules, issue_type, step)))
        .filter(|(_, mode)| *mode != Mode::Hidden)
        .map(|(field, mode)| Shown {
            field,
            mode,
            added: false,
        })
        .collect();
    let on_screen: BTreeSet<u128> = screen_fields.iter().copied().collect();
    shown.extend(
        fields
            .iter()
            .filter(|f| !on_screen.contains(&f.id) && mode(&f.rules, issue_type, step) == Mode::Required)
            .map(|f| Shown {
                field: f.id,
                mode: Mode::Required,
                added: true,
            }),
    );
    shown
}

/// Why a screen cannot be removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemovalRefusal {
    /// A general creation or view screen: every configuration keeps them.
    General,
    /// These transitions use it.
    InUse(Vec<u128>),
}

/// Why `screen` cannot be removed, if it cannot; `used_by` are the
/// transitions attached to it.
pub fn removal_refusal(screen: &Screen, used_by: &[u128]) -> Option<RemovalRefusal> {
    if screen.kind != Kind::Transition && screen.issue_type.is_none() {
        Some(RemovalRefusal::General)
    } else if !used_by.is_empty() {
        Some(RemovalRefusal::InUse(used_by.to_vec()))
    } else {
        None
    }
}

/// Whether `fields` lists each field once, each a field of `known`.
pub fn fields_errors(fields: &[u128], known: &[u128]) -> Vec<(usize, &'static str)> {
    let mut seen = BTreeSet::new();
    fields
        .iter()
        .enumerate()
        .filter_map(|(i, id)| {
            if !known.contains(id) {
                Some((i, "not_found"))
            } else if !seen.insert(*id) {
                Some((i, "duplicate"))
            } else {
                None
            }
        })
        .collect()
}

/// A screen every configuration starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultScreen {
    pub kind: Kind,
    pub name_fr: &'static str,
    pub name_en: &'static str,
}

/// The general screens (`app_seed_default_screens`; an integration test
/// keeps both in step), with every field of the configuration.
pub const DEFAULT_SCREENS: [DefaultScreen; 2] = [
    DefaultScreen {
        kind: Kind::Creation,
        name_fr: "Création",
        name_en: "Creation",
    },
    DefaultScreen {
        kind: Kind::View,
        name_fr: "Consultation",
        name_en: "View",
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fields::Rule;

    const STORY: u128 = 1;
    const BUG: u128 = 2;
    const IN_PROGRESS: u128 = 10;
    const POINTS: u128 = 100;
    const APPLICATION: u128 = 101;
    const REASON: u128 = 102;
    const ENVIRONMENT: u128 = 103;

    fn screen(id: u128, kind: Kind, issue_type: Option<u128>, fields: &[u128]) -> Screen {
        Screen {
            id,
            kind,
            issue_type,
            fields: fields.to_vec(),
        }
    }

    #[test]
    fn kinds_round_trip() {
        for kind in Kind::ALL {
            assert_eq!(Kind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(Kind::parse("edit"), None);
    }

    #[test]
    fn a_type_uses_its_own_screen_else_the_general_one() {
        let screens = [
            screen(1, Kind::Creation, None, &[POINTS]),
            screen(2, Kind::View, None, &[POINTS]),
            screen(3, Kind::Creation, Some(BUG), &[APPLICATION]),
            screen(4, Kind::Transition, None, &[REASON]),
        ];
        assert_eq!(pick(&screens, Kind::Creation, BUG).map(|s| s.id), Some(3));
        assert_eq!(pick(&screens, Kind::Creation, STORY).map(|s| s.id), Some(1));
        assert_eq!(pick(&screens, Kind::View, BUG).map(|s| s.id), Some(2));
        assert_eq!(pick(&screens[2..], Kind::View, BUG), None);
    }

    #[test]
    fn rules_win_over_the_screen() {
        // Application hidden for bugs in progress; the environment required there.
        let hidden = [Rule {
            issue_type: BUG,
            step: Step::Status(IN_PROGRESS),
            mode: Mode::Hidden,
        }];
        let required = [Rule {
            issue_type: BUG,
            step: Step::Status(IN_PROGRESS),
            mode: Mode::Required,
        }];
        let fields = [
            FieldRules {
                id: POINTS,
                rules: Rules {
                    all_types: false,
                    issue_types: &[STORY],
                    rules: &[],
                },
            },
            FieldRules {
                id: APPLICATION,
                rules: Rules {
                    all_types: true,
                    issue_types: &[],
                    rules: &hidden,
                },
            },
            FieldRules {
                id: ENVIRONMENT,
                rules: Rules {
                    all_types: true,
                    issue_types: &[],
                    rules: &required,
                },
            },
            FieldRules {
                id: REASON,
                rules: Rules {
                    all_types: true,
                    issue_types: &[],
                    rules: &[],
                },
            },
        ];
        let on_screen = [APPLICATION, POINTS, REASON, 999];
        let shown = |issue_type, step| resolve(&on_screen, &fields, issue_type, step);
        let visible = |field| Shown {
            field,
            mode: Mode::Visible,
            added: false,
        };
        // A story at creation: as the screen says, the unknown field left out.
        assert_eq!(
            shown(STORY, Step::Creation),
            [visible(APPLICATION), visible(POINTS), visible(REASON)]
        );
        // A bug in progress: Points is not a bug's, Application is hidden,
        // the environment required and added at the end.
        assert_eq!(
            shown(BUG, Step::Status(IN_PROGRESS)),
            [
                visible(REASON),
                Shown {
                    field: ENVIRONMENT,
                    mode: Mode::Required,
                    added: true
                }
            ]
        );
        // Already on the screen: required in place, not added again.
        let with_environment = [ENVIRONMENT, REASON];
        assert_eq!(
            resolve(&with_environment, &fields, BUG, Step::Status(IN_PROGRESS)),
            [
                Shown {
                    field: ENVIRONMENT,
                    mode: Mode::Required,
                    added: false
                },
                visible(REASON)
            ]
        );
    }

    #[test]
    fn general_screens_and_screens_in_use_stay() {
        let general = screen(1, Kind::Creation, None, &[]);
        assert_eq!(removal_refusal(&general, &[]), Some(RemovalRefusal::General));
        let for_bug = screen(2, Kind::View, Some(BUG), &[]);
        assert_eq!(removal_refusal(&for_bug, &[]), None);
        let rejection = screen(3, Kind::Transition, None, &[REASON]);
        assert_eq!(
            removal_refusal(&rejection, &[7, 8]),
            Some(RemovalRefusal::InUse(vec![7, 8]))
        );
        assert_eq!(removal_refusal(&rejection, &[]), None);
    }

    #[test]
    fn a_screen_lists_fields_of_the_configuration_once() {
        assert!(fields_errors(&[POINTS, REASON], &[POINTS, REASON, APPLICATION]).is_empty());
        assert_eq!(
            fields_errors(&[POINTS, 999, POINTS], &[POINTS]),
            [(1, "not_found"), (2, "duplicate")]
        );
    }

    #[test]
    fn the_default_screens_are_creation_and_view() {
        let kinds: Vec<Kind> = DEFAULT_SCREENS.iter().map(|s| s.kind).collect();
        assert_eq!(kinds, [Kind::Creation, Kind::View]);
    }
}
