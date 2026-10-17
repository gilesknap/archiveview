//! Value axes: which axis a new trace joins, and what each axis is called.
//!
//! A trace settles on an axis once, when its first data arrives (see
//! DESIGN.md "Value axes"): it joins the existing axis whose units and
//! values suit it best, in whichever pane, or gets a new one, in a new
//! pane when the others are full. Nothing here moves a trace again; only
//! the user does, from its chip.
//!
//! Pure functions over [`Stats`] and names, so the rules are table-tested.

/// A pane holds at most this many axes of values (axes of states aside):
/// a trace that would need another opens a new pane. Soft: the user may
/// put more in one by hand.
pub const SOFT_LIMIT: usize = 4;

/// What a trace's (or an axis's) values look like, for choosing an axis.
#[derive(Clone, Debug, PartialEq)]
pub struct Stats {
    /// Units (`EGU`), empty when it has none.
    pub units: String,
    /// The bulk of the values: the 5th and 95th percentiles, so a spike
    /// or a glitch to zero does not decide the scale.
    pub lo: f64,
    pub hi: f64,
    /// The smallest and largest values.
    pub min: f64,
    pub max: f64,
    /// Every value is above zero (a log scale can show them all).
    pub positive: bool,
    /// Whole numbers: an enum or binary PV, drawn as states.
    pub whole: bool,
    /// An enum's state names, when known (from the live PV).
    pub states: Vec<String>,
}

impl Stats {
    /// The stats of `values` (finite ones; `None` with none).
    pub fn of(values: impl Iterator<Item = f64>, units: &str, whole: bool) -> Option<Self> {
        let mut v: Vec<f64> = values.filter(|v| v.is_finite()).collect();
        if v.is_empty() {
            return None;
        }
        v.sort_by(f64::total_cmp);
        let at = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
        Some(Self {
            units: units.to_string(),
            lo: at(0.05),
            hi: at(0.95),
            min: v[0],
            max: v[v.len() - 1],
            positive: v[0] > 0.0,
            whole,
            states: Vec::new(),
        })
    }

    /// The stats of an axis holding traces with stats `members`.
    pub fn merge<'a>(members: impl IntoIterator<Item = &'a Stats>) -> Option<Self> {
        members.into_iter().cloned().reduce(|a, b| Self {
            // An axis keeps its first units; mixed only by hand.
            units: if a.units.is_empty() { b.units } else { a.units },
            lo: a.lo.min(b.lo),
            hi: a.hi.max(b.hi),
            min: a.min.min(b.min),
            max: a.max.max(b.max),
            positive: a.positive && b.positive,
            whole: a.whole && b.whole,
            states: a.states,
        })
    }

    /// The typical size of the values: how far from zero the scale must
    /// reach. Zero for values that are all zero.
    pub fn magnitude(&self) -> f64 {
        self.lo.abs().max(self.hi.abs()).max(self.hi - self.lo)
    }

    /// Best on a log scale: positive values in units of pressure (vacuum
    /// spans decades, if not always in one window), or spanning several
    /// decades: two in the bulk of the values, or three in all.
    pub fn wants_log(&self) -> bool {
        self.positive
            && !self.whole
            && self.min > 0.0
            && (quantity(&self.units) == Some("Pressure")
                || self.hi / self.lo >= 100.0
                || self.max / self.min >= 1000.0)
    }
}

/// How well a trace with stats `t` fits an axis with stats `a`: `None`
/// when it never may (different units, states among values, states of
/// another enum), else a score where up to 1 is a match and lower is
/// better.
///
/// The score is how many decades apart the two typical sizes are, over
/// the decades allowed: one, and a little more for matching units and for
/// ranges that overlap. Enums share only with enums of the same states.
pub fn score(t: &Stats, a: &Stats) -> Option<f64> {
    if t.whole || a.whole {
        let same = t.whole && a.whole && !t.states.is_empty() && t.states == a.states;
        return same.then_some(0.0);
    }
    let units = !t.units.is_empty() && !a.units.is_empty();
    if units && t.units != a.units {
        return None;
    }
    let (mt, ma) = (t.magnitude(), a.magnitude());
    let decades = match (mt > 0.0, ma > 0.0) {
        (true, true) => (mt / ma).log10().abs(),
        (false, false) => 0.0,
        // All zero (0 % in a shutdown) says nothing of the scale.
        _ => f64::INFINITY,
    };
    let overlap = t.lo <= a.hi && a.lo <= t.hi;
    let allowed = 1.0 + if units { 0.3 } else { 0.0 } + if overlap { 0.3 } else { 0.0 };
    Some(decades / allowed)
}

