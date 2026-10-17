//! Tolerant JSON: the archiver writes NaN and infinities as bare tokens.

use std::borrow::Cow;

/// Replace the bare tokens `NaN`, `Infinity` and `-Infinity` outside strings
/// with `null`, so `serde_json` accepts the body. Borrowed when there are none.
pub fn sanitize_json(body: &str) -> Cow<'_, str> {
    if !body.contains("NaN") && !body.contains("Infinity") {
        return Cow::Borrowed(body);
    }
    let b = body.as_bytes();
    let mut out = String::with_capacity(body.len());
    // `out` takes whole slices of `body` between replacements, so it stays
    // valid UTF-8 (every split point is an ASCII byte).
    let mut copied = 0;
    let mut in_string = false;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if in_string {
            match c {
                b'\\' => i += 1,
                b'"' => in_string = false,
                _ => {}
            }
            i += 1;
            continue;
        }
        let token = match c {
            b'"' => {
                in_string = true;
                None
            }
            b'N' if b[i..].starts_with(b"NaN") => Some(3),
            b'I' if b[i..].starts_with(b"Infinity") => Some(8),
            b'-' if b[i..].starts_with(b"-Infinity") => Some(9),
            b'+' if b[i..].starts_with(b"+Infinity") => Some(9),
            _ => None,
        };
        match token {
            Some(len) => {
                out.push_str(&body[copied..i]);
                out.push_str("null");
                i += len;
                copied = i;
            }
            None => i += 1,
        }
    }
    if copied == 0 {
        return Cow::Borrowed(body);
    }
    out.push_str(&body[copied..]);
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_bare_tokens_only() {
        let body = r#"[{"val": NaN, "a": [Infinity,-Infinity, 1.0E-5], "s": "NaN \"Infinity\" -Infinity"}]"#;
        let s = sanitize_json(body);
        assert_eq!(
            s,
            r#"[{"val": null, "a": [null,null, 1.0E-5], "s": "NaN \"Infinity\" -Infinity"}]"#
        );
        assert!(serde_json::from_str::<serde_json::Value>(&s).is_ok());
    }

    #[test]
    fn borrows_when_clean() {
        assert!(matches!(sanitize_json("[1, 2]"), Cow::Borrowed(_)));
        // Mentions only inside strings: unchanged and still borrowed.
        assert!(matches!(
            sanitize_json(r#"["NaN", "Infinity\\"]"#),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn keeps_unicode() {
        let s = sanitize_json("[\"\u{2013} é\", NaN]");
        assert_eq!(s, "[\"\u{2013} é\", null]");
    }
}
