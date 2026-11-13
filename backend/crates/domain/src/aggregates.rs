//! Aggregates of the hierarchy (specification §4, L03-US05).
//!
//! A node aggregates the figures of its distinct descendant projects: a
//! project reached through several paths counts once. Each project brings
//! its own [`Figures`]; tickets (L05) and workload (L08) feed them.
//!
//! - The weighting of a link applies to the aggregates of that link's parent
//!   only: higher up, the project counts whole, once ([`share_percent`]).
//! - It weights amounts (planned load, capacity). Tickets are counted whole:
//!   a ticket is open or it is not.
//! - Archived projects leave the aggregates of their ancestors.
//! - Confidential projects are counted like the others; how many they are
//!   and their open tickets are also kept apart, to tell what is anonymous.
//!
//! Everything here is a pure function over the links and the projects of a
//! tenant; the infrastructure only loads them.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::grants::Id;

/// What is measured on a project, or summed over the projects of a node.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Figures {
    /// Planned load.
    pub planned_minutes: u64,
    /// Capacity of the teams assigned.
    pub capacity_minutes: u64,
    /// Tickets by status category.
    pub todo: u64,
    pub in_progress: u64,
    pub done: u64,
    /// Open tickets past their due date.
    pub late: u64,
}

impl Figures {
    /// Tickets not done yet.
    pub const fn open(&self) -> u64 {
        self.todo + self.in_progress
    }

    /// Progress by status category: the share of tickets done, in percent
    /// rounded down (100 only when everything is done). `None` without
    /// tickets.
    pub fn progress_percent(&self) -> Option<u8> {
        let tickets = u128::from(self.open()) + u128::from(self.done);
        if tickets == 0 {
            return None;
        }
        u8::try_from(u128::from(self.done) * 100 / tickets).ok()
    }

    /// Consumed capacity: planned load over capacity, in percent rounded to
    /// the nearest (above 100 when overbooked). `None` without capacity.
    pub fn capacity_used_percent(&self) -> Option<u32> {
        if self.capacity_minutes == 0 {
            return None;
        }
        let capacity = u128::from(self.capacity_minutes);
        let percent = (u128::from(self.planned_minutes) * 100 + capacity / 2) / capacity;
        Some(u32::try_from(percent).unwrap_or(u32::MAX))
    }
}

/// A project, as the aggregates need it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Project {
    pub figures: Figures,
    /// Out of its ancestors' aggregates.
    pub archived: bool,
    /// Counted without name or detail.
    pub confidential: bool,
}

/// A parent-child link with its weighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    pub parent: Id,
    pub child: Id,
    /// 0 to 100.
    pub weight_percent: u8,
}

/// What a node aggregates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Aggregate {
    /// Distinct projects counted: the node itself if it is a project, and
    /// its descendant projects that are not archived.
    pub projects: u32,
    /// Those of them that are confidential.
    pub confidential_projects: u32,
    /// Their figures: amounts weighted, tickets whole.
    pub figures: Figures,
    /// Open tickets of the confidential ones.
    pub confidential_open: u64,
}

/// Share of a descendant project in the aggregates of a node, in percent:
/// the weighting of their direct link, unless a longer path also leads to
/// the project, where it counts whole. Never above 100.
pub const fn share_percent(direct_weight: Option<u8>, reached_deeper: bool) -> u8 {
    match direct_weight {
        Some(weight) if !reached_deeper => {
            if weight > 100 {
                100
            } else {
                weight
            }
        }
        _ => 100,
    }
}

/// The links and projects of a tenant.
#[derive(Debug, Clone, Default)]
pub struct Hierarchy {
    children: HashMap<Id, Vec<(Id, u8)>>,
    parents: HashMap<Id, Vec<Id>>,
    projects: HashMap<Id, Project>,
}

impl Hierarchy {
    /// `projects`: every project by id; nodes missing from it are portfolios
    /// or programs, which bring no figures of their own.
    pub fn new(projects: impl IntoIterator<Item = (Id, Project)>, links: &[Link]) -> Self {
        let mut hierarchy = Self {
            projects: projects.into_iter().collect(),
            ..Self::default()
        };
        for link in links {
            hierarchy
                .children
                .entry(link.parent)
                .or_default()
                .push((link.child, link.weight_percent));
            hierarchy.parents.entry(link.child).or_default().push(link.parent);
        }
        hierarchy
    }