/// The axes of a pane, for placing a trace: the id and stats of each
/// axis with traces, and how many more have traces but no stats yet
/// (never shown), which count toward the limit.
#[derive(Clone, Debug, Default)]
pub struct PaneAxes {
    pub known: Vec<(u32, Stats)>,
    pub unknown: usize,
}

impl PaneAxes {
    /// Its axes of values (not of states).
    pub fn values(&self) -> usize {
        self.known.iter().filter(|(_, a)| !a.whole).count() + self.unknown
    }
}

/// Where a trace settles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Place {
    /// The existing axis with this id, in the pane at this index.
    Join(usize, u32),
    /// An axis of its own in the pane at this index.
    New(usize),
    /// An axis of its own in a new pane: those it may go in are full.
    NewPane,
}

/// Where a trace with stats `t` settles, among `panes` (those it may go
/// in, in order).
///
/// The best match within its score in any of them (the first pane's on a
/// tie), else a new axis in the first pane with room for it (under the
/// [`SOFT_LIMIT`] of axes of values; states always have room), else a new
/// pane. A trace never joins an axis it does not match: other units,
/// states, or values too far apart.
pub fn place(t: &Stats, panes: &[PaneAxes]) -> Place {
    let best = (panes.iter().enumerate())
        .flat_map(|(k, p)| {
            (p.known.iter()).filter_map(move |(id, a)| score(t, a).map(|s| (k, *id, s)))
        })
        .filter(|(_, _, s)| *s <= 1.0)
        .min_by(|a, b| a.2.total_cmp(&b.2));
    if let Some((k, id, _)) = best {
        return Place::Join(k, id);
    }
    match panes
        .iter()
        .position(|p| t.whole || p.values() < SOFT_LIMIT)
    {
        Some(k) => Place::New(k),
        None => Place::NewPane,
    }
}

/// What an axis is named from: its traces' units, names and descriptions.
#[derive(Clone, Debug, Default)]
pub struct Members<'a> {
    pub units: Vec<&'a str>,
    pub names: Vec<&'a str>,
    pub descs: Vec<&'a str>,
    /// An axis of states (enums).
    pub states: bool,
}

/// The quantity units measure, from a small table ("mA" → "Current").
pub fn quantity(units: &str) -> Option<&'static str> {
    Some(match units.trim() {
        "A" | "mA" | "uA" | "µA" | "nA" | "pA" | "kA" => "Current",
        // "C" is degrees at facilities far more often than coulombs.
        "°C" | "degC" | "C" | "C°" | "K" | "mK" | "°F" | "degF" => "Temperature",
        "mbar" | "mBar" | "bar" | "Torr" | "torr" | "mTorr" | "Pa" | "kPa" | "hPa" | "MPa" => {
            "Pressure"
        }
        "m" | "mm" | "um" | "µm" | "nm" | "cm" => "Position",
        "V" | "mV" | "kV" | "uV" | "µV" | "MV" => "Voltage",
        "Hz" | "kHz" | "MHz" | "GHz" => "Frequency",
        "mC" | "uC" | "µC" | "nC" | "pC" | "fC" => "Charge",
        "s" | "ms" | "us" | "µs" | "ns" | "min" | "h" | "hr" | "hours" | "d" => "Time",
        "W" | "mW" | "kW" | "MW" => "Power",
        "deg" | "°" | "rad" | "mrad" | "urad" | "µrad" => "Angle",
        "T" | "mT" | "G" | "gauss" => "Field",
        "eV" | "keV" | "MeV" | "GeV" | "J" | "mJ" | "uJ" | "µJ" => "Energy",
        _ => return None,
    })
}

