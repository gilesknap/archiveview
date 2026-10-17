//! The client against an in-process mock appliance, over real HTTP.

use std::time::{SystemTime, UNIX_EPOCH};

use archiver::{ArchiverClient, Error, Operator, Query, Search, Severity, ValueKind};
use mockarchiver::MockArchiver;

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

fn setup() -> (MockArchiver, ArchiverClient) {
    let mock = MockArchiver::start_local().unwrap();
    let client = ArchiverClient::new(&mock.url())
        .unwrap()
        .with_timeout_secs(30);
    (mock, client)
}

#[test]
fn raw_data_with_meta() {
    let (_mock, c) = setup();
    let t = now().floor() - 3600.0;
    let q = Query::new("MOCK:SINE", t - 59.5, t, Operator::Raw);
    let s = c.get_data(&q).unwrap().unwrap();
    assert_eq!(s.meta.name, "MOCK:SINE");
    assert_eq!(s.meta.egu.as_deref(), Some("mA"));
    assert_eq!(s.meta.prec, Some(3));
    assert_eq!(s.kind, ValueKind::Scalar);
    assert!(!s.binned);
    // 60 samples in range plus the one that held at `from`.
    assert_eq!(s.samples.len(), 61);
    assert!(s.samples[0].t < q.from);
    assert!(s.samples.windows(2).all(|w| w[1].t - w[0].t == 1.0));
    assert!(s.samples.iter().all(|s| (200.0..300.0).contains(&s.val)));
    // The sine raises a MINOR alarm near its top.
    let q = Query::new("MOCK:SINE", t - 600.0, t, Operator::Raw);
    let s = c.get_data(&q).unwrap().unwrap();
    assert!(s.samples.iter().any(|s| s.severity == Severity::Minor));
}

#[test]
fn latest_metadata() {
    let (_mock, c) = setup();
    let t = now() - 60.0;
    let mut q = Query::new("MOCK:SINE", t - 10.0, t, Operator::Raw);
    q.fetch_latest_metadata = true;
    let s = c.get_data(&q).unwrap().unwrap();
    assert_eq!(s.meta.lopr, Some(200.0));
    assert_eq!(s.meta.hopr, Some(300.0));
    assert!(s.meta.desc.is_some());
}

#[test]
fn optimized_bins_a_busy_pv() {
    let (_mock, c) = setup();
    let to = now();
    let q = Query::new("MOCK:NOISE", to - 86400.0, to, Operator::for_width(1200.0));
    let s = c.get_data(&q).unwrap().unwrap();
    assert!(s.binned);
    assert_eq!(s.kind, ValueKind::Scalar);
    // I = 86400 / 1200 = 72 s: about 1200 bins.
    assert!(
        (1199..=1203).contains(&s.samples.len()),
        "{}",
        s.samples.len()
    );
    let full: Vec<_> = s.samples.iter().filter(|b| b.count == 720).collect();
    assert!(full.len() > 1100);
    for b in &s.samples {
        assert!(b.min <= b.val && b.val <= b.max, "{b:?}");
        assert_eq!((b.t - 36.0) % 72.0, 0.0);
    }
}

#[test]
fn optimized_is_raw_for_a_quiet_pv() {
    let (_mock, c) = setup();
    let to = now();
    let q = Query::new("MOCK:SLOW", to - 86400.0, to, Operator::Optimized(1000));
    let s = c.get_data(&q).unwrap().unwrap();
    assert!(!s.binned);
    assert!((288..=290).contains(&s.samples.len()));
    assert!(s.samples.iter().all(|s| s.count == 1 && s.min == s.max));
}

