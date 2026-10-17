//! Blocking HTTP over `ureq` (native builds only).

use std::time::Duration;

use crate::{ArchiverClient, Error, Result};

/// The largest body read: raw requests over long ranges can be big.
const MAX_BODY: u64 = 1 << 30;

impl ArchiverClient {
    /// GET `url` and return the body of a 2xx answer as text. A non-2xx
    /// answer is [`Error::Http`] with the body's first meaningful line.
    pub(crate) fn get(&self, url: &str) -> Result<String> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(self.timeout_secs.max(1))))
            .http_status_as_error(false)
            .user_agent(concat!("archiveview/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        log::debug!("GET {url}");
        let mut response = agent
            .get(url)
            .header("Accept", "application/json")
            .call()
            .map_err(|e| Error::Transport(format!("{}: {e}", self.base())))?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_BODY)
            .read_to_vec()
            .map_err(|e| Error::Transport(format!("{}: {e}", self.base())))?;
        if !(200..300).contains(&status) {
            let text = String::from_utf8_lossy(&body);
            return Err(Error::Http {
                status,
                message: first_line(&text),
            });
        }
        String::from_utf8(body).map_err(|e| Error::Parse(format!("not UTF-8: {e}")))
    }
}

/// A short human message from an error body: an HTML page's `<title>`, or
/// the first non-empty line of the text with tags stripped.
pub(crate) fn first_line(body: &str) -> String {
    const MAX: usize = 200;
    let lower = body.to_ascii_lowercase();
    let text = match (lower.find("<title>"), lower.find("</title>")) {
        (Some(a), Some(b)) if a + 7 <= b => strip_tags(&body[a + 7..b]),
        _ => strip_tags(body),
    };
    let line = text
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    match line.char_indices().nth(MAX) {
        Some((i, _)) => format!("{}…", &line[..i]),
        None => line,
    }
}

/// Drop tags (and `<style>`/`<script>` contents) and decode the common
/// entities. Tags become line breaks, so block text stays separate.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let tail = &rest[open..];
        let lower = tail.get(..8).unwrap_or(tail).to_ascii_lowercase();
        let skip_to = if lower.starts_with("<style") {
            Some("</style>")
        } else if lower.starts_with("<script") {
            Some("</script>")
        } else {
            None
        };
        let end = match skip_to {
            Some(close) => tail
                .to_ascii_lowercase()
                .find(close)
                .map(|i| i + close.len()),
            None => tail.find('>').map(|i| i + 1),
        };
        match end {
            Some(end) => {
                out.push('\n');
                rest = &tail[end..];
            }
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::first_line;

    #[test]
    fn tomcat_pages_give_their_title() {
        let page = "<!doctype html><html lang=\"en\"><head><title>HTTP Status 404 \u{2013} Not Found</title>\
                    <style type=\"text/css\">body {font-family:Tahoma;}</style></head><body><h1>HTTP Status 404</h1></body></html>";
        assert_eq!(first_line(page), "HTTP Status 404 \u{2013} Not Found");
    }

    #[test]
    fn plain_and_tagged_text() {
        assert_eq!(
            first_line("\n\n  Appliance  starting \nmore"),
            "Appliance starting"
        );
        assert_eq!(
            first_line("<html><style>x{}</style><body><p>No &amp; way</p></body>"),
            "No & way"
        );
        assert_eq!(first_line(""), "");
        let long = "x".repeat(500);
        assert_eq!(first_line(&long).chars().count(), 201);
    }
}