/// Names for `axes`, built in layers, each used only when it says more:
///
/// 1. the quantity and units: "Current (mA)"; unknown units as they are,
///    "(counts)";
/// 2. for axes that share units, the part of their PV names that tells
///    them apart: "Position X (mm)", "Position Y (mm)";
/// 3. without units, the PVs' description, when known;
/// 4. else "Axis N".
///
/// An axis of states is named after its PV, or "State" with several.
pub fn names(axes: &[Members]) -> Vec<String> {
    let units: Vec<String> = axes.iter().map(axis_units).collect();
    axes.iter()
        .enumerate()
        .map(|(i, m)| {
            if m.states {
                return match m.names.as_slice() {
                    [one] => one.to_string(),
                    _ => "State".to_string(),
                };
            }
            let u = &units[i];
            // Layer 2: the others with these units, this one among them.
            let group: Vec<usize> = (0..axes.len()).filter(|&j| units[j] == *u).collect();
            let apart = if group.len() > 1 && !u.is_empty() {
                let others: Vec<&str> = group
                    .iter()
                    .filter(|&&j| j != i)
                    .flat_map(|&j| axes[j].names.iter().copied())
                    .collect();
                distinguishing(&m.names, &others)
            } else {
                None
            };
            let quantity = quantity(u).map(str::to_string);
            let base = match (quantity, apart) {
                (Some(q), Some(a)) => Some(format!("{q} {a}")),
                (q, a) => q.or(a),
            };
            if !u.is_empty() {
                return match base {
                    Some(b) => format!("{b} ({u})"),
                    None => format!("({u})"),
                };
            }
            if let Some(b) = base {
                return b;
            }
            // Layer 3: one description shared by its traces.
            match m.descs.first() {
                Some(d) if !d.is_empty() && m.descs.iter().all(|x| x == d) => d.to_string(),
                _ => format!("Axis {}", i + 1),
            }
        })
        .collect()
}

/// An axis's units: its traces', distinct and in order ("degC · V" for a
/// mix the user made); empty with none.
fn axis_units(m: &Members) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for u in &m.units {
        let u = u.trim();
        if !u.is_empty() && !seen.contains(&u) {
            seen.push(u);
        }
    }
    seen.join(" · ")
}

/// The words that set `mine` (an axis's PV names) apart from `others`
/// (the names on the axes it is told from): those every one of `mine`
/// has and none of `others` has, in the first name's order; at most
/// three, or it is no name. `None` when no word is left.
///
/// `SR01C-DI-EBPM-01:SA:X` against `SR01C-DI-EBPM-01:SA:Y` is "X". Plain
/// string logic: it suits `DOMAIN-TECH-COMP-NN:SIGNAL` names without
/// depending on them.
fn distinguishing(mine: &[&str], others: &[&str]) -> Option<String> {
    let (first, rest) = mine.split_first()?;
    let rest: Vec<Vec<&str>> = rest.iter().map(|n| words(n)).collect();
    let other: Vec<&str> = others.iter().flat_map(|n| words(n)).collect();
    let mut kept: Vec<&str> = Vec::new();
    for w in words(first) {
        if rest.iter().all(|r| r.contains(&w)) && !other.contains(&w) && !kept.contains(&w) {
            kept.push(w);
        }
    }
    (!kept.is_empty() && kept.len() <= 3).then(|| kept.join(" "))
}

