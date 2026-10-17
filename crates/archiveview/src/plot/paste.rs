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

/// The most PVs one paste adds. A paste naming more adds none (most
/// likely a screenful selected by accident), and says so.
pub const MAX_PASTE: usize = 20;

impl Pasted {
    /// A short message saying so, for a toast.
    pub fn message(&self) -> String {
        if self.too_many > 0 {
            return format!(
                "The selection names {} PVs: select at most {MAX_PASTE}",
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
    /// What the box selected by itself on focus (select-all): not the
    /// user's selection, so not reported, until a press or key in the box.
    pub auto: Option<String>,
    /// Where the search box's ✕ was last drawn.
    #[cfg(test)]
    pub clear_rect: Option<egui::Rect>,
}

/// The PV names in `text`, in order and once each. It is split into words
/// on whitespace, commas and semicolons, and quotes and brackets around a
/// word are dropped. A word can be a name if, after an optional `ca://` or
/// `pva://`, it has only characters EPICS record names use (letters,
/// digits, `_ - + : [ ] < > { } $`, and `.` before a field), at least one
/// letter, and does not start with `-` or `+` (a command-line flag) or
/// start or end with `.` or `:`. Shapes that are something else are
/// left out: times and dates (`12:30:00`, `2026-10-17T13:41:52Z`),
/// `host:port`, `a::b` paths and log prefixes (`INFO:root:…`).
///
/// A selection of one word is a name if it can be one, so names with no
/// structure at all (`TEMP1`, `BEAMCURRENT`) work. In several words only
/// those with a scheme or some structure are kept (a `:`, `-`, `_`,
/// braces, a digit or a `.FIELD`), so prose, shell commands and code
/// selected with them add nothing.
pub fn pv_names(text: &str) -> Vec<String> {
    let words: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
        .map(unwrap)
        .filter(|w| !w.is_empty())
        .collect();
    let one = words.len() == 1;
    let mut names: Vec<String> = Vec::new();
    for word in words {
        if looks_like_pv(word, one) && !names.iter().any(|n| n == word) {
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

/// Log levels, which start lines such as `INFO:root:Connected`.
const LOG_LEVELS: [&str; 8] = [
    "TRACE", "DEBUG", "INFO", "WARN", "WARNING", "ERROR", "CRITICAL", "FATAL",
];

/// Whether `word` can be a PV name; `alone` when it is the whole
/// selection (see [`pv_names`]).
fn looks_like_pv(word: &str, alone: bool) -> bool {
    let (prefixed, body) = match word.split_once("://") {
        Some((scheme, body))
            if scheme.eq_ignore_ascii_case("ca") || scheme.eq_ignore_ascii_case("pva") =>
        {
            (true, body)
        }
        Some(_) => return false,
        None => (false, word),
    };
    let chars_ok = body.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(c, '_' | '-' | '+' | ':' | '[' | ']' | '<' | '>')
            || matches!(c, '.' | '{' | '}' | '$')
    });
    let ends_ok = !body.starts_with(['.', ':', '-', '+']) && !body.ends_with(['.', ':']);
    if !(chars_ok && ends_ok && body.chars().any(|c| c.is_ascii_alphabetic())) {
        return false;
    }
    if something_else(body) {
        return false;
    }
    alone || prefixed || structured(body)
}

/// A time, date, `host:port`, `a::b` path or log line, not a PV.
fn something_else(body: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let parts: Vec<&str> = body.split(':').collect();
    let b = body.as_bytes();
    let date = b.len() >= 10
        && digits(&body[..4])
        && b[4] == b'-'
        && digits(&body[5..7])
        && b[7] == b'-'
        && digits(&body[8..10]);
    // 12:30, 13:41:52Z: digits before every colon.
    let time = parts.len() > 1 && parts[..parts.len() - 1].iter().all(|p| digits(p));
    let port = parts.len() > 1 && digits(parts[parts.len() - 1]);
    let log = parts.len() > 1 && LOG_LEVELS.contains(&parts[0]);
    date || time || port || log || body.contains("::")
}

/// Has some structure, as a PV name in running text would: a `:`, `-`,
/// `_`, braces, a digit, or a `.FIELD` (upper case) at its end.
fn structured(body: &str) -> bool {
    let field = body.rsplit_once('.').is_some_and(|(_, f)| {
        f.starts_with(|c: char| c.is_ascii_uppercase())
            && f.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    });
    field
        || body
            .chars()
            .any(|c| matches!(c, ':' | '-' | '_' | '{' | '}') || c.is_ascii_digit())
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
        // One word with no structure is a name (some sites' have none).
        assert_eq!(pv_names(" TEMP1\n"), ["TEMP1"]);
        assert_eq!(pv_names("BEAMCURRENT"), ["BEAMCURRENT"]);
        assert_eq!(pv_names("'beamcurrent'"), ["beamcurrent"]);
        // In several, words with some structure are.
        assert_eq!(
            pv_names("TEMP1 beam_current RING-CURRENT BEAM.VAL plain Word"),
            ["TEMP1", "beam_current", "RING-CURRENT", "BEAM.VAL"]
        );
        // Quotes and brackets around a name go.
        assert_eq!(
            pv_names(r#"("A:B") ['C:D'] {E:F} <G1> “H:I”"#),
            ["A:B", "C:D", "E:F", "G1", "H:I"]
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
            // Prose, a shell command's flags, an `ls -l` line, code.
            "the beam is down",
            "$ caget -a",
            "-rw-r--r-- 1 root root 0 Oct 17 12:00 file.txt",
            "fn main() { let x = 1; }",
            "-a",
            "+x",
            // Times, dates, ports, paths, log lines.
            "2026-10-17T13:41:52Z",
            "2026-10-17",
            "13:41:52Z",
            "localhost:5064",
            "std::vec::Vec",
            "INFO:root:Connected",
        ] {
            assert!(pv_names(text).is_empty(), "{text:?}: {:?}", pv_names(text));
        }
        assert_eq!(pv_names("$ caget -a SR:X"), ["SR:X"]);
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
            "The selection names 143 PVs: select at most 20"
        );
    }
}
