//! Unit tests: URL building and decoding of fixtures in the appliance's
//! exact serialisation.

use super::*;

fn client() -> ArchiverClient {
    ArchiverClient::new("http://arch:17668").unwrap()
}

#[test]
fn url_normalisation() {
    let base = |u: &str| ArchiverClient::new(u).map(|c| c.base().to_string());
    assert_eq!(base("http://h:17668").unwrap(), "http://h:17668/retrieval");
    assert_eq!(base("http://h:17668/").unwrap(), "http://h:17668/retrieval");
    assert_eq!(
        base(" http://h:17668/retrieval/ ").unwrap(),
        "http://h:17668/retrieval"
    );
    assert_eq!(base("h:17668").unwrap(), "http://h:17668/retrieval");
    assert_eq!(base("https://h/arch").unwrap(), "https://h/arch/retrieval");
    // Phoebus's archive data source URL
    assert_eq!(
        base("pbraw://h:17668/retrieval").unwrap(),
        "http://h:17668/retrieval"
    );
    assert!(matches!(base(""), Err(Error::BadUrl(_))));
    assert!(matches!(base("ftp://h"), Err(Error::BadUrl(_))));
}

#[test]
fn data_url_encodes_operator_and_times() {
    let q = Query::new(
        "SR-DI-DCCT-01:SIGNAL",
        1_704_067_200.0,
        1_704_070_800.5,
        Operator::Optimized(1200),
    );
    assert_eq!(
        client().data_url(&q),
        "http://arch:17668/retrieval/data/getData.json?pv=optimized_1200%28SR-DI-DCCT-01%3ASIGNAL%29\
         &from=2024-01-01T00%3A00%3A00.000Z&to=2024-01-01T01%3A00%3A00.500Z"
    );
    let mut q = Query::new("a b+c", 0.0, 1.0, Operator::Raw);
    q.fetch_latest_metadata = true;
    let url = client().data_url(&q);
    assert!(url.contains("pv=a%20b%2Bc&"), "{url}");
    assert!(url.ends_with("&fetchLatestMetadata=true"));
}

#[test]
fn search_and_metadata_urls() {
    assert_eq!(
        client().search_url(&Search::from_user(" DCCT ", 50)),
        "http://arch:17668/retrieval/bpl/getMatchingPVs?regex=%28%3Fi%29.%2ADCCT.%2A&limit=50"
    );
    assert_eq!(
        client().search_url(&Search::regex("SR.*", -1)),
        "http://arch:17668/retrieval/bpl/getMatchingPVs?regex=SR.%2A&limit=-1"
    );
    assert_eq!(
        client().metadata_url("bl01t:SUM"),
        "http://arch:17668/retrieval/bpl/getMetadata?pv=bl01t%3ASUM"
    );
}