#[test]
fn mean_and_nth() {
    let (_mock, c) = setup();
    let to = (now() / 600.0).floor() * 600.0 - 3600.0;
    let s = c
        .get_data(&Query::new(
            "MOCK:SINE",
            to - 3600.0,
            to,
            Operator::Mean(600),
        ))
        .unwrap()
        .unwrap();
    assert!(!s.binned);
    assert_eq!(s.samples.len(), 7);
    assert_eq!(s.samples[0].t % 600.0, 300.0);
    // A 10-minute mean of a 10-minute sine sits at its centre.
    assert!(
        (s.samples[0].val - 250.0).abs() < 1.0,
        "{}",
        s.samples[0].val
    );
    let s = c
        .get_data(&Query::new(
            "MOCK:SINE",
            to - 100.0,
            to - 1.0,
            Operator::Nth(10),
        ))
        .unwrap()
        .unwrap();
    assert_eq!(s.samples.len(), 10);
}

#[test]
fn nan_tokens_and_disconnects() {
    let (_mock, c) = setup();
    let to = now();
    let s = c
        .get_data(&Query::new("MOCK:NAN", to - 7200.0, to, Operator::Raw))
        .unwrap()
        .unwrap();
    let gaps = s.samples.iter().filter(|s| s.is_gap()).count();
    // A NaN every 50 samples, plus 3-4 disconnects in two hours.
    assert!(gaps > 100, "{gaps}");
    let lost: Vec<_> = s
        .samples
        .iter()
        .filter(|s| s.is_gap() && s.severity == Severity::Invalid && s.status == 9)
        .collect();
    assert!((3..=4).contains(&lost.len()), "{}", lost.len());
    // Each is followed by a 60 s hole.
    for g in lost {
        let next = s.samples.iter().find(|s| s.t > g.t).unwrap();
        assert_eq!(next.t - g.t, 60.0);
    }
    // Optimized over the same range parses too (NaN inside bins).
    let s = c
        .get_data(&Query::new(
            "MOCK:NAN",
            to - 7200.0,
            to,
            Operator::Optimized(100),
        ))
        .unwrap()
        .unwrap();
    assert!(s.binned);
    assert!(s.samples.iter().all(|b| b.count <= 72));
}

#[test]
fn other_value_kinds() {
    let (_mock, c) = setup();
    let to = now();
    let get = |pv: &str, op| {
        c.get_data(&Query::new(pv, to - 3600.0, to, op))
            .unwrap()
            .unwrap()
    };
    let w = get("MOCK:WAVE", Operator::Raw);
    assert_eq!(w.kind, ValueKind::Waveform);
    // Even when asked for optimized: the mock returns waveforms raw, and
    // their 8-element arrays are not bins.
    let w = get("MOCK:WAVE", Operator::Optimized(10));
    assert_eq!(w.kind, ValueKind::Waveform);
    assert!(!w.binned);
    let t = get("MOCK:STRING", Operator::Raw);
    assert_eq!(t.kind, ValueKind::Text);
    assert!(t.samples.iter().all(|s| s.is_gap()));
    let e = get("MOCK:ENUM", Operator::Raw);
    assert_eq!(e.kind, ValueKind::Scalar);
    assert!(e.samples.iter().all(|s| (0.0..=3.0).contains(&s.val)));
    let b = get("MOCK:BIG", Operator::Raw);
    assert!(b.samples.iter().any(|s| s.val.abs() > 1e10));
}

#[test]
fn no_data_and_errors() {
    let (_mock, c) = setup();
    let to = now();
    // Before the mock's history: `[ ]`.
    let q = Query::new(
        "MOCK:SINE",
        to - 90.0 * 86400.0,
        to - 80.0 * 86400.0,
        Operator::Raw,
    );
    assert_eq!(c.get_data(&q).unwrap(), None);
    // Not archived.
    let e = c
        .get_data(&Query::new("NOT:ARCHIVED", to - 60.0, to, Operator::Raw))
        .unwrap_err();
    assert!(e.is_not_archived(), "{e}");
    assert_eq!(
        e.to_string(),
        "HTTP 404: HTTP Status 404 \u{2013} Not Found"
    );
    // Backwards range.
    let e = c
        .get_data(&Query::new("MOCK:SINE", to, to - 60.0, Operator::Raw))
        .unwrap_err();
    assert!(matches!(e, Error::Http { status: 400, .. }), "{e}");
}