/// The words of a PV name, split at `-`, `:`, `.`, `_`, `/` and spaces.
fn words(name: &str) -> Vec<&str> {
    name.split(['-', ':', '.', '_', '/', ' '])
        .filter(|w| !w.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(units: &str, lo: f64, hi: f64) -> Stats {
        Stats {
            units: units.into(),
            lo,
            hi,
            min: lo,
            max: hi,
            positive: lo > 0.0,
            whole: false,
            states: Vec::new(),
        }
    }

    fn states(names: &[&str]) -> Stats {
        Stats {
            whole: true,
            states: names.iter().map(|s| s.to_string()).collect(),
            ..stats("", 0.0, names.len() as f64 - 1.0)
        }
    }

    #[test]
    fn auto_join_scores() {
        // (what, trace, axis, joins). Ranges are the 5th..95th percentiles
        // seen at DLS.
        let enum_ = states(&["Off", "On"]);
        let cases = [
            (
                "DCCT lifetime does not join the beam current",
                stats("h", 5.2, 12.7),
                stats("mA", 0.0, 299.8),
                false,
            ),
            (
                "EBPM SA:Y joins SA:X (mm, a decade apart, overlapping)",
                stats("mm", 0.0, 0.597),
                stats("mm", 0.0, 0.0398),
                true,
            ),
            (
                "differing units never join, however alike",
                stats("mA", 290.0, 300.0),
                stats("V", 290.0, 300.0),
                false,
            ),
            (
                "same units, three decades apart",
                stats("mA", 0.25, 0.35),
                stats("mA", 295.0, 300.0),
                false,
            ),
            (
                "charges in nC share",
                stats("nC", -0.029, 0.043),
                stats("nC", 0.0013, 0.010),
                true,
            ),
            (
                "efficiencies in % share",
                stats("%", 0.0, 23.7),
                stats("%", 0.0, 19.2),
                true,
            ),
            (
                "no units: close values join",
                stats("", 1.0, 5.0),
                stats("", 2.0, 8.0),
                true,
            ),
            (
                "no units: far apart, not overlapping",
                stats("", 100.0, 120.0),
                stats("", 1.0, 5.0),
                false,
            ),
            (
                "units on one side only: by range",
                stats("", 280.0, 300.0),
                stats("mA", 0.0, 300.0),
                true,
            ),
            (
                "an enum never joins analogue values",
                enum_.clone(),
                stats("", 0.0, 3.0),
                false,
            ),
            (
                "enums with the same states share",
                enum_.clone(),
                enum_.clone(),
                true,
            ),
            (
                "enums with other states do not",
                states(&["Shutdown", "User", "Special"]),
                enum_,
                false,
            ),
            (
                "enums with states unknown do not",
                states(&[]),
                states(&[]),
                false,
            ),
            (
                "all zero (0 % in a shutdown) fits no scale",
                stats("%", 0.0, 0.0),
                stats("%", 0.0, 23.7),
                false,
            ),
            (
                "unless the axis is all zero too",
                stats("%", 0.0, 0.0),
                stats("%", 0.0, 0.0),
                true,
            ),
        ];
        for (what, t, a, joins) in cases {
            let s = score(&t, &a);
            assert_eq!(s.is_some_and(|s| s <= 1.0), joins, "{what}: {s:?}");
        }
    }

    /// One pane of `known` axes.
    fn pane(known: &[(u32, Stats)], unknown: usize) -> PaneAxes {
        PaneAxes {
            known: known.to_vec(),
            unknown,
        }
    }

    #[test]
    fn traces_choose_an_axis_and_respect_the_soft_limit() {
        let one =
            |t: &Stats, known: &[(u32, Stats)], unknown: usize| place(t, &[pane(known, unknown)]);
        let current = (0, stats("mA", 0.0, 300.0));
        let x = stats("mm", 0.0, 0.04);
        assert_eq!(one(&x, &[], 0), Place::New(0));
        assert_eq!(one(&x, std::slice::from_ref(&current), 0), Place::New(0));
        let axes = [current.clone(), (1, x.clone())];
        assert_eq!(one(&stats("mm", 0.0, 0.6), &axes, 0), Place::Join(0, 1));
        // The better of two matches.
        let axes = [
            (0, stats("", 0.0, 1.0)),
            (1, stats("", 0.0, 5.0)),
            (2, stats("", 0.0, 50.0)),
        ];
        assert_eq!(one(&stats("", 0.0, 8.0), &axes, 0), Place::Join(0, 1));
        // Four axes: a match still joins; anything else, even in the
        // same units but far apart, needs a new pane.
        let full = [
            current.clone(),
            (1, stats("h", 5.0, 13.0)),
            (2, stats("nC", 0.0, 0.01)),
            (3, stats("mm", 0.0, 1.0)),
        ];
        assert_eq!(one(&stats("mm", 0.0, 0.5), &full, 0), Place::Join(0, 3));
        assert_eq!(one(&stats("mm", 50.0, 90.0), &full, 0), Place::NewPane);
        assert_eq!(one(&stats("V", 9.0, 11.0), &full, 0), Place::NewPane);
        assert_eq!(one(&stats("%", 0.0, 0.0), &full, 0), Place::NewPane);
        // Axes not yet shown count; axes of states do not.
        let three = &full[..3];
        assert_eq!(one(&stats("mm", 50.0, 90.0), three, 0), Place::New(0));
        assert_eq!(one(&stats("mm", 50.0, 90.0), three, 1), Place::NewPane);
        let with_state = [three, &[(4, states(&["Off", "On"]))]].concat();
        assert_eq!(
            one(&stats("x", 1000.0, 2000.0), &with_state, 0),
            Place::New(0),
            "three of values and one of states"
        );
        // States never join values, or other states, and always have room.
        let full_state = [&full[..], &[(4, states(&["Off", "On"]))]].concat();
        assert_eq!(
            one(&states(&["Shutdown", "User"]), &full_state, 0),
            Place::New(0)
        );
        assert_eq!(
            one(&states(&["Off", "On"]), &full_state, 0),
            Place::Join(0, 4)
        );
    }

    #[test]
    fn traces_find_their_pane() {
        let full = pane(
            &[
                (0, stats("mA", 0.0, 300.0)),
                (1, stats("h", 5.0, 13.0)),
                (2, stats("nC", 0.0, 0.01)),
                (3, stats("%", 0.0, 20.0)),
            ],
            0,
        );
        let vac = pane(&[(4, stats("mbar", 1e-10, 1e-8))], 0);
        // A match in any pane, the better one first.
        let gauge = stats("mbar", 2e-10, 3e-9);
        assert_eq!(
            place(&gauge, &[full.clone(), vac.clone()]),
            Place::Join(1, 4)
        );
        let current = stats("mA", 0.0, 250.0);
        assert_eq!(
            place(&current, &[full.clone(), vac.clone()]),
            Place::Join(0, 0)
        );
        // No match: the first pane with room, else a new one.
        let volts = stats("V", 9.0, 11.0);
        assert_eq!(place(&volts, &[full.clone(), vac.clone()]), Place::New(1));
        assert_eq!(place(&volts, &[full.clone()]), Place::NewPane);
        assert_eq!(place(&volts, &[]), Place::NewPane);
    }

    #[test]
    fn stats_use_the_bulk_of_the_values() {
        // A glitch to zero and a spike do not set the scale.
        let mut v: Vec<f64> = vec![300.0; 98];
        v.extend([0.0, 5000.0]);
        let s = Stats::of(v.into_iter(), "mA", false).unwrap();
        assert_eq!((s.lo, s.hi), (300.0, 300.0));
        assert!(!s.positive);
        assert!(Stats::of([f64::NAN].into_iter(), "", false).is_none());
        // Vacuum over decades wants a log scale; a lifetime does not.
        let p = Stats::of(
            (0..100).map(|i| 1e-10 * 10f64.powf(i as f64 / 25.0)),
            "mbar",
            false,
        )
        .unwrap();
        assert!(p.wants_log());
        assert!(!stats("h", 5.2, 12.7).wants_log());
        assert!(!stats("mA", -0.1, 300.0).wants_log());
        // A gauge in mbar is on a log scale even in a quiet week; a vent
        // spike in the full range counts too.
        assert!(stats("mbar", 2e-10, 3e-10).wants_log());
        assert!(!stats("mbar", 0.0, 3e-10).wants_log(), "not with a zero");
        let spike = Stats {
            max: 3e-7,
            ..stats("", 2e-10, 3e-10)
        };
        assert!(spike.wants_log());
        let both = Stats::merge([&stats("mm", 0.0, 0.04), &stats("mm", -0.1, 0.6)]).unwrap();
        assert_eq!((both.lo, both.hi, both.units.as_str()), (-0.1, 0.6, "mm"));
    }

    fn members<'a>(units: &[&'a str], names: &[&'a str]) -> Members<'a> {
        Members {
            units: units.to_vec(),
            names: names.to_vec(),
            descs: Vec::new(),
            states: false,
        }
    }

    #[test]
    fn axes_name_themselves() {
        // Layer 1: the quantity from the units, unknown units as they are.
        assert_eq!(
            names(&[
                members(&["mA"], &["SR-DI-DCCT-01:SIGNAL"]),
                members(&["h"], &["SR-DI-DCCT-01:LIFETIME"]),
                members(&["counts"], &["X"]),
                members(
                    &["nC", "nC"],
                    &["LB-DI-ICT-01:SIGNAL", "BS-DI-ICT-01:SIGNAL"]
                ),
            ]),
            ["Current (mA)", "Time (h)", "(counts)", "Charge (nC)"]
        );
        // Layer 2: axes with the same units, told apart by their names.
        assert_eq!(
            names(&[
                members(&["mm"], &["SR01C-DI-EBPM-01:SA:X"]),
                members(&["mm"], &["SR01C-DI-EBPM-01:SA:Y"]),
            ]),
            ["Position X (mm)", "Position Y (mm)"]
        );
        assert_eq!(
            names(&[
                members(&["mA"], &["SR-DI-DCCT-01:SIGNAL"]),
                members(&["mA"], &["BR01C-DI-DCCT-01:SIGNAL"]),
            ]),
            ["Current SR (mA)", "Current BR01C (mA)"]
        );
        // Several names on one axis: the words they share.
        assert_eq!(
            names(&[
                members(
                    &["mm", "mm"],
                    &["SR01C-DI-EBPM-01:SA:X", "SR02C-DI-EBPM-01:SA:X"]
                ),
                members(&["mm"], &["SR01C-DI-EBPM-01:SA:Y"]),
            ]),
            ["Position X (mm)", "Position Y (mm)"]
        );
        // Layers 3 and 4: no units: a shared description, else "Axis N".
        let desc = Members {
            descs: vec!["Gun state"],
            ..members(&[""], &["G:STATE"])
        };
        assert_eq!(
            names(&[members(&[""], &["A"]), desc]),
            ["Axis 1", "Gun state"]
        );
        // States: the PV, or "State" for several.
        let state = |names: &[&'static str]| Members {
            states: true,
            ..members(&[""], names)
        };
        assert_eq!(
            names(&[state(&["CS-CS-MSTAT-01:MODE"]), state(&["A:STA", "B:STA"])]),
            ["CS-CS-MSTAT-01:MODE", "State"]
        );
        assert_eq!(quantity("C"), Some("Temperature"));
        // Mixed by hand: every unit shows.
        assert_eq!(
            names(&[members(&["degC", "V"], &["A", "B"])]),
            ["(degC · V)"]
        );
    }

    #[test]
    fn names_split_into_words() {
        assert_eq!(
            words("SR-DI-DCCT-01:SIGNAL"),
            ["SR", "DI", "DCCT", "01", "SIGNAL"]
        );
        assert_eq!(words("a::b"), ["a", "b"]);
        assert_eq!(distinguishing(&["A:B:C"], &["A:X:C"]).as_deref(), Some("B"));
        // Identical names: nothing tells them apart; nor do many words.
        assert_eq!(distinguishing(&["A:B"], &["A:B"]), None);
        assert_eq!(distinguishing(&["A:B:C:D"], &["X"]), None);
    }
}
