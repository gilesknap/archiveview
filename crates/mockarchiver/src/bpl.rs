//! The business-logic (`bpl`) endpoints: PV search and metadata.

use crate::data::{java_double, json_str};
use crate::signals;
use crate::{Params, Reply};

/// `limit` as the appliance reads it: default 500, -1 (or any negative)
/// for all.
fn limit(params: &Params) -> Result<Option<usize>, Reply> {
    match params.get("limit") {
        None => Ok(Some(500)),
        Some(s) => match s.trim().parse::<i64>() {
            Ok(n) if n < 0 => Ok(None),
            Ok(n) => Ok(Some(n as usize)),
            Err(_) => Err(Reply::error(400, "Bad Request", &format!("bad limit {s}"))),
        },
    }
}

/// A whole-name matcher (Java `Pattern.matches`).
fn full_match(re: &str) -> Result<regex::Regex, Reply> {
    regex::Regex::new(&format!("^(?:{re})$"))
        .map_err(|e| Reply::error(500, "Internal Server Error", &e.to_string()))
}

fn names_json<'a>(names: impl Iterator<Item = &'a str>) -> String {
    let items: Vec<String> = names.map(json_str).collect();
    format!("[{}]", items.join(","))
}

/// `GET /retrieval/bpl/getMatchingPVs?pv=<glob>|regex=<re>&limit=N`.
///
/// The glob is converted as naively as the appliance does it (`*` → `.*`,
/// `?` → `.`, nothing escaped) and must match the whole name. `pv` wins
/// over `regex`; with neither the answer is 404. Sorted.
pub(crate) fn get_matching_pvs(params: &Params) -> Reply {
    let re = match (params.get("pv"), params.get("regex")) {
        (Some(glob), _) => glob.replace('*', ".*").replace('?', "."),
        (None, Some(re)) => re.to_string(),
        (None, None) => {
            return Reply::error(404, "Not Found", "Please specify a pv or regex parameter");
        }
    };
    let (re, limit) = match (full_match(&re), limit(params)) {
        (Ok(re), Ok(limit)) => (re, limit),
        (Err(e), _) | (_, Err(e)) => return e,
    };
    let mut names: Vec<&str> = signals::all()
        .iter()
        .map(|s| s.name)
        .filter(|n| re.is_match(n))
        .collect();
    names.sort_unstable();
    names.truncate(limit.unwrap_or(usize::MAX));
    Reply::json(names_json(names.into_iter()))
}

/// A real glob (Apache `WildcardFileFilter`: `*` and `?`, everything else
/// literal) as a regex.
fn glob_regex(glob: &str) -> String {
    let mut re = String::new();
    for c in glob.chars() {
        match c {
            '*' => re.push_str(".*"),
            '?' => re.push('.'),
            c => re.push_str(&regex::escape(&c.to_string())),
        }
    }
    re
}

/// `GET /mgmt/bpl/getAllPVs?pv=<glob>[,<glob>...]&regex=<re>&limit=N`:
/// unsorted (table order); everything with neither filter.
pub(crate) fn get_all_pvs(params: &Params) -> Reply {
    let re = match (params.get("pv"), params.get("regex")) {
        (Some(globs), _) => {
            let alts: Vec<String> = globs.split(',').map(glob_regex).collect();
            Some(alts.join("|"))
        }
        (None, Some(re)) => Some(re.to_string()),
        (None, None) => None,
    };
    let re = match re.map(|r| full_match(&r)).transpose() {
        Ok(re) => re,
        Err(e) => return e,
    };
    let limit = match limit(params) {
        Ok(l) => l.unwrap_or(usize::MAX),
        Err(e) => return e,
    };
    let names = signals::all()
        .iter()
        .map(|s| s.name)
        .filter(|n| re.as_ref().is_none_or(|re| re.is_match(n)))
        .take(limit);
    Reply::json(names_json(names))
}

/// `GET /retrieval/bpl/getVersion`, in the appliance's form.
pub(crate) fn get_version() -> Reply {
    let version = format!(
        "Archiver Appliance Version mock-{}",
        env!("CARGO_PKG_VERSION")
    );
    Reply::json(format!("{{\"version\":{}}}", json_str(&version)))
}

/// `GET /retrieval/bpl/getMetadata?pv=PV`: a flat object of strings.
pub(crate) fn get_metadata(params: &Params) -> Reply {
    let Some(pv) = params.get("pv") else {
        return Reply::error(400, "Bad Request", "No PV specified");
    };
    let Some(s) = signals::find(pv) else {
        return Reply::error(404, "Not Found", &format!("Cannot find PV {pv}"));
    };
    let fields: Vec<(&str, String)> = vec![
        ("pvName", s.name.to_string()),
        ("DBRType", s.kind.dbr_type().to_string()),
        ("elementCount", s.kind.element_count().to_string()),
        ("samplingMethod", "MONITOR".into()),
        ("samplingPeriod", java_double(s.period)),
        ("units", s.egu.to_string()),
        ("precision", s.prec.to_string()),
        ("lowerDisplayLimit", java_double(s.lopr)),
        ("upperDisplayLimit", java_double(s.hopr)),
        ("EGU", s.egu.to_string()),
        ("PREC", s.prec.to_string()),
        ("DESC", s.desc.to_string()),
        ("paused", "false".into()),
        ("applianceIdentity", "mockarchiver".into()),
    ];
    let items: Vec<String> = fields
        .iter()
        .map(|(k, v)| format!("{}: {}", json_str(k), json_str(v)))
        .collect();
    Reply::json(format!("{{ {} }}", items.join(", ")))
}
