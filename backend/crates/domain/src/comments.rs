//! Comments of a ticket (specification §12, L05-US04): who sees, edits and
//! deletes them.
//!
//! A comment is internal (the team) or public (also shown on the portal,
//! L13); internal by default. Its author edits it and deletes it; those who
//! administer the project delete anyone's, for moderation, never edit it. A
//! deleted comment stays as a placeholder, without its content.

/// Who sees a comment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Visibility {
    /// The members who see the ticket.
    #[default]
    Internal,
    /// Also the requester, on the portal.
    Public,
}

impl Visibility {
    pub const ALL: [Self; 2] = [Self::Internal, Self::Public];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::Public => "public",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == raw)
    }

    /// Whether the portal (L13) may return it: public comments only.
    pub const fn on_portal(self) -> bool {
        matches!(self, Self::Public)
    }
}

/// What the member acting may do with a comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Acting {
    pub account: u128,
    /// Holds `comment` on the ticket's project.
    pub comments: bool,
    /// Administers the project (or the tenant): moderation.
    pub moderates: bool,
}

/// Whether `acting` may change the comment written by `author` (none: an
/// erased account, or a transition's comment of one): its author, while they
/// still comment there; never a deleted one.
pub fn can_edit(author: Option<u128>, deleted: bool, acting: &Acting) -> bool {
    !deleted && acting.comments && author == Some(acting.account)
}

/// Whether `acting` may delete the comment written by `author`: its author
/// while they still comment there, or a moderator; never twice.
pub fn can_delete(author: Option<u128>, deleted: bool, acting: &Acting) -> bool {
    !deleted && (acting.moderates || can_edit(author, deleted, acting))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: u128 = 1;
    const OTHER: u128 = 2;

    fn acting(comments: bool, moderates: bool) -> Acting {
        Acting {
            account: ME,
            comments,
            moderates,
        }
    }

    #[test]
    fn visibilities_round_trip_and_only_public_ones_reach_the_portal() {
        for visibility in Visibility::ALL {
            assert_eq!(Visibility::parse(visibility.as_str()), Some(visibility));
        }
        assert_eq!(Visibility::default(), Visibility::Internal);
        assert!(Visibility::Public.on_portal());
        assert!(!Visibility::Internal.on_portal());
        assert_eq!(Visibility::parse("private"), None);
    }

    #[test]
    fn the_author_edits_and_deletes_moderators_delete_only() {
        let member = acting(true, false);
        assert!(can_edit(Some(ME), false, &member));
        assert!(can_delete(Some(ME), false, &member));
        assert!(!can_edit(Some(OTHER), false, &member));
        assert!(!can_delete(Some(OTHER), false, &member));
        // No longer commenting there: neither.
        assert!(!can_edit(Some(ME), false, &acting(false, false)));
        // A moderator deletes anyone's, edits only their own.
        let moderator = acting(true, true);
        assert!(can_delete(Some(OTHER), false, &moderator));
        assert!(can_delete(None, false, &moderator));
        assert!(!can_edit(Some(OTHER), false, &moderator));
        // Deleted: nothing more.
        assert!(!can_edit(Some(ME), true, &member));
        assert!(!can_delete(Some(ME), true, &moderator));
    }
}