#[test]
fn search_from_user() {
    let p = |t: &str| {
        let s = Search::from_user(t, 5);
        assert_eq!(s.mode, SearchMode::Regex);
        s.pattern
    };
    // Plain text: anywhere, any case, metacharacters literal.
    assert_eq!(p("SR-DI"), "(?i).*SR-DI.*");
    assert_eq!(p("sr.di"), r"(?i).*sr\.di.*");
    assert_eq!(p("ES{Dif}Mtr"), r"(?i).*ES\{Dif\}Mtr.*");
    assert_eq!(p(""), "(?i).*.*");
    // A glob: the whole name, with case; only `*` and `?` are wild.
    assert_eq!(p("SR-DI*"), "SR-DI.*");
    assert_eq!(p("SR?DI"), "SR.DI");
    assert_eq!(p("XF:23ID1-ES{Dif-Ax:TH}*"), r"XF:23ID1-ES\{Dif-Ax:TH\}.*");
    assert_eq!(Search::exact("a(b)+c", 1), Search::regex(r"a\(b\)\+c", 1));
    assert_eq!(regex_escape(r"^a|b$[x]\"), r"\^a\|b\$\[x\]\\");
}

#[test]
fn operators() {
    assert_eq!(Operator::Raw.wrap("X"), "X");
    assert_eq!(Operator::Mean(600).wrap("X"), "mean_600(X)");
    assert_eq!(Operator::Nth(5).wrap("X"), "nth_5(X)");
    // One bin per pixel of the window fetched.
    assert_eq!(Operator::for_width(600.0), Operator::Optimized(600));
    assert_eq!(Operator::for_width(1000.0 * 1.5), Operator::Optimized(1500));
    assert_eq!(Operator::for_width(5000.0), Operator::Optimized(MAX_BINS));
    assert_eq!(Operator::for_width(0.0), Operator::Optimized(1));
}

/// The spec's example, byte for byte as `JSONResponse` writes it.
const SPEC: &str = r#"[
{ "meta": { "name": "SR-DI-DCCT-01:SIGNAL" , "EGU": "mA" , "PREC": "3" },
"data": [
{ "secs": 1700000000, "val": 300.123, "nanos": 120000000, "severity":0, "status":0 },
{ "secs": 1700000001, "val": 300.119, "nanos": 120000000, "severity":0, "status":0, "fields": { "cnxlostepsecs": "1699999990", "cnxregainedepsecs": "1700000001", "DESC": "Beam current"} }
] }
 ]"#;

#[test]
fn parses_the_spec_example() {
    let s = parse_data(SPEC).unwrap();
    assert_eq!(s.len(), 1);
    let s = &s[0];
    assert_eq!(s.meta.name, "SR-DI-DCCT-01:SIGNAL");
    assert_eq!(s.meta.egu.as_deref(), Some("mA"));
    assert_eq!(s.meta.prec, Some(3));
    assert_eq!(s.kind, ValueKind::Scalar);
    assert!(!s.binned);
    // The connection was lost before the first sample: nothing to break.
    assert_eq!(s.samples.len(), 2);
    assert!((s.samples[0].t - 1_700_000_000.12).abs() < 1e-6);
    assert_eq!(s.samples[0].val, 300.123);
    assert_eq!(s.samples[0].min, 300.123);
    assert_eq!(s.samples[0].count, 1);
    assert!(!s.samples.iter().any(Sample::is_gap));
    assert_eq!(s.samples[1].val, 300.119);
}

#[test]
fn empty_responses() {
    assert!(parse_data("[ ]").unwrap().is_empty());
    assert!(parse_data("").unwrap().is_empty());
    assert!(parse_data("  \n").unwrap().is_empty());
    let s = parse_data(r#"[{"meta":{"name":"X"},"data":[]}]"#).unwrap();
    assert!(s[0].samples.is_empty());
    assert!(matches!(parse_data("<html>"), Err(Error::Parse(_))));
    assert!(matches!(parse_data("42"), Err(Error::Parse(_))));
}

#[test]
fn nan_tokens_and_numeric_meta() {
    let body = r#"[{"meta":{"name":"X","PREC":2,"LOPR":"-5","HOPR":5.5,"DESC":"d","waveform":false,"HIHI":"9"},
        "data":[{"secs":10,"val":NaN,"nanos":0,"severity":0,"status":0},
                {"secs":11,"val":Infinity,"nanos":0,"severity":0,"status":0},
                {"secs":12,"val":-Infinity,"nanos":500000000,"severity":2,"status":3},
                {"secs":13,"val":1.0E-5,"nanos":0,"severity":1,"status":0}]}]"#;
    let s = &parse_data(body).unwrap()[0];
    assert_eq!(s.meta.prec, Some(2));
    assert_eq!(s.meta.lopr, Some(-5.0));
    assert_eq!(s.meta.hopr, Some(5.5));
    assert_eq!(s.meta.desc.as_deref(), Some("d"));
    assert_eq!(
        s.meta.extra.get("waveform").map(String::as_str),
        Some("false")
    );
    assert_eq!(s.meta.extra.get("HIHI").map(String::as_str), Some("9"));
    assert!(s.samples.iter().take(3).all(Sample::is_gap));
    assert_eq!(s.samples[2].t, 12.5);
    assert_eq!(s.samples[2].severity, Severity::Major);
    assert_eq!(s.samples[2].status, 3);
    assert_eq!(s.samples[3].val, 1e-5);
    assert_eq!(s.samples[3].severity, Severity::Minor);
}

#[test]
fn optimized_bins() {
    let body = r#"[
{ "meta": { "name": "X" , "PREC": "3" },
"data": [
{ "secs": 1703, "val": [1.5,0.5,1.0,2.0,4.0], "nanos": 500000000, "severity":0, "status":0 },
{ "secs": 1710, "val": [NaN,NaN,NaN,NaN,0.0], "nanos": 500000000, "severity":0, "status":0 },
{ "secs": 1717, "val": [3.0,0.0,3.0,3.0,1.0], "nanos": 500000000, "severity":2, "status":0, "fields": { "connectionChange": "true"} }
] }
 ]"#;
    let s = &parse_data(body).unwrap()[0];
    assert!(s.binned);
    assert_eq!(s.kind, ValueKind::Scalar);
    let b = s.samples[0];
    assert_eq!(
        (b.t, b.val, b.min, b.max, b.count),
        (1703.5, 1.5, 1.0, 2.0, 4)
    );
    assert!(s.samples[1].is_gap());
    assert_eq!(s.samples[1].count, 0);
    assert_eq!(s.samples[2].severity, Severity::Major);
}

