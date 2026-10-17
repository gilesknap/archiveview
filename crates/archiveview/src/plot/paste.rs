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
    /// It named more than [`MAX_PASTE`] PVs, this many, so none were
    /// added: most likely a screenful pasted by accident.
    pub too_many: usize,
}

/// The most PVs one paste adds. A paste naming more adds none, and says
/// to paste it into the search box instead.
pub const MAX_PASTE: usize = 20;

impl Pasted {
    /// A short message saying so, for a toast.
    pub fn message(&self) -> String {
        if self.too_many > 0 {
            return format!(
                "The selection names {} PVs (at most {MAX_PASTE}): \
                 paste it into the search box to choose",
                self.too_many
            );
        }
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
/// whitespace, commas and semicolons, with quotes and brackets around them
/// dropped, that look like PV names. After an optional `ca://` or
/// `pva://`, a name has only characters EPICS record names use (letters,
/// digits, `_ - + : [ ] < > { } $` and `.` before a field), at least one
/// letter, and no `.` or `:` at either end. So a plain word is a name
/// (some sites' names have no colon), while numbers, times such as
/// `12:30:00`, URLs and a sentence's last word ("down.") are not.
pub fn pv_names(text: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for word in text.split(|c: char| c.is_whitespace() || c == ',' || c == ';') {
        let word = unwrap(word);
        if looks_like_pv(word) && !names.iter().any(|n| n == word) {
            names.push(word.to_string());
        }
    }
    names
}

/// `word` without the quotes and brackets around it: `"A:B"`, `(A:B)`,
/// `['A:B']`. A bracket that belongs to the name (`A:B[1]`) stays.
fn unwrap(mut word: &str) -> &str {
    loop {
        let quotes = |c| matches!(c, '"' | '\'' | '`' | '“' | '”' | '‘' | '’');
        let trimmed = word.trim_matches(quotes);
        let inner = [('(', ')'), ('[', ']'), ('{', '}'), ('<', '>')]
            .iter()
            .find_map(|&(open, close)| {
                trimmed
                    .strip_prefix(open)
                    .and_then(|w| w.strip_suffix(close))
            })
            .unwrap_or(trimmed);
        if inner == word {
            return word;
        }
        word = inner;
    }
}

fn looks_like_pv(word: &str) -> bool {
    let body = match word.split_once("://") {
        Some((scheme, body))
            if scheme.eq_ignore_ascii_case("ca") || scheme.eq_ignore_ascii_case("pva") =>
        {
            body
        }
        Some(_) => return false,
        None => word,
    };
    let chars_ok = body.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(c, '_' | '-' | '+' | ':' | '[' | ']' | '<' | '>')
            || matches!(c, '.' | '{' | '}' | '$')
    });
    let ends_ok = !body.starts_with(['.', ':']) && !body.ends_with(['.', ':']);
    chars_ok && ends_ok && body.chars().any(|c| c.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_picked_out_of_text() {
        assert_eq!(pv_names("BL16I-EA-DET-01:COUNT"), ["BL16I-EA-DET-01:COUNT"]);
        assert_eq!(
            pv_names(" SR-DI-DCCT-01:SIGNAL,\nSR-DI-DCCT-01:LIFETIME\t'BL01T:X.VAL'; "),
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
            pv_names("XF:23ID-CT{Bus}Sts A:B[1] A:B<x> $(P)M1"),
            ["XF:23ID-CT{Bus}Sts", "A:B[1]", "A:B<x>"]
        );
        // Names without colons, as some sites have.
        assert_eq!(pv_names("TEMP1 beam_current"), ["TEMP1", "beam_current"]);
        // Quotes and brackets around a name go.
        assert_eq!(
            pv_names(r#"("A:B") ['C:D'] {E:F} <G> “H:I”"#),
            ["A:B", "C:D", "E:F", "G", "H:I"]
        );
        // Once each, in order.
        assert_eq!(pv_names("A:B C:D A:B"), ["A:B", "C:D"]);
    }

    #[test]
    fn other_words_are_not_names() {
        for text in [
            "",
            "  \n ",
            "42 3.14 -7",
            "12:30:05",
            "down. Note:",
            ":LEADING .dot",
            "https://archiver.example.com/retrieval",
            "sim://sine",
            "file:///tmp/x",
            "A:B(1)",
            "A:B/C",
            "it's",
            "a=b",
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
            too_many: 0,
        };
        assert_eq!(p(&["A:B"], &[], "").message(), "Added A:B");
        assert_eq!(p(&["A:B", "C:D"], &["E:F"], "").message(), "Added 2 PVs");
        assert_eq!(p(&[], &["A:B"], "").message(), "A:B is plotted already");
        assert_eq!(
            p(&[], &["A:B", "C:D"], "").message(),
            "2 PVs are plotted already"
        );
        assert_eq!(
            p(&[], &[], "12:30\n  42").message(),
            "No PV names in “12:30 42”"
        );
        assert!(p(&[], &[], &"1".repeat(100)).message().ends_with("1…”"));
        assert!(p(&[], &[], " ").message().contains("empty"));
        let many = Pasted {
            too_many: 143,
            ..Pasted::default()
        };
        assert_eq!(
            many.message(),
            "The selection names 143 PVs (at most 20): paste it into the search box to choose"
        );
    }
}