    /// Every distinct descendant of `node` with its share in the node's
    /// aggregates ([`share_percent`]).
    pub fn shares(&self, node: Id) -> HashMap<Id, u8> {
        let direct = self.children.get(&node).map_or(&[][..], Vec::as_slice);
        // What lies under the children: reached by a path of two links or
        // more.
        let mut deeper = HashSet::new();
        let mut pending: Vec<Id> = direct.iter().map(|(child, _)| *child).collect();
        let mut walked = HashSet::new();
        while let Some(current) = pending.pop() {
            if !walked.insert(current) {
                continue;
            }
            for (child, _) in self.children.get(&current).map_or(&[][..], Vec::as_slice) {
                deeper.insert(*child);
                pending.push(*child);
            }
        }
        let mut shares: HashMap<Id, u8> = deeper.iter().map(|id| (*id, 100)).collect();
        for (child, weight) in direct {
            let share = share_percent(Some(*weight), deeper.contains(child));
            // Two links to the same child cannot exist; the larger share
            // would win.
            let entry = shares.entry(*child).or_insert(share);
            *entry = (*entry).max(share);
        }
        shares
    }

    /// The aggregate of `node`.
    pub fn aggregate(&self, node: Id) -> Aggregate {
        let mut sum = Sum::default();
        // A project counts itself whole, archived or not: it is its
        // ancestors' aggregates an archived project leaves.
        if let Some(own) = self.projects.get(&node) {
            sum.add(own, 100);
        }
        for (descendant, share) in self.shares(node) {
            if let Some(project) = self.projects.get(&descendant).filter(|p| !p.archived) {
                sum.add(project, share);
            }
        }
        sum.into_aggregate()
    }

    /// The nodes whose aggregate a change to `changed` may alter: these
    /// nodes and all their ancestors.
    pub fn affected(&self, changed: &[Id]) -> HashSet<Id> {
        let mut affected: HashSet<Id> = HashSet::new();
        let mut pending = changed.to_vec();
        while let Some(current) = pending.pop() {
            if affected.insert(current) {
                pending.extend(self.parents.get(&current).into_iter().flatten().copied());
            }
        }
        affected
    }
}

/// Running sum of an aggregate; amounts in hundredths until the end, so
/// that weighted minutes are rounded once.
#[derive(Default)]
struct Sum {
    aggregate: Aggregate,
    planned_hundredths: u128,
    capacity_hundredths: u128,
}

impl Sum {
    fn add(&mut self, project: &Project, share: u8) {
        let figures = &project.figures;
        let share = u128::from(share.min(100));
        self.planned_hundredths += u128::from(figures.planned_minutes) * share;
        self.capacity_hundredths += u128::from(figures.capacity_minutes) * share;
        let total = &mut self.aggregate;
        total.projects += 1;
        total.figures.todo += figures.todo;
        total.figures.in_progress += figures.in_progress;
        total.figures.done += figures.done;
        total.figures.late += figures.late;
        if project.confidential {
            total.confidential_projects += 1;
            total.confidential_open += figures.open();
        }
    }

    fn into_aggregate(self) -> Aggregate {
        let minutes = |hundredths: u128| u64::try_from((hundredths + 50) / 100).unwrap_or(u64::MAX);
        let mut aggregate = self.aggregate;
        aggregate.figures.planned_minutes = minutes(self.planned_hundredths);
        aggregate.figures.capacity_minutes = minutes(self.capacity_hundredths);
        aggregate
    }
}

/// How long after a change its aggregates are recomputed: changes made in
/// the meantime are taken by the same recompute.
pub const RECOMPUTE_DELAY: Duration = Duration::from_secs(2);