#[test]
fn waveforms_strings_enums_and_objects() {
    let wave = r#"[{"meta":{"name":"W"},"data":[{"secs":1,"val":[4.0,5.0,6.0],"nanos":0,"severity":0,"status":0},
        {"secs":2,"val":[],"nanos":0,"severity":0,"status":0}]}]"#;
    let s = &parse_data(wave).unwrap()[0];
    assert_eq!(s.kind, ValueKind::Waveform);
    assert!(!s.binned);
    assert_eq!(s.samples[0].val, 4.0);
    assert!(s.samples[1].is_gap());

    // A 5-element waveform is only taken for bins when bins are allowed.
    let five = r#"[{"meta":{"name":"W"},"data":[{"secs":1,"val":[1,2,3,4,5],"nanos":0,"severity":0,"status":0}]}]"#;
    assert!(parse_data(five).unwrap()[0].binned);
    let raw = decode::data(five, false).unwrap();
    assert_eq!(raw[0].kind, ValueKind::Waveform);
    assert_eq!(raw[0].samples[0].val, 1.0);
    // ... and never when the server says it's a waveform.
    let hinted = five.replace(r#""name":"W""#, r#""name":"W","waveform":true"#);
    assert!(!parse_data(&hinted).unwrap()[0].binned);

    let text = r#"[{"meta":{"name":"S"},"data":[{"secs":1,"val":"Fault: \"door\" open, NaN","nanos":0,"severity":0,"status":0}]}]"#;
    let s = &parse_data(text).unwrap()[0];
    assert_eq!(s.kind, ValueKind::Text);
    assert!(s.samples[0].is_gap());

    let en =
        r#"[{"meta":{"name":"E"},"data":[{"secs":1,"val":2,"nanos":0,"severity":0,"status":0}]}]"#;
    let s = &parse_data(en).unwrap()[0];
    assert_eq!(s.kind, ValueKind::Scalar);
    assert_eq!(s.samples[0].val, 2.0);

    let obj = r#"[{"meta":{"name":"O"},"data":[{"secs":1,"val":{"a":1},"nanos":0,"severity":0,"status":0}]}]"#;
    assert_eq!(parse_data(obj).unwrap()[0].kind, ValueKind::Other);
}

#[test]
fn connection_gaps() {
    // The mock's (and the spec's) form: a severity-3 event carrying
    // `cnxlostepsecs` is itself the gap; the reconnect sample adds no second.
    let body = r#"[{"meta":{"name":"X"},"data":[
        {"secs":100,"val":1.0,"nanos":0,"severity":0,"status":0},
        {"secs":101,"val":1.0,"nanos":0,"severity":3,"status":9,"fields":{"cnxlostepsecs":"101"}},
        {"secs":160,"val":2.0,"nanos":0,"severity":0,"status":0,"fields":{"cnxlostepsecs":"101","cnxregainedepsecs":"160"}}]}]"#;
    let s = &parse_data(body).unwrap()[0];
    let gaps: Vec<f64> = s
        .samples
        .iter()
        .filter(|s| s.is_gap())
        .map(|s| s.t)
        .collect();
    assert_eq!(gaps, vec![101.0]);
    assert_eq!(s.samples.len(), 3);
    assert_eq!(s.samples[1].status, 9);

    // The engine's form: only the reconnect sample says when it was lost.
    let body = r#"[{"meta":{"name":"X"},"data":[
        {"secs":100,"val":1.0,"nanos":0,"severity":0,"status":0},
        {"secs":160,"val":2.0,"nanos":0,"severity":0,"status":0,"fields":{"cnxlostepsecs":"130","cnxregainedepsecs":"160","startup":"true"}}]}]"#;
    let s = &parse_data(body).unwrap()[0];
    assert_eq!(s.samples.len(), 3);
    assert!(s.samples[1].is_gap());
    assert_eq!(s.samples[1].t, 130.0);
    assert_eq!(s.samples[2].val, 2.0);
}

