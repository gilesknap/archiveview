//! Unit tests of the mock's endpoints, driven through `route` without HTTP.

use super::*;
use data::{Op, Stat, java_double, parse_op, split_pv};
use serde_json::Value as Json;

/// 2024-01-01T00:00:00Z.
const NOW: i64 = 1_704_067_200;

fn clock() -> Clock {
    Clock::fixed(NOW * 1_000_000_000)
}

fn get(url: &str) -> (u16, String) {
    let r = route(url, &clock());
    let status = r.status;
    (status, r.text())
}

fn json(url: &str) -> Json {
    let (status, body) = get(url);
    assert_eq!(status, 200, "{url}: {body}");
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("{url}: {e}\n{body}"))
}

fn data(pv: &str, from: &str, to: &str) -> Vec<Json> {
    let url = format!("/retrieval/data/getData.json?pv={pv}&from={from}&to={to}");
    let v = json(&url);
    let a = v.as_array().unwrap();
    if a.is_empty() {
        return Vec::new();
    }
    a[0]["data"].as_array().unwrap().clone()
}

fn secs(s: &Json) -> i64 {
    s["secs"].as_i64().unwrap()
}

#[test]
fn raw_includes_the_sample_before_from() {
    let d = data(
        "MOCK:SINE",
        "2023-12-31T23:00:00.500Z",
        "2023-12-31T23:00:10Z",
    );
    assert_eq!(d.len(), 11);
    assert_eq!(secs(&d[0]), NOW - 3600);
    assert_eq!(secs(&d[1]), NOW - 3600 + 1);
    assert_eq!(secs(&d[10]), NOW - 3600 + 10);
    assert!(d[0]["val"].is_f64());
    assert_eq!(d[0]["nanos"], 0);
}

#[test]
fn raw_with_a_sample_at_from_has_no_extra() {
    let d = data("MOCK:SINE", "2023-12-31T23:00:00Z", "2023-12-31T23:00:10Z");
    assert_eq!(d.len(), 11);
    assert_eq!(secs(&d[0]), NOW - 3600);
}

#[test]
fn raw_stops_at_now_and_starts_at_history() {
    let d = data("MOCK:SLOW", "2023-12-31T23:00:00Z", "2024-01-01T05:00:00Z");
    assert_eq!(secs(d.last().unwrap()), NOW);
    assert_eq!(d.len(), 13);
    // Before history: nothing at all.
    let v = json(
        "/retrieval/data/getData.json?pv=MOCK:SINE&from=2020-01-01T00:00:00Z&to=2020-01-02T00:00:00Z",
    );
    assert_eq!(v, serde_json::json!([]));
    // After now: just the last value, before `from`.
    let d = data("MOCK:SLOW", "2024-01-02T00:00:00Z", "2024-01-03T00:00:00Z");
    assert_eq!(d.len(), 1);
    assert_eq!(secs(&d[0]), NOW);
}

#[test]
fn meta_has_string_prec_and_optional_egu() {
    let v = json(
        "/retrieval/data/getData.json?pv=MOCK:SINE&from=2023-12-31T23:00:00Z&to=2023-12-31T23:00:01Z",
    );
    assert_eq!(v[0]["meta"]["name"], "MOCK:SINE");
    assert_eq!(v[0]["meta"]["EGU"], "mA");
    assert_eq!(v[0]["meta"]["PREC"], "3");
    assert!(v[0]["meta"].get("DESC").is_none());
    let v = json(
        "/retrieval/data/getData.json?pv=bl01t%3ASUM&from=2023-12-31T23:00:00Z&to=2023-12-31T23:00:01Z&fetchLatestMetadata=true",
    );
    assert!(v[0]["meta"].get("EGU").is_none());
    assert_eq!(v[0]["meta"]["DESC"], "Sum A and B");
    assert_eq!(v[0]["meta"]["HOPR"], "20.0");
}

