//! The synthetic PVs: deterministic functions of time on a fixed sample grid.
//!
//! | PV | Signal |
//! |---|---|
//! | `MOCK:SINE` | 1 Hz, 10-minute-period sine, EGU `mA`, PREC 3 (MINOR alarm near the top) |
//! | `MOCK:RAMP` | 0.1 Hz sawtooth 0..100 over an hour |
//! | `MOCK:NOISE` | 10 Hz noise on a slow drift, seeded per sample (no RNG state) |
//! | `MOCK:SLOW` | one sample every 5 minutes (so `optimized` returns raw) |
//! | `MOCK:STEP` | a value that holds for 4 minutes, then jumps (1 s grid) |
//! | `MOCK:ENUM` | integer 0..3, sampled every minute |
//! | `MOCK:STRING` | text |
//! | `MOCK:WAVE` | 8-element array |
//! | `MOCK:NAN` | sine with a `NaN` every 50th sample and severity-3 `cnxlostepsecs` gaps |
//! | `MOCK:BIG` | 1 Hz sine of amplitude 3e10 (Java `3.0E10` notation on the wire) |
//! | `bl01t:A`, `bl01t:B`, `bl01t:SUM`, `BL01T-MO-SIMC-01:M1.RBV`, `BL01T-MO-SIMC-01:M2.RBV` | names bobview-testioc serves, so archive and live join up |
//!
//! Every value is a pure function of the sample's absolute grid index
//! (`t / period`), so two servers started at different times agree on
//! every sample they both have. "Noise" is a hash of that index.

/// Days of history before the server's start time.
pub const HISTORY_DAYS: u32 = 30;

/// What a mock PV's archived values are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Double,
    Int,
    Enum,
    Text,
    Waveform(u32),
}

impl Kind {
    /// The appliance's `DBRType` name for this kind.
    pub fn dbr_type(self) -> &'static str {
        match self {
            Kind::Double => "DBR_SCALAR_DOUBLE",
            Kind::Int => "DBR_SCALAR_INT",
            Kind::Enum => "DBR_SCALAR_ENUM",
            Kind::Text => "DBR_SCALAR_STRING",
            Kind::Waveform(_) => "DBR_WAVEFORM_DOUBLE",
        }
    }

    /// Elements per sample.
    pub fn element_count(self) -> u32 {
        match self {
            Kind::Waveform(n) => n,
            _ => 1,
        }
    }

    /// True when the summarising operators apply (numeric scalars).
    pub fn is_scalar_numeric(self) -> bool {
        matches!(self, Kind::Double | Kind::Int | Kind::Enum)
    }
}

/// What the synthetic signal looks like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Sine,
    Ramp,
    Noise,
    Slow,
    Step,
    Enum,
    Text,
    Wave,
    Nan,
    Big,
    BeamA,
    BeamB,
    BeamSum,
    Motor1,
    Motor2,
}

/// One synthetic PV.
#[derive(Clone, Copy, Debug)]
pub struct Signal {
    pub name: &'static str,
    pub kind: Kind,
    /// Seconds between samples.
    pub period: f64,
    pub egu: &'static str,
    pub prec: u32,
    /// `DESC`.
    pub desc: &'static str,
    /// `LOPR`/`HOPR`.
    pub lopr: f64,
    pub hopr: f64,
    pub shape: Shape,
}

#[allow(clippy::too_many_arguments)]
const fn sig(
    name: &'static str,
    kind: Kind,
    period: f64,
    egu: &'static str,
    prec: u32,
    desc: &'static str,
    range: (f64, f64),
    shape: Shape,
) -> Signal {
    Signal {
        name,
        kind,
        period,
        egu,
        prec,
        desc,
        lopr: range.0,
        hopr: range.1,
        shape,
    }
}

#[rustfmt::skip]
static ALL: [Signal; 15] = [
    sig("MOCK:SINE", Kind::Double, 1.0, "mA", 3, "Sine, 10 minute period", (200.0, 300.0), Shape::Sine),
    sig("MOCK:RAMP", Kind::Double, 10.0, "%", 1, "Sawtooth over an hour", (0.0, 100.0), Shape::Ramp),
    sig("MOCK:NOISE", Kind::Double, 0.1, "V", 4, "Noise on a slow drift", (-10.0, 10.0), Shape::Noise),
    sig("MOCK:SLOW", Kind::Double, 300.0, "degC", 2, "Every five minutes", (15.0, 25.0), Shape::Slow),
    sig("MOCK:STEP", Kind::Double, 1.0, "mm", 2, "Holds, then jumps", (0.0, 10.0), Shape::Step),
    sig("MOCK:ENUM", Kind::Enum, 60.0, "", 0, "Mode 0..3", (0.0, 3.0), Shape::Enum),
    sig("MOCK:STRING", Kind::Text, 600.0, "", 0, "Status text", (0.0, 0.0), Shape::Text),
    sig("MOCK:WAVE", Kind::Waveform(8), 10.0, "counts", 1, "8-element waveform", (-1.0, 1.0), Shape::Wave),
    sig("MOCK:NAN", Kind::Double, 1.0, "mA", 3, "Sine with NaNs and disconnects", (-1.0, 1.0), Shape::Nan),
    sig("MOCK:BIG", Kind::Double, 1.0, "Pa", 3, "Large values", (-3e10, 3e10), Shape::Big),
    sig("bl01t:A", Kind::Double, 10.0, "", 3, "Input A", (0.0, 10.0), Shape::BeamA),
    sig("bl01t:B", Kind::Double, 10.0, "", 3, "Input B", (0.0, 10.0), Shape::BeamB),
    sig("bl01t:SUM", Kind::Double, 10.0, "", 3, "Sum A and B", (0.0, 20.0), Shape::BeamSum),
    sig("BL01T-MO-SIMC-01:M1.RBV", Kind::Double, 1.0, "mm", 3, "Motor 1 readback", (-10.0, 10.0), Shape::Motor1),
    sig("BL01T-MO-SIMC-01:M2.RBV", Kind::Double, 1.0, "mm", 3, "Motor 2 readback", (-10.0, 10.0), Shape::Motor2),
];

