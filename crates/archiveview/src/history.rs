//! Recent searches: the search box's terms that were useful, newest first.
//!
//! A term is remembered when a PV is added from its results (or the typed
//! text itself is added), so typos and half-typed prefixes are not. The
//! list is plain data: [`crate::Plot`] keeps one in memory, and the
//! standalone app loads and saves it as `history.json` (see
//! `config::load_history`). A host embedding the plot may persist it the
//! same way, or not at all.

use serde::{Deserialize, Serialize};

/// The most searches kept.
pub const MAX_SEARCHES: usize = 50;

/// A remembered search: the text as typed, and whether it was a regex.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentSearch {
    pub text: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub regex: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// The recent searches, most recent first, without duplicates, at most
/// [`MAX_SEARCHES`]. Also the shape of `history.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHistory {
    #[serde(default)]
    searches: Vec<RecentSearch>,
}

impl SearchHistory {
    /// A history of `searches`, most recent first; blank and repeated
    /// entries are dropped, and any past [`MAX_SEARCHES`].
    pub fn new(searches: Vec<RecentSearch>) -> Self {
        let mut h = Self::default();
        for s in searches.into_iter().rev() {
            h.record(&s.text, s.regex);
        }
        h
    }

    /// The searches, most recent first.
    pub fn searches(&self) -> &[RecentSearch] {
        &self.searches
    }

    pub fn is_empty(&self) -> bool {
        self.searches.is_empty()
    }

    /// Remember `text` (trimmed) as the most recent search, moving it to
    /// the top if it is already there. Returns whether the list changed.
    pub fn record(&mut self, text: &str, regex: bool) -> bool {
        let text = text.trim();
        if text.is_empty() {
            return false;
        }
        let entry = RecentSearch {
            text: text.to_string(),
            regex,
        };
        if self.searches.first() == Some(&entry) {
            return false;
        }
        self.searches.retain(|s| *s != entry);
        self.searches.insert(0, entry);
        self.searches.truncate(MAX_SEARCHES);
        true
    }

    /// Forget one search. Returns whether it was there.
    pub fn forget(&mut self, search: &RecentSearch) -> bool {
        let before = self.searches.len();
        self.searches.retain(|s| s != search);
        self.searches.len() != before
    }

    pub fn clear(&mut self) {
        self.searches.clear();
    }

    /// The searches offered while `text` is typed: those starting with it,
    /// then those containing it (ignoring case), each most recent first.
    /// Not `text` itself, nor anything with the box empty.
    pub fn matching(&self, text: &str) -> Vec<&RecentSearch> {
        let text = text.trim().to_lowercase();
        if text.is_empty() {
            return Vec::new();
        }
        let (mut starts, mut contains) = (Vec::new(), Vec::new());
        for s in &self.searches {
            let lower = s.text.to_lowercase();
            if lower == text {
                continue;
            } else if lower.starts_with(&text) {
                starts.push(s);
            } else if lower.contains(&text) {
                contains.push(s);
            }
        }
        starts.extend(contains);
        starts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(h: &SearchHistory) -> Vec<&str> {
        h.searches().iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn newest_first_without_duplicates() {
        let mut h = SearchHistory::default();
        assert!(h.record("BL16I", false));
        assert!(h.record(" SR-DI ", false));
        assert!(!h.record("SR-DI", false), "already the newest");
        assert!(!h.record("  ", false));
        assert_eq!(texts(&h), ["SR-DI", "BL16I"]);
        // A repeat moves to the top; the regex flag makes it another search.
        assert!(h.record("BL16I", false));
        assert_eq!(texts(&h), ["BL16I", "SR-DI"]);
        assert!(h.record("BL16I", true));
        assert_eq!(texts(&h), ["BL16I", "BL16I", "SR-DI"]);
        assert!(h.searches()[0].regex);

        assert!(h.forget(&RecentSearch {
            text: "BL16I".into(),
            regex: true
        }));
        assert_eq!(texts(&h), ["BL16I", "SR-DI"]);
        assert!(!h.searches()[0].regex);
        h.clear();
        assert!(h.is_empty());
    }

    #[test]
    fn capped_at_the_most_recent() {
        let mut h = SearchHistory::default();
        for i in 0..MAX_SEARCHES + 10 {
            h.record(&format!("PV{i}"), false);
        }
        assert_eq!(h.searches().len(), MAX_SEARCHES);
        assert_eq!(h.searches()[0].text, format!("PV{}", MAX_SEARCHES + 9));
        assert_eq!(h.searches().last().unwrap().text, "PV10");
        // As loaded: cleaned up the same way, order kept.
        let loaded = SearchHistory::new(
            ["b", "a", "b", " ", "c"]
                .iter()
                .map(|t| RecentSearch {
                    text: t.to_string(),
                    regex: false,
                })
                .collect(),
        );
        assert_eq!(texts(&loaded), ["b", "a", "c"]);
    }

    #[test]
    fn matches_start_then_contain() {
        let mut h = SearchHistory::default();
        for t in ["xbl16i", "BL16I-EA-*", "SR-DI", "bl16i", "BL16I-MO"] {
            h.record(t, false);
        }
        let m: Vec<&str> = h
            .matching("BL16I")
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(m, ["BL16I-MO", "BL16I-EA-*", "xbl16i"]);
        assert!(h.matching(" ").is_empty());
    }
}