#[test]
fn optimized_returns_raw_when_few_samples() {
    let d = data(
        "optimized_1000(MOCK:SLOW)",
        "2023-12-31T00:00:00Z",
        "2024-01-01T00:00:00Z",
    );
    assert_eq!(d.len(), 289);
    assert!(d.iter().all(|s| s["val"].is_f64()));
}

#[test]
fn optimized_bins_are_centred_counted_and_aligned() {
    let d = data(
        "optimized_100(MOCK:SINE)",
        "2023-12-31T23:00:00Z",
        "2024-01-01T00:00:00Z",
    );
    // I = 3600 / 100 = 36 s; bins from/36 ..= to/36.
    assert_eq!(d.len(), 101);
    let b0 = (NOW - 3600) / 36;
    for (i, s) in d.iter().enumerate() {
        assert_eq!(secs(s), (b0 + i as i64) * 36 + 18);
        assert_eq!(s["nanos"], 0);
        let v = s["val"].as_array().unwrap();
        assert_eq!(v.len(), 5);
        let (mean, std, min, max, n) = (
            v[0].as_f64().unwrap(),
            v[1].as_f64().unwrap(),
            v[2].as_f64().unwrap(),
            v[3].as_f64().unwrap(),
            v[4].as_f64().unwrap(),
        );
        assert!(min <= mean && mean <= max && std >= 0.0);
        assert_eq!(n, if i == 100 { 1.0 } else { 36.0 });
    }
}

#[test]
fn optimized_odd_interval_has_half_second_nanos() {
    let d = data(
        "optimized_514(MOCK:SINE)",
        "2023-12-31T23:00:00Z",
        "2024-01-01T00:00:00Z",
    );
    assert_eq!(d[0]["nanos"], 500_000_000);
    assert_eq!(secs(&d[0]) % 7, 3);
}

#[test]
fn optimized_puts_the_sample_before_from_in_its_own_bin() {
    let d = data(
        "optimized_100(MOCK:SINE)",
        "2023-12-31T23:00:00.500Z",
        "2024-01-01T00:00:00Z",
    );
    let b0 = (NOW - 3600) / 36;
    assert_eq!(secs(&d[0]), (b0 - 1) * 36 + 18);
    assert_eq!(d[0]["val"][4], 1.0);
    assert_eq!(d[1]["val"][4], 35.0);
}

#[test]
fn fill_and_no_fill_variants() {
    // MOCK:SLOW has one sample per 5 minutes; 1-minute bins are mostly empty.
    let filled = data(
        "mean_60(MOCK:SLOW)",
        "2023-12-31T23:00:00Z",
        "2024-01-01T00:00:00Z",
    );
    let sparse = data(
        "meanSample_60(MOCK:SLOW)",
        "2023-12-31T23:00:00Z",
        "2024-01-01T00:00:00Z",
    );
    assert_eq!(filled.len(), 61);
    assert_eq!(sparse.len(), 13);
    assert_eq!(filled[1]["val"], filled[0]["val"]);
    assert!(sparse.iter().all(|s| secs(s) % 300 == 30));
}

#[test]
fn bins_fill_past_now_to_the_requested_end() {
    let d = data(
        "mean_600(MOCK:SLOW)",
        "2023-12-31T23:00:00Z",
        "2024-01-01T01:00:00Z",
    );
    assert_eq!(secs(d.last().unwrap()), NOW + 3600 + 300);
}

#[test]
fn op_forms_are_equivalent() {
    let a = get("/retrieval/data/getData.json?pv=mean_600(MOCK:SINE)&from=2023-12-31T20:00:00Z&to=2024-01-01T00:00:00Z").1;
    let b = get("/retrieval/data/getData.json?pv=mean(MOCK:SINE,600)&from=2023-12-31T20:00:00Z&to=2024-01-01T00:00:00Z").1;
    let c = get("/retrieval/data/getData.json?pv=MOCK:SINE&pp=mean_600&from=2023-12-31T20:00:00Z&to=2024-01-01T00:00:00Z").1;
    assert_eq!(a, b);
    assert_eq!(a, c);
    // An unknown operator means raw data.
    let raw = data("MOCK:SINE", "2023-12-31T23:00:00Z", "2023-12-31T23:01:00Z");
    let unknown = data(
        "frobnicate_7(MOCK:SINE)",
        "2023-12-31T23:00:00Z",
        "2023-12-31T23:01:00Z",
    );
    assert_eq!(raw, unknown);
}