/// After a change: in how long to recompute, given the recompute already
/// queued `pending` from now, if any. `None`: that one comes soon enough
/// and takes the change with it.
pub fn recompute_in(pending: Option<Duration>) -> Option<Duration> {
    match pending {
        Some(queued) if queued <= RECOMPUTE_DELAY => None,
        _ => Some(RECOMPUTE_DELAY),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PORTFOLIO: Id = 1;
    const PROGRAM_A: Id = 2;
    const PROGRAM_B: Id = 3;
    const PLATFORM: Id = 10;
    const PORTAL: Id = 11;
    const WAREHOUSE: Id = 12;
    const ERP: Id = 13;

    fn project(planned_minutes: u64, todo: u64, done: u64, late: u64) -> Project {
        Project {
            figures: Figures {
                planned_minutes,
                capacity_minutes: planned_minutes * 2,
                todo,
                in_progress: 0,
                done,
                late,
            },
            archived: false,
            confidential: false,
        }
    }

    fn link(parent: Id, child: Id, weight_percent: u8) -> Link {
        Link {
            parent,
            child,
            weight_percent,
        }
    }

    /// The portfolio of mockup 05: the platform project is under both
    /// programs, the ERP flow is a sub-project of the warehouse.
    fn projects() -> Vec<(Id, Project)> {
        vec![
            (PLATFORM, project(600, 42, 58, 1)),
            (PORTAL, project(900, 71, 29, 3)),
            (WAREHOUSE, project(500, 37, 13, 3)),
            (ERP, project(300, 18, 2, 4)),
        ]
    }

    fn links(platform_under_a: u8, platform_under_b: u8) -> Vec<Link> {
        vec![
            link(PORTFOLIO, PROGRAM_A, 100),
            link(PORTFOLIO, PROGRAM_B, 100),
            link(PROGRAM_A, PLATFORM, platform_under_a),
            link(PROGRAM_B, PLATFORM, platform_under_b),
            link(PROGRAM_A, PORTAL, 100),
            link(PROGRAM_A, WAREHOUSE, 100),
            link(WAREHOUSE, ERP, 100),
        ]
    }

    #[test]
    fn a_project_reached_through_two_programs_counts_once_at_the_portfolio() {
        let hierarchy = Hierarchy::new(projects(), &links(100, 100));

        let portfolio = hierarchy.aggregate(PORTFOLIO);
        assert_eq!(portfolio.projects, 4);
        assert_eq!(portfolio.figures.planned_minutes, 600 + 900 + 500 + 300);
        assert_eq!(portfolio.figures.open(), 42 + 71 + 37 + 18);
        assert_eq!(portfolio.figures.late, 1 + 3 + 3 + 4);

        // Each program counts it too.
        assert_eq!(
            hierarchy.aggregate(PROGRAM_A).figures.planned_minutes,
            600 + 900 + 500 + 300
        );
        assert_eq!(hierarchy.aggregate(PROGRAM_B).figures.planned_minutes, 600);
        assert_eq!(hierarchy.aggregate(PROGRAM_B).projects, 1);
    }

    #[test]
    fn each_parent_applies_its_own_weighting_and_those_above_count_the_project_whole() {
        let hierarchy = Hierarchy::new(projects(), &links(60, 25));

        let a = hierarchy.aggregate(PROGRAM_A);
        assert_eq!(a.figures.planned_minutes, 360 + 900 + 500 + 300);
        assert_eq!(a.figures.capacity_minutes, 720 + 1800 + 1000 + 600);
        let b = hierarchy.aggregate(PROGRAM_B);
        assert_eq!(b.figures.planned_minutes, 150);

        // Not 60 % + 25 %: the portfolio is no direct parent.
        let portfolio = hierarchy.aggregate(PORTFOLIO);
        assert_eq!(portfolio.figures.planned_minutes, 600 + 900 + 500 + 300);
        assert_eq!(portfolio.projects, 4);
    }

    #[test]
    fn the_weighting_leaves_tickets_whole() {
        let hierarchy = Hierarchy::new(projects(), &links(100, 25));
        let b = hierarchy.aggregate(PROGRAM_B);
        assert_eq!(b.figures.planned_minutes, 150);
        assert_eq!((b.figures.todo, b.figures.done, b.figures.late), (42, 58, 1));
        assert_eq!(b.projects, 1);

        // Even at 0 %: the project is there, its load is not.
        let hierarchy = Hierarchy::new(projects(), &links(100, 0));
        let b = hierarchy.aggregate(PROGRAM_B);
        assert_eq!(
            (b.projects, b.figures.planned_minutes, b.figures.open()),
            (1, 0, 42)
        );
    }

    #[test]
    fn a_direct_link_also_reached_deeper_counts_whole() {
        let mut links = links(100, 100);
        links.push(link(PORTFOLIO, PLATFORM, 30));
        let hierarchy = Hierarchy::new(projects(), &links);
        assert_eq!(hierarchy.shares(PORTFOLIO)[&PLATFORM], 100);
        assert_eq!(
            hierarchy.aggregate(PORTFOLIO).figures.planned_minutes,
            600 + 900 + 500 + 300
        );

        assert_eq!(share_percent(Some(30), false), 30);
        assert_eq!(share_percent(Some(30), true), 100);
        assert_eq!(share_percent(None, true), 100);
        assert_eq!(share_percent(Some(250), false), 100, "never above 100 %");
    }

    #[test]
    fn a_project_aggregates_itself_and_its_sub_projects() {
        let hierarchy = Hierarchy::new(projects(), &links(100, 100));
        let warehouse = hierarchy.aggregate(WAREHOUSE);
        assert_eq!(warehouse.projects, 2);
        assert_eq!(warehouse.figures.planned_minutes, 800);
        assert_eq!(warehouse.figures.late, 7);

        let leaf = hierarchy.aggregate(ERP);
        assert_eq!((leaf.projects, leaf.figures.planned_minutes), (1, 300));

        // The weighting of a sub-project applies to its parent project.
        let mut links = links(100, 100);
        links.retain(|l| l.child != ERP);
        links.push(link(WAREHOUSE, ERP, 50));
        let hierarchy = Hierarchy::new(projects(), &links);
        assert_eq!(hierarchy.aggregate(WAREHOUSE).figures.planned_minutes, 650);
        assert_eq!(hierarchy.aggregate(PROGRAM_A).figures.planned_minutes, 2300);
    }

    #[test]
    fn archived_projects_leave_the_aggregates_of_their_ancestors() {
        let mut projects = projects();
        projects[2].1.archived = true; // the warehouse
        let hierarchy = Hierarchy::new(projects, &links(100, 100));

        let a = hierarchy.aggregate(PROGRAM_A);
        assert_eq!(a.projects, 3, "its sub-project stays");
        assert_eq!(a.figures.planned_minutes, 600 + 900 + 300);
        assert_eq!(hierarchy.aggregate(PORTFOLIO).figures.late, 1 + 3 + 4);

        // Opened by itself, it still shows its own figures.
        let own = hierarchy.aggregate(WAREHOUSE);
        assert_eq!((own.projects, own.figures.planned_minutes), (2, 800));
    }

    #[test]
    fn confidential_projects_are_counted_and_told_apart() {
        let mut projects = projects();
        projects[1].1.confidential = true; // the portal
        let hierarchy = Hierarchy::new(projects, &links(100, 100));

        let portfolio = hierarchy.aggregate(PORTFOLIO);
        assert_eq!(portfolio.projects, 4);
        assert_eq!(portfolio.figures.planned_minutes, 2300);
        assert_eq!(portfolio.confidential_projects, 1);
        assert_eq!(portfolio.confidential_open, 71);

        let b = hierarchy.aggregate(PROGRAM_B);
        assert_eq!((b.confidential_projects, b.confidential_open), (0, 0));
    }

    #[test]
    fn nodes_without_projects_aggregate_nothing() {
        let hierarchy = Hierarchy::new([], &[link(PORTFOLIO, PROGRAM_A, 100)]);
        assert_eq!(hierarchy.aggregate(PORTFOLIO), Aggregate::default());
        assert_eq!(hierarchy.aggregate(99), Aggregate::default());
    }

    #[test]
    fn weighted_minutes_are_rounded_once() {
        // Three projects of 1 minute at 50 %: 1.5 minutes, not 3 × 0.5 → 0 or 3.
        let projects = (10..13).map(|id| (id, project(1, 0, 0, 0)));
        let links: Vec<Link> = (10..13).map(|id| link(PROGRAM_A, id, 50)).collect();
        let hierarchy = Hierarchy::new(projects, &links);
        assert_eq!(hierarchy.aggregate(PROGRAM_A).figures.planned_minutes, 2);
    }

    #[test]
    fn progress_is_the_share_of_tickets_done() {
        let figures = |todo, in_progress, done| Figures {
            todo,
            in_progress,
            done,
            ..Figures::default()
        };
        assert_eq!(figures(0, 0, 0).progress_percent(), None);
        assert_eq!(figures(27, 27, 46).progress_percent(), Some(46));
        assert_eq!(figures(0, 1, 999).progress_percent(), Some(99), "not done yet");
        assert_eq!(figures(0, 0, 3).progress_percent(), Some(100));
        assert_eq!(figures(5, 0, 0).progress_percent(), Some(0));
        assert_eq!(figures(2, 3, 1).open(), 5);
    }

    #[test]
    fn consumed_capacity_is_the_load_over_the_capacity() {
        let figures = |planned_minutes, capacity_minutes| Figures {
            planned_minutes,
            capacity_minutes,
            ..Figures::default()
        };
        assert_eq!(figures(100, 0).capacity_used_percent(), None);
        assert_eq!(figures(860, 1000).capacity_used_percent(), Some(86));
        assert_eq!(figures(1, 3).capacity_used_percent(), Some(33));
        assert_eq!(figures(2, 3).capacity_used_percent(), Some(67));
        assert_eq!(
            figures(1500, 1000).capacity_used_percent(),
            Some(150),
            "overbooked"
        );
    }

    #[test]
    fn a_change_affects_the_node_and_all_its_ancestors() {
        let hierarchy = Hierarchy::new(projects(), &links(100, 100));
        let set = |ids: &[Id]| ids.iter().copied().collect::<HashSet<Id>>();

        assert_eq!(
            hierarchy.affected(&[ERP]),
            set(&[ERP, WAREHOUSE, PROGRAM_A, PORTFOLIO])
        );
        assert_eq!(
            hierarchy.affected(&[PLATFORM]),
            set(&[PLATFORM, PROGRAM_A, PROGRAM_B, PORTFOLIO]),
            "through both parents"
        );
        assert_eq!(hierarchy.affected(&[PORTFOLIO]), set(&[PORTFOLIO]));
        assert_eq!(
            hierarchy.affected(&[PORTAL, PROGRAM_B, 99]),
            set(&[PORTAL, PROGRAM_A, PROGRAM_B, PORTFOLIO, 99]),
            "unknown nodes stand alone"
        );
        assert!(hierarchy.affected(&[]).is_empty());
    }

    #[test]
    fn changes_in_a_burst_share_one_recompute() {
        // Nothing queued: recompute after the delay.
        assert_eq!(recompute_in(None), Some(RECOMPUTE_DELAY));
        // One is coming within the delay: it takes this change too.
        assert_eq!(recompute_in(Some(Duration::ZERO)), None);
        assert_eq!(recompute_in(Some(Duration::from_millis(1500))), None);
        assert_eq!(recompute_in(Some(RECOMPUTE_DELAY)), None);
        // The queued one is a retry far away: do not wait for it.
        assert_eq!(
            recompute_in(Some(Duration::from_secs(600))),
            Some(RECOMPUTE_DELAY)
        );
        assert!(RECOMPUTE_DELAY <= Duration::from_secs(5), "within seconds");
    }

    /// A small deterministic generator (xorshift): the properties below run
    /// on the same graphs at every run.
    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, bound: u64) -> u64 {
            self.next() % bound
        }
    }

    /// An acyclic graph of `size` nodes (links go from a lower id to a
    /// higher one), about half of them projects, with random weightings.
    fn random_case(random: &mut Random, size: u64) -> (Vec<(Id, Project)>, Vec<Link>) {
        let mut projects = Vec::new();
        let mut links = Vec::new();
        for id in 0..size {
            if random.below(2) == 0 {
                let mut p = project(random.below(5000), random.below(50), random.below(50), 0);
                p.figures.in_progress = random.below(20);
                p.figures.late = random.below(p.figures.open() + 1);
                p.archived = random.below(6) == 0;
                p.confidential = random.below(5) == 0;
                projects.push((Id::from(id), p));
            }
            for parent in 0..id {
                if random.below(4) == 0 {
                    let weight = u8::try_from(random.below(101)).unwrap();
                    links.push(link(Id::from(parent), Id::from(id), weight));
                }
            }
        }
        (projects, links)
    }

    /// Every node `to` is reached from, by one link or more.
    fn descendants(from: Id, links: &[Link]) -> HashSet<Id> {
        let mut found = HashSet::new();
        let mut pending = vec![from];
        while let Some(current) = pending.pop() {
            for l in links.iter().filter(|l| l.parent == current) {
                if found.insert(l.child) {
                    pending.push(l.child);
                }
            }
        }
        found
    }

    #[test]
    fn property_a_project_never_contributes_more_than_once_nor_more_than_whole() {
        let mut random = Random(0x9E37_79B9_7F4A_7C15);
        for round in 0..300 {
            let size = 2 + random.below(12);
            let (projects, links) = random_case(&mut random, size);
            let hierarchy = Hierarchy::new(projects.clone(), &links);
            for node in 0..size {
                let node = Id::from(node);
                let below = descendants(node, &links);
                // What counts, each once and whole.
                let counted: Vec<&Project> = projects
                    .iter()
                    .filter(|(id, p)| *id == node || (below.contains(id) && !p.archived))
                    .map(|(_, p)| p)
                    .collect();
                let aggregate = hierarchy.aggregate(node);
                let case = format!("round {round}, node {node}");

                assert_eq!(aggregate.projects as usize, counted.len(), "{case}");
                let whole = |pick: fn(&Figures) -> u64| counted.iter().map(|p| pick(&p.figures)).sum::<u64>();
                assert_eq!(aggregate.figures.todo, whole(|f| f.todo), "{case}");
                assert_eq!(aggregate.figures.in_progress, whole(|f| f.in_progress), "{case}");
                assert_eq!(aggregate.figures.done, whole(|f| f.done), "{case}");
                assert_eq!(aggregate.figures.late, whole(|f| f.late), "{case}");
                assert!(
                    aggregate.figures.planned_minutes <= whole(|f| f.planned_minutes),
                    "{case}: more than 100 % of a project"
                );
                assert!(
                    aggregate.figures.capacity_minutes <= whole(|f| f.capacity_minutes),
                    "{case}"
                );
                assert!(aggregate.confidential_projects <= aggregate.projects, "{case}");
                assert!(aggregate.confidential_open <= aggregate.figures.open(), "{case}");

                for share in hierarchy.shares(node).values() {
                    assert!(*share <= 100, "{case}");
                }
            }
        }
    }

    #[test]
    fn property_a_single_project_never_weighs_more_than_itself_anywhere() {
        let mut random = Random(0x2545_F491_4F6C_DD1D);
        for round in 0..300 {
            let size = 3 + random.below(10);
            let (_, links) = random_case(&mut random, size);
            // Only the last node is a project.
            let only = Id::from(size - 1);
            let hierarchy = Hierarchy::new([(only, project(1000, 10, 5, 2))], &links);
            for node in 0..size {
                let aggregate = hierarchy.aggregate(Id::from(node));
                assert!(aggregate.projects <= 1, "round {round}");
                assert!(aggregate.figures.planned_minutes <= 1000, "round {round}");
                assert!(aggregate.figures.open() <= 10, "round {round}");
            }
        }
    }

    #[test]
    fn property_a_change_reaches_exactly_the_nodes_above_it() {
        let mut random = Random(0xD6E8_FEB8_6659_FD93);
        for round in 0..200 {
            let size = 2 + random.below(12);
            let (projects, links) = random_case(&mut random, size);
            let hierarchy = Hierarchy::new(projects, &links);
            let changed = Id::from(random.below(size));
            let affected = hierarchy.affected(&[changed]);
            for node in 0..size {
                let node = Id::from(node);
                let above = node == changed || descendants(node, &links).contains(&changed);
                assert_eq!(affected.contains(&node), above, "round {round}, node {node}");
            }
        }
    }
}