/// Every mock PV, in a fixed order.
pub fn all() -> &'static [Signal] {
    &ALL
}

/// The PV called `name`.
pub fn find(name: &str) -> Option<&'static Signal> {
    all().iter().find(|s| s.name == name)
}

/// One archived value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Double(f64),
    Int(i64),
    Text(String),
    Array(Vec<f64>),
}

impl Value {
    /// The number the summarising operators see (element 0 for arrays).
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Double(v) => Some(*v),
            Value::Int(i) => Some(*i as f64),
            Value::Text(_) => None,
            Value::Array(a) => Some(a.first().copied().unwrap_or(f64::NAN)),
        }
    }
}

/// One archived event.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// UTC nanoseconds since the epoch.
    pub t_ns: i64,
    pub value: Value,
    pub severity: u8,
    pub status: u16,
    /// Extra `fields`, e.g. `cnxlostepsecs`.
    pub fields: Vec<(&'static str, String)>,
}

/// `MOCK:NAN` loses its connection after sample `NAN_LOST` of every
/// `NAN_CYCLE` (absolute index modulo the cycle) and is back at
/// `NAN_REGAINED`; the samples in between are missing.
pub const NAN_CYCLE: i64 = 2000;
pub const NAN_LOST: i64 = 1000;
pub const NAN_REGAINED: i64 = 1060;