#[test]
fn nth_and_ncount() {
    let d = data(
        "nth_10(MOCK:SINE)",
        "2023-12-31T23:00:00Z",
        "2023-12-31T23:01:39Z",
    );
    assert_eq!(d.len(), 10);
    assert_eq!(secs(&d[1]) - secs(&d[0]), 10);
    let d = data(
        "ncount(MOCK:SINE)",
        "2023-12-31T23:00:00Z",
        "2023-12-31T23:01:39Z",
    );
    assert_eq!(d.len(), 1);
    assert_eq!(d[0]["val"], 100);
}

#[test]
fn stats_operators() {
    let range = ("2023-12-31T23:00:00Z", "2023-12-31T23:09:59Z");
    let one = |op: &str| {
        data(&format!("{op}_600(MOCK:SINE)"), range.0, range.1)[0]["val"]
            .as_f64()
            .unwrap()
    };
    let (mean, min, max, count) = (one("mean"), one("min"), one("max"), one("count"));
    assert!(min < mean && mean < max);
    assert_eq!(count, 600.0);
    assert!((one("variance").sqrt() - one("std")).abs() < 1e-9);
    let med = one("median");
    assert!(min <= med && med <= max);
    let raw = data("MOCK:SINE", range.0, range.1);
    assert_eq!(one("firstSample"), raw[0]["val"].as_f64().unwrap());
    assert_eq!(
        one("lastSample"),
        raw.last().unwrap()["val"].as_f64().unwrap()
    );
}

#[test]
fn nan_tokens_and_gaps_on_the_wire() {
    let (status, body) = get(
        "/retrieval/data/getData.json?pv=MOCK:NAN&from=2023-12-31T22:00:00Z&to=2024-01-01T00:00:00Z",
    );
    assert_eq!(status, 200);
    assert!(body.contains("\"val\": NaN"));
    assert!(body.contains("\"severity\":3, \"status\":9, \"fields\": { \"cnxlostepsecs\": "));
    assert!(body.contains("\"cnxregainedepsecs\""));
    assert!(serde_json::from_str::<Json>(&body).is_err());
    // Bins skip NaNs; a bin of only gaps would be [NaN,NaN,NaN,NaN,0.0].
    let (_, body) = get(
        "/retrieval/data/getData.json?pv=optimized_100(MOCK:NAN)&from=2023-12-31T22:00:00Z&to=2024-01-01T00:00:00Z",
    );
    assert!(body.contains("\"val\": ["));
}

#[test]
fn java_number_formats() {
    assert_eq!(java_double(300.0), "300.0");
    assert_eq!(java_double(300.123), "300.123");
    assert_eq!(java_double(1e-5), "1.0E-5");
    assert_eq!(java_double(3e10), "3.0E10");
    assert_eq!(java_double(-2.5e-7), "-2.5E-7");
    assert_eq!(java_double(0.0), "0.0");
    assert_eq!(java_double(f64::NAN), "NaN");
    assert_eq!(java_double(f64::NEG_INFINITY), "-Infinity");
    let (_, body) = get(
        "/retrieval/data/getData.json?pv=MOCK:BIG&from=2023-12-31T23:00:00Z&to=2023-12-31T23:00:10Z",
    );
    assert!(body.contains("E10,") || body.contains("E9,"), "{body}");
}

