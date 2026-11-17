//! Typos in full-text search (specification §6, L06-US03).
//!
//! A searched word the tenant never wrote may be a typo: it also matches the
//! words the tenant wrote within a few edits of it. An edit inserts, deletes
//! or replaces a letter, or swaps two adjacent letters ("outgae" is one edit
//! from "outage"): typing mistakes are mostly these.
//!
//! - Words of fewer than 4 letters are never corrected: too many words lie
//!   one edit away from them.
//! - Up to 7 letters, one edit; beyond, two.
//! - The nearest words win; among them, those made of the same letters
//!   (swaps), then those starting with the same letter. At most
//!   [`MAX_CORRECTIONS`].
//!
//! Words are compared as the search stores them: lower case, no accents.

/// Corrections kept for one word.
pub const MAX_CORRECTIONS: usize = 5;

/// Shortest word corrected, in letters.
const MIN_LETTERS: usize = 4;

/// Longest word kept, in letters.
pub const MAX_LETTERS: usize = 64;

/// Whether `word` may be a typo of another: letters only, 4 to 64 of them.
pub fn correctable(word: &str) -> bool {
    let letters = word.chars().count();
    (MIN_LETTERS..=MAX_LETTERS).contains(&letters) && word.chars().all(char::is_alphabetic)
}

/// How many edits a correction of `word` may be away from it: none when it
/// is not [`correctable`].
pub fn max_edits(word: &str) -> usize {
    if !correctable(word) {
        return 0;
    }
    if word.chars().count() <= 7 { 1 } else { 2 }
}

/// Edits between `a` and `b`: insertions, deletions, replacements and swaps
/// of adjacent letters (optimal string alignment distance), counted on
/// characters.
pub fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    // Three rows of the table: two back for the swaps.
    let mut before: Vec<usize> = vec![0; b.len() + 1];
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current: Vec<usize> = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        current[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (previous[j] + 1)
                .min(current[j - 1] + 1)
                .min(previous[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(before[j - 2] + 1);
            }
            current[j] = best;
        }
        std::mem::swap(&mut before, &mut previous);
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

/// The words of `candidates` that `word` may be a typo of, best first (see
/// the module). Empty when `word` is not [`correctable`].
pub fn corrections<'a>(word: &str, candidates: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let limit = max_edits(word);
    if limit == 0 {
        return Vec::new();
    }
    let letters = sorted_letters(word);
    let first = word.chars().next();
    let mut found: Vec<(usize, bool, bool, &str)> = candidates
        .into_iter()
        .filter(|candidate| *candidate != word && correctable(candidate))
        .filter_map(|candidate| {
            let edits = distance(word, candidate);
            (edits <= limit).then(|| {
                (
                    edits,
                    sorted_letters(candidate) != letters,
                    candidate.chars().next() != first,
                    candidate,
                )
            })
        })
        .collect();
    found.sort_unstable();
    found.dedup_by(|a, b| a.3 == b.3);
    let Some(nearest) = found.first().map(|f| f.0) else {
        return Vec::new();
    };
    found
        .into_iter()
        .take_while(|f| f.0 == nearest)
        .take(MAX_CORRECTIONS)
        .map(|f| f.3.to_owned())
        .collect()
}

fn sorted_letters(word: &str) -> Vec<char> {
    let mut letters: Vec<char> = word.chars().collect();
    letters.sort_unstable();
    letters
}

/// A word of a search, with the words it may be a typo of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    pub word: String,
    pub corrections: Vec<String>,
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn a_swap_of_adjacent_letters_is_one_edit() {
        for (typo, word) in [
            ("outgae", "outage"),
            ("otuage", "outage"),
            ("mial", "mail"),
            ("pnane", "panne"),
            ("lgoin", "login"),
            ("directroy", "directory"),
            ("sevreur", "serveur"),
        ] {
            assert_eq!(distance(typo, word), 1, "{typo} → {word}");
        }
    }

    #[test]
    fn insertions_deletions_and_replacements_count_one_each() {
        assert_eq!(distance("outage", "outage"), 0);
        assert_eq!(distance("outae", "outage"), 1);
        assert_eq!(distance("outtage", "outage"), 1);
        assert_eq!(distance("outahe", "outage"), 1);
        assert_eq!(distance("output", "outage"), 3);
        assert_eq!(distance("", "abc"), 3);
        assert_eq!(distance("ca", "abc"), 3, "no edit of a swapped pair");
        assert_eq!(distance("électricité", "electricite"), 2, "characters, not bytes");
    }

    #[test]
    fn short_words_are_never_corrected_long_ones_by_two_edits() {
        assert_eq!(max_edits("mal"), 0);
        assert_eq!(max_edits("mail"), 1);
        assert_eq!(max_edits("serveur"), 1);
        assert_eq!(max_edits("directory"), 2);
        assert_eq!(max_edits("v2rc"), 0, "letters only");
        assert_eq!(max_edits(&"a".repeat(65)), 0);
    }

    #[test]
    fn a_typo_is_corrected_to_the_nearest_words() {
        let written = ["outage", "output", "outlook", "outbound", "outages", "directory"];
        assert_eq!(corrections("outgae", written), ["outage"]);
        // One edit from both; the swap (same letters) first.
        assert_eq!(corrections("outagse", written), ["outages", "outage"]);
        assert_eq!(corrections("direcotry", written), ["directory"]);
        assert!(corrections("weather", written).is_empty());
        assert!(corrections("out", written).is_empty(), "too short");
    }

    #[test]
    fn swaps_then_the_same_first_letter_come_first() {
        let written = ["dial", "meal", "mail", "mill", "rial"];
        assert_eq!(
            corrections("mial", written),
            ["mail", "meal", "mill", "dial", "rial"]
        );
        let many = ["mail", "aial", "bial", "cial", "dial", "fial", "gial"];
        assert_eq!(corrections("mial", many).len(), MAX_CORRECTIONS);
        assert_eq!(corrections("mial", many)[0], "mail");
    }

    #[test]
    fn the_word_itself_and_words_not_kept_are_no_correction() {
        assert!(corrections("outage", ["outage"]).is_empty());
        assert!(corrections("outage", ["outag3", "out"]).is_empty());
    }

    proptest! {
        #[test]
        fn distance_is_symmetric_and_zero_only_between_equals(a in "[a-e]{0,8}", b in "[a-e]{0,8}") {
            prop_assert_eq!(distance(&a, &b), distance(&b, &a));
            prop_assert_eq!(distance(&a, &b) == 0, a == b);
            prop_assert!(distance(&a, &b) <= a.chars().count().max(b.chars().count()));
        }
    }
}