/// splitmix64: a deterministic hash of the sample index.
fn hash(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// Uniform in [0, 1).
fn unit(k: i64, salt: u64) -> f64 {
    (hash(k as u64 ^ salt.rotate_left(17)) >> 11) as f64 / (1u64 << 53) as f64
}

/// Roughly normal, mean 0, sd 1 (Irwin-Hall of 4).
fn normal(k: i64, salt: u64) -> f64 {
    let s: f64 = (0..4)
        .map(|i| unit(k, salt.wrapping_add(i).wrapping_mul(0x1000_0001)))
        .sum();
    (s - 2.0) * 3.0f64.sqrt()
}

/// A motor that moves to a new position every 15 minutes, taking 20 s.
fn motor(t: f64, salt: u64) -> f64 {
    const MOVE: f64 = 900.0;
    const TRAVEL: f64 = 20.0;
    let n = (t / MOVE).floor() as i64;
    let pos = |n: i64| ((unit(n, salt) * 16.0 - 8.0) * 100.0).round() / 100.0;
    let (from, to) = (pos(n - 1), pos(n));
    let f = ((t - n as f64 * MOVE) / TRAVEL).clamp(0.0, 1.0);
    from + (to - from) * f
}

const TAU: f64 = std::f64::consts::TAU;

impl Signal {
    /// The sample period in nanoseconds.
    pub fn period_ns(&self) -> i64 {
        (self.period * 1e9).round() as i64
    }

    /// The event at absolute grid index `k` (time `k * period`), or `None`
    /// when the PV was disconnected then.
    pub fn event(&self, k: i64) -> Option<Event> {
        let t_ns = k * self.period_ns();
        let t = t_ns as f64 * 1e-9;
        let mut severity = 0;
        let mut status = 0;
        let mut fields = Vec::new();
        let salt = self
            .name
            .bytes()
            .fold(0u64, |h, b| h.wrapping_mul(31) ^ b as u64);
        let value = match self.shape {
            Shape::Sine => {
                let v = 250.0 + 40.0 * (TAU * t / 600.0).sin() + 0.2 * normal(k, salt);
                if v > 285.0 {
                    severity = 1;
                    status = 4; // HIGH
                }
                Value::Double(v)
            }
            Shape::Ramp => Value::Double((t % 3600.0) / 36.0),
            Shape::Noise => Value::Double(5.0 * (TAU * t / 10800.0).sin() + normal(k, salt)),
            Shape::Slow => {
                Value::Double(20.0 + 3.0 * (TAU * t / 86400.0).sin() + 0.3 * normal(k, salt))
            }
            Shape::Step => {
                let n = (t / 240.0).floor() as i64;
                Value::Double((unit(n, salt) * 1000.0).round() / 100.0)
            }
            Shape::Enum => Value::Int((hash((t / 1800.0).floor() as u64 ^ salt) % 4) as i64),
            Shape::Text => {
                const STATES: [&str; 4] = ["Idle", "Running", "Paused", "Fault: \"door\" open"];
                Value::Text(STATES[k.rem_euclid(4) as usize].to_string())
            }
            Shape::Wave => Value::Array(
                (0..8)
                    .map(|i| ((TAU * (t / 60.0 + i as f64 / 8.0)).sin() * 10.0).round() / 10.0)
                    .collect(),
            ),
            Shape::Nan => {
                let phase = k.rem_euclid(NAN_CYCLE);
                if phase > NAN_LOST && phase < NAN_REGAINED {
                    return None;
                }
                let lost_secs = (k - phase + NAN_LOST) * self.period_ns() / 1_000_000_000;
                if phase == NAN_LOST {
                    severity = 3;
                    status = 9; // LINK
                    fields.push(("cnxlostepsecs", lost_secs.to_string()));
                } else if phase == NAN_REGAINED {
                    fields.push(("cnxlostepsecs", lost_secs.to_string()));
                    fields.push(("cnxregainedepsecs", (t_ns / 1_000_000_000).to_string()));
                }
                if k.rem_euclid(50) == 49 {
                    Value::Double(f64::NAN)
                } else {
                    Value::Double((TAU * t / 300.0).sin())
                }
            }
            Shape::Big => Value::Double(3e10 * (TAU * t / 600.0).sin()),
            Shape::BeamA => Value::Double(beam_a(t)),
            Shape::BeamB => Value::Double(beam_b(t)),
            Shape::BeamSum => Value::Double(beam_a(t) + beam_b(t)),
            Shape::Motor1 => Value::Double(motor(t, salt)),
            Shape::Motor2 => Value::Double(motor(t + 450.0, salt)),
        };
        Some(Event {
            t_ns,
            value,
            severity,
            status,
            fields,
        })
    }
}

fn beam_a(t: f64) -> f64 {
    2.54 + 0.4 * (TAU * t / 7200.0).sin()
}

fn beam_b(t: f64) -> f64 {
    2.61 + 0.3 * (TAU * t / 10800.0).cos()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_findable() {
        for s in all() {
            assert_eq!(find(s.name).unwrap().name, s.name);
        }
        let mut names: Vec<_> = all().iter().map(|s| s.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), all().len());
        assert!(find("NOPE").is_none());
    }

    #[test]
    fn periods_divide_the_history_alignment() {
        for s in all() {
            assert_eq!(600_000_000_000 % s.period_ns(), 0, "{}", s.name);
        }
    }

    #[test]
    fn values_are_deterministic() {
        let s = find("MOCK:NOISE").unwrap();
        assert_eq!(s.event(12345), s.event(12345));
        assert_ne!(s.event(12345), s.event(12346));
    }

    #[test]
    fn nan_pv_has_nans_and_gaps() {
        let s = find("MOCK:NAN").unwrap();
        let k0 = 1_700_000_000 / NAN_CYCLE * NAN_CYCLE;
        let lost = s.event(k0 + NAN_LOST).unwrap();
        assert_eq!(lost.severity, 3);
        assert_eq!(lost.fields[0].0, "cnxlostepsecs");
        assert!(s.event(k0 + NAN_LOST + 1).is_none());
        assert!(s.event(k0 + NAN_REGAINED - 1).is_none());
        let back = s.event(k0 + NAN_REGAINED).unwrap();
        assert_eq!(back.fields.len(), 2);
        let v = s.event(k0 + 49).unwrap().value.as_f64().unwrap();
        assert!(v.is_nan());
    }

    #[test]
    fn kinds() {
        assert!(Kind::Enum.is_scalar_numeric());
        assert!(!Kind::Text.is_scalar_numeric());
        assert!(!Kind::Waveform(8).is_scalar_numeric());
        assert_eq!(Kind::Waveform(8).element_count(), 8);
        let wave = find("MOCK:WAVE").unwrap().event(0).unwrap();
        assert_eq!(wave.value.as_f64(), Some(0.0));
    }

    #[test]
    fn testioc_values_line_up() {
        let a = find("bl01t:A").unwrap().event(170_000_000).unwrap();
        let b = find("bl01t:B").unwrap().event(170_000_000).unwrap();
        let sum = find("bl01t:SUM").unwrap().event(170_000_000).unwrap();
        let (a, b, sum) = (a.value.as_f64(), b.value.as_f64(), sum.value.as_f64());
        assert!((a.unwrap() + b.unwrap() - sum.unwrap()).abs() < 1e-12);
    }
}