#[test]
fn other_value_types() {
    let d = data("MOCK:ENUM", "2023-12-31T23:00:00Z", "2023-12-31T23:05:00Z");
    assert!(d[0]["val"].is_i64());
    let d = data(
        "MOCK:STRING",
        "2023-12-31T23:00:00Z",
        "2023-12-31T23:30:00Z",
    );
    assert!(d.iter().any(|s| s["val"] == "Fault: \"door\" open"));
    let d = data("MOCK:WAVE", "2023-12-31T23:00:00Z", "2023-12-31T23:01:00Z");
    assert_eq!(d[0]["val"].as_array().unwrap().len(), 8);
    // Summaries of non-numeric PVs come back raw.
    let d = data(
        "optimized_10(MOCK:WAVE)",
        "2023-12-31T23:00:00Z",
        "2023-12-31T23:10:00Z",
    );
    assert_eq!(d[0]["val"].as_array().unwrap().len(), 8);
    assert_eq!(d.len(), 61);
}

#[test]
fn data_errors() {
    let base = "/retrieval/data/getData.json";
    let (s, body) = get(&format!(
        "{base}?pv=NOPE&from=2023-12-31T23:00:00Z&to=2024-01-01T00:00:00Z"
    ));
    assert_eq!(s, 404);
    assert!(body.contains("<title>HTTP Status 404"));
    assert_eq!(get(&format!("{base}?pv=MOCK:SINE&from=yesterday")).0, 400);
    assert_eq!(
        get(&format!(
            "{base}?pv=MOCK:SINE&from=2024-01-01T00:00:00Z&to=2023-12-31T00:00:00Z"
        ))
        .0,
        400
    );
    assert_eq!(get(&format!("{base}?pv=mean_600(MOCK:SINE")).0, 400);
    assert_eq!(get(&format!("{base}?pv=mean_x(MOCK:SINE)")).0, 400);
    assert_eq!(get(base).0, 400);
    assert_eq!(get("/retrieval/data/getData.csv?pv=MOCK:SINE").0, 404);
    // Offsets are accepted, as by `ISO_OFFSET_DATE_TIME`.
    let d = data(
        "MOCK:SINE",
        "2023-12-31T18:00:00-05:00",
        "2023-12-31T18:00:05-05:00",
    );
    assert_eq!(secs(&d[0]), NOW - 3600);
    // Defaults: the last day up to now + 1h.
    let d = json("/retrieval/data/getData.json?pv=MOCK:SLOW");
    assert_eq!(d[0]["data"].as_array().unwrap().len(), 277);
}

#[test]
fn op_parsing_follows_registration_order() {
    assert_eq!(
        parse_op("optimized_1200"),
        Ok(Op::Optimized {
            points: 1200,
            fill: true,
            last: false
        })
    );
    assert_eq!(
        parse_op("optimizedLastSample_10"),
        Ok(Op::Optimized {
            points: 10,
            fill: true,
            last: true
        })
    );
    assert_eq!(
        parse_op("meanSample_600"),
        Ok(Op::Stat {
            stat: Stat::Mean,
            secs: 600,
            fill: false
        })
    );
    assert_eq!(
        parse_op("firstSample_60"),
        Ok(Op::Stat {
            stat: Stat::First,
            secs: 60,
            fill: true
        })
    );
    assert_eq!(
        parse_op("lastSampleSample_60"),
        Ok(Op::Stat {
            stat: Stat::Last,
            secs: 60,
            fill: false
        })
    );
    assert_eq!(
        parse_op("mean"),
        Ok(Op::Stat {
            stat: Stat::Mean,
            secs: 900,
            fill: true
        })
    );
    assert_eq!(parse_op("ncount"), Ok(Op::NCount));
    assert_eq!(parse_op("jitter_5"), Ok(Op::Raw));
    assert!(parse_op("optimized_lots").is_err());
    assert_eq!(split_pv("A:B"), Some(("A:B".into(), None)));
    assert_eq!(
        split_pv("mean(A:B, 60)"),
        Some(("A:B".into(), Some("mean_60".into())))
    );
    assert_eq!(split_pv("mean(A:B"), None);
    assert_eq!(split_pv("a(b(c))"), None);
}

