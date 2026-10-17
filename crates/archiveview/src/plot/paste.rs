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
/// on whitespace, commas and semicolons, except inside the `(arguments)`
/// of a `loc://` or `sim://` name closed on the same line
/// (`sim://sine(0, 10, 0.1)` is one word), and quotes and brackets around
/// a word are dropped, as is a lone bracket left by a list in brackets
/// (`(A:B, C:D)`).
///
/// The schemes taken are those the host's [`pv::PvManager`] serves, as
/// [`pv::PvName::parse`] reads them: `ca://` and `pva://` (in either case),
/// and `loc://` and `sim://`, which no archiver has, so they are plotted
/// live only. Formulas (`=expr`, `eq://expr`) are not: their text has
/// spaces and operators, and cannot be told from prose. Any other scheme
/// (`https://`, `sys://`) is not a PV.
///
/// After `ca://` or `pva://`, or with no scheme, a word can be a name if it
/// has only characters EPICS record names use (letters, digits,
/// `_ - + : [ ] < > { } $`, and `.` before a field), at least one letter,
/// and does not start with `-` or `+` (a command-line flag) or start or
/// end with `.` or `:`. After `loc://` such a name can be followed by a
/// `<VType>` and then `(arguments)`, which end it (`loc://x<VLong>(7)`);
/// after `sim://` it is one of bobview-pv's functions (`sine`,
/// `sawtooth`, `ramp`, `noise`, `gaussianNoise`, `flipflop`, `const`), then
/// optionally `(arguments)` (`sim://ramp`, `sim://sine(0, 10, 0.1)`). Unless it has a scheme, shapes that
/// are something else are left out: times and dates (`12:30:00`,
/// `2026-10-17T13:41:52Z`), `host:port` (two parts, a host name with a
/// `.`, `localhost` or lower case, then digits: not `SR:BPM:01` or
/// `DLS:01`), `a::b` paths, and in running text Python log lines
/// (`INFO:root:Connected`; `WARN:LIMIT` is a name).
///
/// A selection of one word is a name if it can be one, so names with no
/// structure at all (`TEMP1`, `BEAMCURRENT`) work. In several words only
/// those with a scheme or some structure are kept (a `:`, `-`, `_`,
/// braces, a digit or a `.FIELD`), so prose, shell commands and code
/// selected with them add nothing.
pub fn pv_names(text: &str) -> Vec<String> {
    let words: Vec<&str> = words(text)
        .into_iter()
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

/// `text` split on whitespace, commas and semicolons, except inside the
/// parentheses of a word that starts (after any quotes or brackets) with
/// `loc://` or `sim://`: the arguments of `sim://sine(0, 10, 0.1)` or
/// `loc://s<VString>("a b")`. A `(` not closed on its line within
/// [`MAX_ARGS`] bytes is an ordinary character, so it cannot swallow the
/// rest, and the work stays linear in a large selection.
fn words(text: &str) -> Vec<&str> {
    let sep = |c: char| c.is_whitespace() || c == ',' || c == ';';
    let mut words = Vec::new();
    // Where the word started, and whether its arguments are kept whole.
    let mut start: Option<(usize, bool)> = None;
    let mut i = 0;
    while let Some(c) = text[i..].chars().next() {
        if sep(c) {
            if let Some((s, _)) = start.take() {
                words.push(&text[s..i]);
            }
        } else {
            let (_, glue) = start.get_or_insert_with(|| (i, has_arguments(&text[i..])));
            if c == '(' && *glue {
                if let Some(len) = closing(&text[i..]) {
                    i += len;
                    continue;
                }
                // Not a name's arguments, so neither is a later `(`.
                *glue = false;
            }
        }
        i += c.len_utf8();
    }
    if let Some((s, _)) = start {
        words.push(&text[s..]);
    }
    words
}

/// Whether `text`, from a word's start, names a `loc://` or `sim://` PV,
/// whose `(arguments)` may hold spaces and commas.
fn has_arguments(text: &str) -> bool {
    let quote_or_bracket = |c| {
        matches!(
            c,
            '"' | '\'' | '`' | '“' | '”' | '‘' | '’' | '(' | '[' | '{' | '<'
        )
    };
    let name = text.trim_start_matches(quote_or_bracket);
    name.starts_with("loc://") || name.starts_with("sim://")
}

/// The longest `(arguments)` kept whole, in bytes: bobview's names are
/// one short line.
const MAX_ARGS: usize = 256;

/// The length of `text`, which starts with `(`, up to and including the
/// `)` that closes it, passing over parentheses in `"strings"`; `None`
/// when it is not closed before a new line or within [`MAX_ARGS`] bytes.
fn closing(text: &str) -> Option<usize> {
    // Bytes: the delimiters are ASCII, which no other character's UTF-8
    // contains.
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for (i, &b) in text.as_bytes().iter().enumerate().take(MAX_ARGS) {
        if b == b'\n' {
            return None;
        }
        if quoted {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => quoted = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => quoted = true,
            b'(' => depth += 1,
            b')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// `word` without the quotes and brackets around it: `"A:B"`, `(A:B)`,
/// `['A:B']`, and a leading `(`/`[` or trailing `)`/`]` the word has one
/// too many of, as a list in brackets splits into (`(A:B,` `C:D)`). A bracket
/// that belongs to the name (`A:B[1]`, `loc://x(1)`) stays.
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
        let inner = [('(', ')'), ('[', ']')]
            .iter()
            .fold(inner, |w, &(open, close)| {
                let count = |c| w.chars().filter(|&x| x == c).count();
                let (opens, closes) = (count(open), count(close));
                match (w.strip_prefix(open), w.strip_suffix(close)) {
                    (Some(rest), _) if opens > closes => rest,
                    (_, Some(rest)) if closes > opens => rest,
                    _ => w,
                }
            });
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
    let Some((scheme, body)) = word.split_once("://") else {
        return record_name(word) && !something_else(word, alone) && (alone || structured(word));
    };
    // A scheme says it is a PV, whatever it looks like, if the host's
    // PvManager serves it.
    match pv::PvName::parse(word).scheme {
        pv::Scheme::Ca | pv::Scheme::Pva => record_name(body),
        pv::Scheme::Loc => host_name(body, false),
        pv::Scheme::Sim => host_name(body, true),
        // Not told from prose (see `pv_names`).
        pv::Scheme::Formula => false,
        // `CA://`, `PVA://`, as some lists write them.
        pv::Scheme::Unsupported => {
            (scheme.eq_ignore_ascii_case("ca") || scheme.eq_ignore_ascii_case("pva"))
                && record_name(body)
        }
    }
}

/// Only characters EPICS record names use, at least one letter, and no
/// flag's `-`/`+` start or `.`/`:` at either end.
fn record_name(body: &str) -> bool {
    let chars_ok = body.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(c, '_' | '-' | '+' | ':' | '[' | ']' | '<' | '>')
            || matches!(c, '.' | '{' | '}' | '$')
    });
    let ends_ok = !body.starts_with(['.', ':', '-', '+']) && !body.ends_with(['.', ':']);
    chars_ok && ends_ok && body.chars().any(|c| c.is_ascii_alphabetic())
}

/// The functions bobview-pv's `sim://` PVs know (its `sim::run`, which
/// does not export them): any other is "unknown simulated PV".
const SIM_FUNCTIONS: [&str; 7] = [
    "sine",
    "sawtooth",
    "ramp",
    "noise",
    "gaussianNoise",
    "flipflop",
    "const",
];

/// A `loc://` name after its scheme: a record name, then optionally a
/// `<VType>`, then optionally `(arguments)` that end it. `sim`: a
/// `sim://` name, one of [`SIM_FUNCTIONS`] with no `<VType>`.
fn host_name(body: &str, sim: bool) -> bool {
    let (base, mut rest) = body.split_at(body.find(['<', '(']).unwrap_or(body.len()));
    if sim && !SIM_FUNCTIONS.contains(&base) {
        return false;
    }
    if let Some(after) = rest.strip_prefix('<')
        && !sim
    {
        let Some((vtype, after)) = after.split_once('>') else {
            return false;
        };
        if vtype.is_empty() || !vtype.chars().all(|c| c.is_ascii_alphanumeric()) {
            return false;
        }
        rest = after;
    }
    let args_ok = rest.is_empty() || (rest.starts_with('(') && closing(rest) == Some(rest.len()));
    record_name(base) && args_ok
}

/// A time, date, `host:port`, `a::b` path or log line, not a PV;
/// `alone` when it is the whole selection.
fn something_else(body: &str, alone: bool) -> bool {
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
    // localhost:5064, ioc.example.org:5075, gateway:5064; not the
    // numbered names everywhere (SR:BPM:01, DLS:01).
    let port = parts.len() == 2 && digits(parts[1]) && {
        let host = parts[0];
        host.contains('.') || host == "localhost" || !host.chars().any(|c| c.is_ascii_uppercase())
    };
    // Python's LEVEL:logger:message, in running text only: WARN:LIMIT
    // alone is a PV.
    let log = !alone
        && parts.len() >= 3
        && LOG_LEVELS.contains(&parts[0])
        && parts[1].chars().any(|c| c.is_ascii_lowercase());
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
            "file:///tmp/x",
            "sys://time",
            "Sim://sine",
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
        ] {
            assert!(pv_names(text).is_empty(), "{text:?}: {:?}", pv_names(text));
        }
        assert_eq!(pv_names("$ caget -a SR:X"), ["SR:X"]);
        assert!(pv_names("INFO:root:Connected and more").is_empty());
    }

    #[test]
    fn local_and_simulated_names_are_names() {
        for (text, want) in [
            ("loc://foo", &["loc://foo"][..]),
            ("loc://x(1.5)", &["loc://x(1.5)"]),
            ("sim://ramp", &["sim://ramp"]),
            // The arguments' commas and spaces do not split the name.
            ("sim://sine(0, 10, 0.1)", &["sim://sine(0, 10, 0.1)"]),
            (" 'sim://sine(0, 10, 0.1)'\n", &["sim://sine(0, 10, 0.1)"]),
            ("loc://a<VLong>(7)", &["loc://a<VLong>(7)"]),
            (
                r#"loc://s<VString>("a (b), c")"#,
                &[r#"loc://s<VString>("a (b), c")"#],
            ),
            ("loc://a(1, 2)", &["loc://a(1, 2)"]),
            ("(sim://noise(-5, 5, 0.5))", &["sim://noise(-5, 5, 0.5)"]),
            (
                "sim://sine(0, 10, 0.1), SR:X; loc://y(2) ca://Z",
                &["sim://sine(0, 10, 0.1)", "SR:X", "loc://y(2)", "ca://Z"],
            ),
            // Among prose, as any scheme'd name is.
            (
                "trend sim://ramp(0, 1, 0.1) and loc://foo please",
                &["sim://ramp(0, 1, 0.1)", "loc://foo"],
            ),
        ] {
            assert_eq!(pv_names(text), want, "{text:?}");
        }
        for text in [
            "loc://",
            "loc://(1)",
            "sim://42",
            "loc://x(1)junk",
            "loc://x<>(1)",
            "loc://x<V Long>(1)",
            "loc://x<VLong",
            // Never closed: an ordinary character, so not a name.
            "sim://sine(0, 10",
            // Formulas are not told from prose.
            "eq://a+b",
            "=a+b",
            // Parentheses without a scheme still split and are still not names.
            "A:B(1, 2)",
        ] {
            assert!(pv_names(text).is_empty(), "{text:?}: {:?}", pv_names(text));
        }
        // An unclosed bracket does not swallow what follows.
        assert_eq!(pv_names("sim://sine(0, SR:X"), ["SR:X"]);
        // Nor does one closed lines later, or far on.
        assert_eq!(
            pv_names("loc://x(1 or\n\nSR:A1 SR:B1\nlater)"),
            ["SR:A1", "SR:B1"]
        );
        let far = format!("loc://x(1 {} SR:A1)", "word ".repeat(60));
        assert_eq!(pv_names(&far), ["SR:A1"]);
        // Only loc:// and sim:// arguments are kept whole: the URL's
        // words split, and the name after its comma is found.
        assert_eq!(pv_names("https://x/Foo_(SR:A1, SR:B1)"), ["SR:B1"]);
        // sim:// names only bobview-pv's functions, with no type.
        for text in [
            "sim://foo",
            "sim://Sine",
            "sim://a:b<VLong>",
            "sim://ramp<VLong>(1)",
        ] {
            assert!(pv_names(text).is_empty(), "{text:?}");
        }
        for f in SIM_FUNCTIONS {
            let name = format!("sim://{f}(1, 2)");
            assert_eq!(pv_names(&name), [name.as_str()]);
        }
    }

    #[test]
    fn a_large_selection_is_parsed_in_linear_time() {
        // Each `(` here once scanned to the end of the text: a 120 KB
        // selection took 20 s. A megabyte now takes well under a second; the
        // bound is generous for a slow CI machine in a debug build.
        for text in [
            "loc://( ".repeat(150_000),
            format!("loc://{}", "(".repeat(1_000_000)),
            format!("sim://sine{}", "(x ".repeat(350_000)),
        ] {
            let started = web_time::Instant::now();
            pv_names(&text);
            let took = started.elapsed();
            assert!(took.as_secs() < 10, "{}: took {took:?}", &text[..20]);
        }
    }

    #[test]
    fn lone_brackets_of_a_list_are_dropped() {
        assert_eq!(pv_names("the beam (SR:X, SR:Y) is down"), ["SR:X", "SR:Y"]);
        assert_eq!(
            pv_names("[SR:X1, SR:Y1] and (A:B[1], loc://x(1))"),
            ["SR:X1", "SR:Y1", "A:B[1]", "loc://x(1)"]
        );
        // Brackets that belong to the name stay.
        assert_eq!(pv_names("A:B[1] C:D[2]"), ["A:B[1]", "C:D[2]"]);
        assert!(pv_names("A:B(1)").is_empty());
    }

    #[test]
    fn numbered_and_log_level_names_are_names() {
        for (text, want) in [
            ("SR:BPM:01", &["SR:BPM:01"][..]),
            ("LI:TEMP:1", &["LI:TEMP:1"]),
            ("TEST:AI:1 TEST:AI:2", &["TEST:AI:1", "TEST:AI:2"]),
            ("pva://SR:BPM:01", &["pva://SR:BPM:01"]),
            ("DLS:01", &["DLS:01"]),
            ("S1:2", &["S1:2"]),
            ("WARN:LIMIT", &["WARN:LIMIT"]),
            ("INFO:STATUS", &["INFO:STATUS"]),
            ("ERROR:COUNT and SR:X", &["ERROR:COUNT", "SR:X"]),
            // A scheme skips the junk checks.
            ("ca://localhost:5064", &["ca://localhost:5064"]),
        ] {
            assert_eq!(pv_names(text), want, "{text:?}");
        }
        for text in ["localhost:5064", "ioc.example.org:5075", "gateway:5064"] {
            assert!(pv_names(text).is_empty(), "{text:?}");
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
            "The selection names 143 PVs: select at most 20"
        );
    }
}