#[test]
fn tolerates_odd_samples_and_orders_them() {
    let body = r#"[{"meta":{"name":"X"},"data":[
        {"secs":"20","val":2.5,"severity":0,"status":0},
        {"val":9.0},
        {"secs":10,"val":true,"nanos":0},
        {"secs":30,"nanos":0,"severity":7,"status":99999}]}]"#;
    let s = &parse_data(body).unwrap()[0];
    assert_eq!(s.samples.len(), 3);
    assert_eq!(s.samples[0].t, 10.0);
    assert_eq!(s.samples[0].val, 1.0);
    assert_eq!(s.samples[1].val, 2.5);
    assert!(s.samples[2].is_gap());
    assert_eq!(s.samples[2].severity, Severity::Invalid);
    assert_eq!(s.samples[2].status, u16::MAX);
}

#[test]
fn names_and_metadata() {
    assert_eq!(parse_names(r#"["b","a"]"#).unwrap(), vec!["b", "a"]);
    assert!(parse_names("[]").unwrap().is_empty());
    assert!(parse_names("{}").is_err());
    let m = parse_metadata(
        r#"{"DBRType":"DBR_WAVEFORM_DOUBLE","elementCount":"8","units":"","EGU":"counts",
            "precision":"2","lowerDisplayLimit":"-1.0","upperDisplayLimit":"1.0E3","paused":"false","n":3}"#,
    )
    .unwrap();
    assert_eq!(m.dbr_type.as_deref(), Some("DBR_WAVEFORM_DOUBLE"));
    assert_eq!(m.element_count, Some(8));
    assert_eq!(m.units.as_deref(), Some("counts"));
    assert_eq!(m.precision, Some(2));
    assert_eq!(m.lower_display_limit, Some(-1.0));
    assert_eq!(m.upper_display_limit, Some(1000.0));
    assert_eq!(m.raw.get("n").map(String::as_str), Some("3"));
    assert!(!m.is_scalar_numeric());
    let m = parse_metadata(r#"{"DBRType":"DBR_SCALAR_ENUM","elementCount":"1"}"#).unwrap();
    assert!(m.is_scalar_numeric());
    assert!(!m.binnable(), "enums are fetched raw");
    let m = parse_metadata(r#"{"DBRType":"DBR_SCALAR_DOUBLE"}"#).unwrap();
    assert!(m.binnable());
    assert!(parse_metadata("[]").is_err());
}

#[test]
fn error_display() {
    let e = Error::Http {
        status: 404,
        message: "HTTP Status 404 – Not Found".into(),
    };
    assert!(e.is_not_archived());
    assert_eq!(e.to_string(), "HTTP 404: HTTP Status 404 – Not Found");
    assert!(!Error::Transport("x".into()).is_not_archived());
}

#[test]
fn long_waveforms_keep_element_zero_and_odd_items_are_skipped() {
    // Only the first elements are kept; the rest are skipped unparsed.
    let wave: Vec<String> = (0..10_000).map(|i| format!("{i}.5")).collect();
    let body = format!(
        r#"[{{"meta":{{"name":"W","waveform":"true"}},"data":[
            {{"secs":1,"val":[{}],"nanos":0}},
            {{"secs":2,"val":[[1,2],{{"a":1}},"3"],"nanos":0}},
            {{"secs":3,"val":["7.5",true],"nanos":0}}]}}]"#,
        wave.join(",")
    );
    let s = &parse_data(&body).unwrap()[0];
    assert_eq!(s.kind, ValueKind::Waveform);
    assert_eq!(s.samples[0].val, 0.5);
    assert!(s.samples[1].is_gap(), "a nested array is not a number");
    assert_eq!(s.samples[2].val, 7.5);

    // Items and samples that are not objects are skipped, as is a `data`
    // that is not an array.
    let body = r#"[1, "x", null, {"meta":{"name":"A"},"data":[3, {"secs":5,"val":2}]},
        {"meta":{"name":"B"},"data":"none"}]"#;
    let s = parse_data(body).unwrap();
    assert_eq!(s.len(), 2);
    assert_eq!(s[0].samples.len(), 1);
    assert_eq!(s[0].samples[0].val, 2.0);
    assert!(s[1].samples.is_empty());
    // A single object rather than an array.
    let s = parse_data(r#"{"meta":{"name":"A"},"data":[{"secs":5,"val":2}]}"#).unwrap();
    assert_eq!(s[0].meta.name, "A");
    assert!(parse_data("null").unwrap().is_empty());
}