#[test]
fn matching_pvs_glob_regex_and_limits() {
    let names = |q: &str| json(&format!("/retrieval/bpl/getMatchingPVs?{q}"));
    assert_eq!(
        names("pv=MOCK:S*"),
        serde_json::json!(["MOCK:SINE", "MOCK:SLOW", "MOCK:STEP", "MOCK:STRING"])
    );
    // Whole-name match: no implicit substring.
    assert_eq!(names("pv=MOCK"), serde_json::json!([]));
    assert_eq!(names("pv=*SUM*"), serde_json::json!(["bl01t:SUM"]));
    // The naive glob leaves `.` as a regex wildcard.
    assert_eq!(
        names("pv=BL01T.MO-SIMC-01:M1.RBV")
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        names("regex=bl01t:[AB]"),
        serde_json::json!(["bl01t:A", "bl01t:B"])
    );
    assert_eq!(names("pv=*&limit=2").as_array().unwrap().len(), 2);
    assert_eq!(
        names("pv=*&limit=-1").as_array().unwrap().len(),
        signals::all().len()
    );
    // Sorted (uppercase before lowercase).
    let all = names("pv=*");
    let v: Vec<&str> = all
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect();
    let mut sorted = v.clone();
    sorted.sort();
    assert_eq!(v, sorted);
    // `pv` wins over `regex`.
    assert_eq!(
        names("pv=MOCK:SINE&regex=bl01t:A"),
        serde_json::json!(["MOCK:SINE"])
    );
    assert_eq!(get("/retrieval/bpl/getMatchingPVs").0, 404);
    assert_eq!(get("/retrieval/bpl/getMatchingPVs?regex=(").0, 500);
}

#[test]
fn all_pvs_real_globs_unsorted() {
    let names = |q: &str| json(&format!("/mgmt/bpl/getAllPVs?{q}"));
    let all = names("");
    assert_eq!(all.as_array().unwrap().len(), signals::all().len());
    assert_eq!(all[0], signals::all()[0].name);
    assert_eq!(
        names("pv=MOCK:NAN,bl01t:?"),
        serde_json::json!(["MOCK:NAN", "bl01t:A", "bl01t:B"])
    );
    // A real glob: `.` is literal.
    assert_eq!(names("pv=BL01T.MO-SIMC-01:M1.RBV"), serde_json::json!([]));
    assert_eq!(
        names("regex=MOCK:.*E&limit=2"),
        serde_json::json!(["MOCK:SINE", "MOCK:NOISE"])
    );
}

#[test]
fn metadata() {
    let m = json("/retrieval/bpl/getMetadata?pv=MOCK:WAVE");
    assert_eq!(m["DBRType"], "DBR_WAVEFORM_DOUBLE");
    assert_eq!(m["elementCount"], "8");
    assert_eq!(m["units"], "counts");
    let m = json("/retrieval/bpl/getMetadata?pv=MOCK:SINE");
    assert_eq!(m["DBRType"], "DBR_SCALAR_DOUBLE");
    assert_eq!(m["precision"], "3");
    assert_eq!(m["upperDisplayLimit"], "300.0");
    assert_eq!(get("/retrieval/bpl/getMetadata?pv=NOPE").0, 404);
}

#[test]
fn params_decode_like_a_servlet() {
    let p = Params::parse("pv=a%3Ab+c&x=%zz&y=%E2%80%93&empty");
    assert_eq!(p.get("pv"), Some("a:b c"));
    assert_eq!(p.get("x"), Some("%zz"));
    assert_eq!(p.get("y"), Some("\u{2013}"));
    assert_eq!(p.get("empty"), Some(""));
    assert_eq!(p.get("nope"), None);
}

#[test]
fn serves_over_http() {
    let server = MockArchiver::start_local().unwrap();
    assert!(server.url().starts_with("http://127.0.0.1:"));
    let mut s = std::net::TcpStream::connect(server.addr()).unwrap();
    use std::io::Write;
    write!(
        s,
        "GET /retrieval/bpl/getMatchingPVs?pv=MOCK:SINE HTTP/1.0\r\nHost: x\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    assert!(
        out.starts_with("HTTP/1.1 200") || out.starts_with("HTTP/1.0 200"),
        "{out}"
    );
    assert!(out.ends_with("[\"MOCK:SINE\"]"), "{out}");
    drop(server);
}