#[test]
fn search_and_metadata() {
    let (_mock, c) = setup();
    assert_eq!(
        c.search(&Search::from_user("SIMC", 50)).unwrap(),
        vec!["BL01T-MO-SIMC-01:M1.RBV", "BL01T-MO-SIMC-01:M2.RBV"]
    );
    assert_eq!(
        c.search(&Search::regex("bl01t:(A|SUM)", -1)).unwrap(),
        vec!["bl01t:A", "bl01t:SUM"]
    );
    assert_eq!(c.search(&Search::from_user("MOCK:*", 3)).unwrap().len(), 3);
    // Plain text ignores case; a glob keeps it.
    assert!(
        c.search(&Search::from_user("sine", 50))
            .unwrap()
            .contains(&"MOCK:SINE".to_string())
    );
    assert!(
        c.search(&Search::from_user("mock:*", 50))
            .unwrap()
            .is_empty()
    );
    // Regex metacharacters are literal, not a server error.
    for text in ["ES{Dif}Mtr", "a(b", "x+", "[", "XF:23ID1-ES{Dif}*"] {
        assert_eq!(c.search(&Search::from_user(text, 50)), Ok(vec![]), "{text}");
    }
    assert_eq!(
        c.search(&Search::exact("MOCK:SINE", 1)).unwrap(),
        vec!["MOCK:SINE"]
    );
    assert!(c.search(&Search::exact("MOCK:SIN", 1)).unwrap().is_empty());
    assert!(
        c.search(&Search::from_user("nothing-here", 50))
            .unwrap()
            .is_empty()
    );
    let e = c.search(&Search::regex("(", 5)).unwrap_err();
    assert!(matches!(e, Error::Http { status: 500, .. }), "{e}");

    let m = c.metadata("MOCK:WAVE").unwrap();
    assert_eq!(m.element_count, Some(8));
    assert!(!m.is_scalar_numeric());
    let m = c.metadata("MOCK:SINE").unwrap();
    assert!(m.is_scalar_numeric());
    assert_eq!(m.units.as_deref(), Some("mA"));
    assert_eq!(m.precision, Some(3));
    assert!(!c.metadata("MOCK:STRING").unwrap().is_scalar_numeric());
    assert!(!c.metadata("MOCK:ENUM").unwrap().binnable());
    assert!(c.metadata("MOCK:SINE").unwrap().binnable());
    assert!(c.metadata("NOPE").unwrap_err().is_not_archived());
    assert!(c.version().unwrap().contains("mock"));
}

#[test]
fn large_streamed_raw_response() {
    let (_mock, c) = setup();
    let to = now();
    let s = c
        .get_data(&Query::new("MOCK:NOISE", to - 7200.0, to, Operator::Raw))
        .unwrap()
        .unwrap();
    assert!(
        (71_990..=72_010).contains(&s.samples.len()),
        "{}",
        s.samples.len()
    );
}

#[test]
fn url_with_retrieval_suffix_and_shared_between_threads() {
    let (mock, _) = setup();
    let c = ArchiverClient::new(&format!("{}/retrieval/", mock.url())).unwrap();
    let to = now();
    let handles: Vec<_> = ["MOCK:SINE", "MOCK:RAMP", "bl01t:SUM", "MOCK:STEP"]
        .into_iter()
        .map(|pv| {
            let c = c.clone();
            std::thread::spawn(move || {
                let q = Query::new(pv, to - 3600.0, to, Operator::Optimized(200));
                c.get_data(&q).unwrap().unwrap()
            })
        })
        .collect();
    for h in handles {
        assert!(!h.join().unwrap().samples.is_empty());
    }
}

#[test]
fn transport_errors() {
    // Bind and drop a mock to find a port nobody is listening on.
    let url = MockArchiver::start_local().unwrap().url();
    let c = ArchiverClient::new(&url).unwrap().with_timeout_secs(2);
    let e = c.search(&Search::from_user("x", 1)).unwrap_err();
    assert!(matches!(e, Error::Transport(_)), "{e}");
    assert!(e.to_string().contains("/retrieval"), "{e}");
}
