//! Adding PVs from pasted text: the primary selection (middle-click, the
//! EDM/MEDM way) or the clipboard (Ctrl+V over the plot).
//!
//! The widget never touches a clipboard itself, so it needs no clipboard
//! dependency: it reports middle-clicks ([`Plot::take_middle_click`]) for
//! the host to answer with the selection's text ([`Plot::paste_pvs`],
//! [`Plot::paste_into_search`]), and reports the search box's selected
//! text for the host to make the primary selection
//! ([`Plot::take_search_selection`]). Ctrl+V needs no host: egui delivers
//! the clipboard as an `Event::Paste`.
//!
//! [`Plot::take_middle_click`]: super::Plot::take_middle_click
//! [`Plot::paste_pvs`]: super::Plot::paste_pvs
//! [`Plot::paste_into_search`]: super::Plot::paste_into_search
//! [`Plot::take_search_selection`]: super::Plot::take_search_selection

/// Where a middle-click landed, so where the selection's text goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MiddleClick {
    /// The plot area or the legend: add the PVs named in it
    /// ([`Plot::paste_pvs`](super::Plot::paste_pvs)).
    Plot,
    /// The search box: paste the text into it at the cursor
    /// ([`Plot::paste_into_search`](super::Plot::paste_into_search)).
    Search,
}

/// What pasting text onto the plot did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pasted {
    /// The PVs added.
    pub added: Vec<String>,
    /// PVs named that were plotted already.
    pub present: Vec<String>,
    /// The text, when it named no PV.
    pub text: String,
}

impl Pasted {
    /// A short message saying so, for a toast.
    pub fn message(&self) -> String {
        match (self.added.as_slice(), self.present.as_slice()) {
            ([one], _) => format!("Added {one}"),
            ([], []) => {
                let text = self.text.split_whitespace().collect::<Vec<_>>().join(" ");
                if text.is_empty() {
                    "Nothing to add: the selection is empty".into()
                } else {
                    let shown: String = text.chars().take(40).collect();
                    let more = if shown.len() < text.len() { "…" } else { "" };
                    format!("No PV names in “{shown}{more}”")
                }
            }
            ([], [one]) => format!("{one} is plotted already"),
            ([], all) => format!("{} PVs are plotted already", all.len()),
            (added, _) => format!("Added {} PVs", added.len()),
        }
    }
}

/// The widget's paste state.
#[derive(Default)]
pub(super) struct PasteState {
    /// A middle-click the host has not taken yet.
    pub middle: Option<MiddleClick>,
    /// The last paste's outcome, until the host takes it.
    pub pasted: Option<Pasted>,
    /// The search box's selected text as last reported, and the report
    /// the host has not taken yet.
    pub selected: String,
    pub selection: Option<String>,
}

/// The PV names in `text`, in order and once each: words split on
/// whitespace and commas (quotes around a word dropped) that look like
/// PV names. A name has a `ca://` or `pva://` prefix, or a colon not at
/// its start or end (so prose such as "Note:" is not one); it has a letter
/// and only characters EPICS record names use (plus `{}`, as at NSLS-II,
/// and `.` before a field).
pub fn pv_names(text: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for word in text.split(|c: char| c.is_whitespace() || c == ',') {
        let word = word.trim_matches(|c| matches!(c, '"' | '\'' | '`'));
        if looks_like_pv(word) && !names.iter().any(|n| n == word) {
            names.push(word.to_string());
        }
    }
    names
}

fn looks_like_pv(word: &str) -> bool {
    let (prefixed, body) = match word.split_once("://") {
        Some((scheme, body)) if scheme.eq_ignore_ascii_case("ca") => (true, body),
        Some((scheme, body)) if scheme.eq_ignore_ascii_case("pva") => (true, body),
        Some(_) => return false,
        None => (false, word),
    };
    let chars_ok = body.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(c, '_' | '-' | '+' | ':' | '[' | ']' | '<' | '>')
            || matches!(c, ';' | '.' | '{' | '}')
    });
    let colon = body.contains(':') && !body.starts_with(':') && !body.ends_with(':');
    chars_ok && body.chars().any(|c| c.is_ascii_alphabetic()) && (prefixed || colon)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_picked_out_of_text() {
        assert_eq!(pv_names("BL16I-EA-DET-01:COUNT"), ["BL16I-EA-DET-01:COUNT"]);
        assert_eq!(
            pv_names(" SR-DI-DCCT-01:SIGNAL,\nSR-DI-DCCT-01:LIFETIME\t'BL01T:X.VAL' "),
            [
                "SR-DI-DCCT-01:SIGNAL",
                "SR-DI-DCCT-01:LIFETIME",
                "BL01T:X.VAL"
            ]
        );
        assert_eq!(
            pv_names("pva://BL01T:IMG ca://lowercase:pv PVA://X"),
            ["pva://BL01T:IMG", "ca://lowercase:pv", "PVA://X"]
        );
        assert_eq!(
            pv_names("XF:23ID-CT{Bus}Sts A:B[1] A:B<x>"),
            ["XF:23ID-CT{Bus}Sts", "A:B[1]", "A:B<x>"]
        );
        // Once each, in order.
        assert_eq!(pv_names("A:B C:D A:B"), ["A:B", "C:D"]);
    }

    #[test]
    fn other_words_are_not_names() {
        for text in [
            "",
            "  \n ",
            "hello world",
            "Note: the beam is down",
            ":LEADING",
            "12:30:05",
            "https://archiver.example.com/retrieval",
            "sim://sine",
            "file:///tmp/x",
            "A:B(1)",
            "A:B/C",
            "SR-DI-DCCT-01",
        ] {
            assert!(pv_names(text).is_empty(), "{text:?}: {:?}", pv_names(text));
        }
    }

    #[test]
    fn the_message_says_what_happened() {
        let p = |added: &[&str], present: &[&str], text: &str| Pasted {
            added: added.iter().map(|s| s.to_string()).collect(),
            present: present.iter().map(|s| s.to_string()).collect(),
            text: text.into(),
        };
        assert_eq!(p(&["A:B"], &[], "").message(), "Added A:B");
        assert_eq!(p(&["A:B", "C:D"], &["E:F"], "").message(), "Added 2 PVs");
        assert_eq!(p(&[], &["A:B"], "").message(), "A:B is plotted already");
        assert_eq!(
            p(&[], &["A:B", "C:D"], "").message(),
            "2 PVs are plotted already"
        );
        assert_eq!(
            p(&[], &[], "hello\n  world").message(),
            "No PV names in “hello world”"
        );
        assert!(p(&[], &[], &"x".repeat(100)).message().ends_with("x…”"));
        assert!(p(&[], &[], " ").message().contains("empty"));
    }
}
