//! SQLite-compatible date/time functions: `date`, `time`, `datetime`,
//! `julianday`, `unixepoch`, `strftime`.
//!
//! This is a faithful port of SQLite's `date.c`: the internal representation is
//! the Julian Day number expressed in integer milliseconds (`ijd`), and the
//! parsing, modifier, and formatting routines mirror SQLite so results are
//! byte-identical for the supported time strings and modifiers.
//!
//! Not supported (returns `NULL`, matching an unrecognized modifier): the
//! `±YYYY-MM-DD` offset modifier and `auto`/`ceiling`/`floor`. Timezone
//! modifiers `utc`/`localtime` are treated as no-ops because Squrust operates
//! purely in UTC and has no timezone database.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::types::Value;

/// JD (in ms) of the unix epoch 1970-01-01 00:00:00: 2440587.5 * 86400000.
const UNIX_EPOCH_MS: i64 = 210_866_760_000_000;
/// JD (in ms) just past 9999-12-31 23:59:59.999 — the upper validity bound.
const MAX_IJD: i64 = 464_269_060_800_000;
/// Largest raw number accepted as a Julian day (exclusive), as in SQLite.
const MAX_RAW_JD: f64 = 5_373_484.5;

#[derive(Clone)]
struct DateTime {
    ijd: i64,
    y: i64,
    m: i64,
    d: i64,
    h: i64,
    min: i64,
    s: f64,
    /// The original numeric input, if the time string was a bare number.
    raw_s: Option<f64>,
    valid_jd: bool,
    valid_ymd: bool,
    valid_hms: bool,
    use_subsec: bool,
    error: bool,
}

impl DateTime {
    fn blank() -> Self {
        DateTime {
            ijd: 0,
            y: 0,
            m: 0,
            d: 0,
            h: 0,
            min: 0,
            s: 0.0,
            raw_s: None,
            valid_jd: false,
            valid_ymd: false,
            valid_hms: false,
            use_subsec: false,
            error: false,
        }
    }

    fn compute_jd(&mut self) {
        if self.valid_jd {
            return;
        }
        let (mut y, mut m, d) = if self.valid_ymd {
            (self.y, self.m, self.d)
        } else {
            (2000, 1, 1)
        };
        if !(-4713..=9999).contains(&y) || self.raw_s.is_some() {
            self.error = true;
            return;
        }
        if m <= 2 {
            y -= 1;
            m += 12;
        }
        let a = (y + 4800) / 100;
        let b = 38 - a + (a / 4);
        let x1 = 36525 * (y + 4716) / 100;
        let x2 = 306001 * (m + 1) / 10000;
        self.ijd = (((x1 + x2 + d + b) as f64 - 1524.5) * 86_400_000.0) as i64;
        self.valid_jd = true;
        if self.valid_hms {
            self.ijd += self.h * 3_600_000 + self.min * 60_000 + (self.s * 1000.0 + 0.5) as i64;
        }
    }

    fn compute_ymd(&mut self) {
        if self.valid_ymd {
            return;
        }
        if !self.valid_jd {
            self.y = 2000;
            self.m = 1;
            self.d = 1;
        } else {
            let z = (self.ijd + 43_200_000) / 86_400_000;
            let alpha = ((z as f64 + 32044.75) / 36524.25) as i64 - 52;
            let a = z + 1 + alpha - ((alpha + 100) / 4) + 25;
            let b = a + 1524;
            let c = ((b as f64 - 122.1) / 365.25) as i64;
            let dd = (36525 * (c & 32767)) / 100;
            let e = ((b - dd) as f64 / 30.6001) as i64;
            let x1 = (30.6001 * e as f64) as i64;
            self.d = b - dd - x1;
            self.m = if e < 14 { e - 1 } else { e - 13 };
            self.y = if self.m > 2 { c - 4716 } else { c - 4715 };
        }
        self.valid_ymd = true;
    }

    fn compute_hms(&mut self) {
        if self.valid_hms {
            return;
        }
        self.compute_jd();
        let day_ms = (self.ijd + 43_200_000).rem_euclid(86_400_000);
        self.s = (day_ms % 60_000) as f64 / 1000.0;
        let day_min = day_ms / 60_000;
        self.min = day_min % 60;
        self.h = day_min / 60;
        self.raw_s = None;
        self.valid_hms = true;
    }

    fn compute_ymd_hms(&mut self) {
        self.compute_ymd();
        self.compute_hms();
    }

    fn clear_derived(&mut self) {
        self.valid_ymd = false;
        self.valid_hms = false;
    }

    fn days_after_sunday(&self) -> i64 {
        ((self.ijd + 129_600_000) / 86_400_000) % 7
    }

    fn days_after_monday(&self) -> i64 {
        ((self.ijd + 43_200_000) / 86_400_000) % 7
    }

    /// A clone moved to the Thursday of the same ISO week (for `%G`/`%g`/`%V`).
    fn iso_thursday(&self) -> DateTime {
        let mut y = self.clone();
        y.ijd += (3 - self.days_after_monday()) * 86_400_000;
        y.valid_ymd = false;
        y.compute_ymd();
        y
    }

    fn days_after_jan01(&self) -> i64 {
        let mut jan01 = self.clone();
        jan01.valid_jd = false;
        jan01.m = 1;
        jan01.d = 1;
        jan01.compute_jd();
        (self.ijd - jan01.ijd + 43_200_000) / 86_400_000
    }
}

/// A raw number is either a Julian day or (later) a unix timestamp.
fn set_raw_number(p: &mut DateTime, r: f64) {
    p.raw_s = Some(r);
    if (0.0..MAX_RAW_JD).contains(&r) {
        p.ijd = (r * 86_400_000.0 + 0.5) as i64;
        p.valid_jd = true;
    }
}

fn set_to_current(p: &mut DateTime) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    p.ijd = secs + UNIX_EPOCH_MS;
    p.valid_jd = true;
}

/// Parse `HH:MM[:SS[.FFF]]` (no timezone) starting at `z`. Returns the parsed
/// `(h, m, s)` and the unconsumed tail, or `None`.
fn parse_hh_mm_ss(z: &str) -> Option<(i64, i64, f64, &str)> {
    let bytes = z.as_bytes();
    if bytes.len() < 5 || !bytes[0].is_ascii_digit() || !bytes[1].is_ascii_digit() {
        return None;
    }
    if bytes[2] != b':' || !bytes[3].is_ascii_digit() || !bytes[4].is_ascii_digit() {
        return None;
    }
    let h = ((bytes[0] - b'0') * 10 + (bytes[1] - b'0')) as i64;
    let m = ((bytes[3] - b'0') * 10 + (bytes[4] - b'0')) as i64;
    let mut rest = &z[5..];
    let mut s = 0.0;
    if let Some(after) = rest.strip_prefix(':') {
        let ab = after.as_bytes();
        if ab.len() < 2 || !ab[0].is_ascii_digit() || !ab[1].is_ascii_digit() {
            return None;
        }
        let sec = ((ab[0] - b'0') * 10 + (ab[1] - b'0')) as i64;
        rest = &after[2..];
        let mut frac = 0.0;
        if let Some(dot) = rest.strip_prefix('.') {
            let mut scale = 1.0;
            let mut consumed = 0;
            for c in dot.bytes() {
                if !c.is_ascii_digit() {
                    break;
                }
                frac = frac * 10.0 + (c - b'0') as f64;
                scale *= 10.0;
                consumed += 1;
            }
            if consumed > 0 {
                frac /= scale;
                if frac > 0.999 {
                    frac = 0.999;
                }
                rest = &rest[1 + consumed..];
            }
        }
        s = sec as f64 + frac;
    }
    Some((h, m, s, rest))
}

/// Parse the leading `YYYY-MM-DD`, optionally followed by ` `/`T` and a time.
fn parse_yyyy_mm_dd(z: &str, p: &mut DateTime) -> bool {
    let (neg, body) = match z.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, z),
    };
    let b = body.as_bytes();
    if b.len() < 10 {
        return false;
    }
    let digit = |i: usize| -> Option<i64> {
        if b[i].is_ascii_digit() {
            Some((b[i] - b'0') as i64)
        } else {
            None
        }
    };
    let (y, m, d) = match (
        digit(0),
        digit(1),
        digit(2),
        digit(3),
        digit(5),
        digit(6),
        digit(8),
        digit(9),
    ) {
        (Some(y0), Some(y1), Some(y2), Some(y3), Some(m0), Some(m1), Some(d0), Some(d1))
            if b[4] == b'-' && b[7] == b'-' =>
        {
            (
                y0 * 1000 + y1 * 100 + y2 * 10 + y3,
                m0 * 10 + m1,
                d0 * 10 + d1,
            )
        }
        _ => return false,
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return false;
    }
    let mut tail = &body[10..];
    tail = tail.trim_start_matches([' ', 'T']);
    if tail.is_empty() {
        p.valid_hms = false;
    } else if let Some((h, mi, s, rest)) = parse_hh_mm_ss(tail) {
        // Allow a trailing 'Z' (UTC); other trailing text is rejected.
        let rest = rest.trim_start_matches('Z');
        if !rest.is_empty() {
            return false;
        }
        p.valid_hms = true;
        p.h = h;
        p.min = mi;
        p.s = s;
    } else {
        return false;
    }
    p.valid_jd = false;
    p.valid_ymd = true;
    p.y = if neg { -y } else { y };
    p.m = m;
    p.d = d;
    true
}

/// Parse the time string (first argument). Returns `false` on parse error.
fn parse_date_or_time(z: &str, p: &mut DateTime) -> bool {
    let z = z.trim();
    if parse_yyyy_mm_dd(z, p) {
        return true;
    }
    if let Some((h, m, s, rest)) = parse_hh_mm_ss(z) {
        if rest.trim_start_matches('Z').is_empty() {
            p.valid_jd = false;
            p.valid_hms = true;
            p.h = h;
            p.min = m;
            p.s = s;
            return true;
        }
    }
    if z.eq_ignore_ascii_case("now") {
        set_to_current(p);
        return true;
    }
    if let Ok(r) = z.parse::<f64>() {
        set_raw_number(p, r);
        return true;
    }
    if z.eq_ignore_ascii_case("subsec") || z.eq_ignore_ascii_case("subsecond") {
        p.use_subsec = true;
        set_to_current(p);
        return true;
    }
    false
}

const XFORM: &[(&str, f64, f64)] = &[
    ("second", 4.6427e+14, 1.0),
    ("minute", 7.7379e+12, 60.0),
    ("hour", 1.2897e+11, 3600.0),
    ("day", 5_373_485.0, 86400.0),
    ("month", 176_546.0, 2_592_000.0),
    ("year", 14713.0, 31_536_000.0),
];

/// Apply one modifier. Returns `false` if the modifier is unrecognized/invalid.
fn parse_modifier(z: &str, p: &mut DateTime, idx: usize) -> bool {
    let lower = z.trim().to_ascii_lowercase();

    if lower == "julianday" {
        if idx > 1 {
            return false;
        }
        if p.valid_jd && p.raw_s.is_some() {
            p.raw_s = None;
            return true;
        }
        return false;
    }
    if lower == "unixepoch" {
        if idx > 1 {
            return false;
        }
        if let Some(r) = p.raw_s {
            let v = r * 1000.0 + UNIX_EPOCH_MS as f64;
            if (0.0..MAX_IJD as f64).contains(&v) {
                p.clear_derived();
                p.ijd = (v + 0.5) as i64;
                p.valid_jd = true;
                p.raw_s = None;
                return true;
            }
        }
        return false;
    }
    // No timezone database: 'utc' and 'localtime' are identity transforms.
    if lower == "utc" || lower == "localtime" {
        return true;
    }
    if lower == "subsec" || lower == "subsecond" {
        p.use_subsec = true;
        return true;
    }
    if let Some(rest) = lower.strip_prefix("weekday ") {
        if let Ok(rf) = rest.trim().parse::<f64>() {
            if (0.0..7.0).contains(&rf) && rf.fract() == 0.0 {
                let n = rf as i64;
                p.compute_ymd_hms();
                p.valid_jd = false;
                p.compute_jd();
                let mut zw = ((p.ijd + 129_600_000) / 86_400_000) % 7;
                if zw > n {
                    zw -= 7;
                }
                p.ijd += (n - zw) * 86_400_000;
                p.clear_derived();
                return true;
            }
        }
        return false;
    }
    if let Some(unit) = lower.strip_prefix("start of ") {
        if !p.valid_jd && !p.valid_ymd && !p.valid_hms {
            return false;
        }
        p.compute_ymd();
        p.valid_hms = true;
        p.h = 0;
        p.min = 0;
        p.s = 0.0;
        p.raw_s = None;
        p.valid_jd = false;
        return match unit {
            "month" => {
                p.d = 1;
                true
            }
            "year" => {
                p.m = 1;
                p.d = 1;
                true
            }
            "day" => true,
            _ => false,
        };
    }

    // Numeric modifiers: "(+|-)N unit" and "(+|-)HH:MM[:SS]".
    let first = lower.as_bytes()[0];
    if first == b'+' || first == b'-' || first.is_ascii_digit() {
        return parse_numeric_modifier(z.trim(), p);
    }
    false
}

fn parse_numeric_modifier(z: &str, p: &mut DateTime) -> bool {
    let neg = z.starts_with('-');
    // Split off the leading signed number from the trailing unit / time.
    let num_end = z
        .char_indices()
        .find(|&(i, c)| i > 0 && !(c.is_ascii_digit() || c == '.'))
        .map(|(i, _)| i)
        .unwrap_or(z.len());
    let (num_str, tail) = z.split_at(num_end);

    // "(+|-)HH:MM[:SS[.FFF]]" time offset.
    if tail.starts_with(':') {
        let after_sign = z.trim_start_matches(['+', '-']);
        let (h, m, s, rest) = match parse_hh_mm_ss(after_sign) {
            Some(v) => v,
            None => return false,
        };
        if !rest.is_empty() {
            return false;
        }
        let mut delta = h * 3_600_000 + m * 60_000 + (s * 1000.0 + 0.5) as i64;
        if neg {
            delta = -delta;
        }
        p.compute_jd();
        p.clear_derived();
        p.ijd += delta;
        return true;
    }

    let r: f64 = match num_str.parse() {
        Ok(v) => v,
        Err(_) => return false,
    };
    let mut unit = tail.trim();
    if unit.is_empty() {
        return false;
    }
    // Strip a trailing plural 's'.
    if unit.len() >= 3 && unit.ends_with('s') {
        unit = &unit[..unit.len() - 1];
    }
    let idx = match XFORM.iter().position(|(name, _, _)| *name == unit) {
        Some(i) => i,
        None => return false,
    };
    let (_, limit, rxform) = XFORM[idx];
    if !(r > -limit && r < limit) {
        return false;
    }
    let rounder = if r < 0.0 { -0.5 } else { 0.5 };
    let mut r = r;
    match idx {
        4 => {
            // months
            p.compute_ymd_hms();
            p.m += r as i64;
            let x = if p.m > 0 {
                (p.m - 1) / 12
            } else {
                (p.m - 12) / 12
            };
            p.y += x;
            p.m -= x * 12;
            p.valid_jd = false;
            r -= r.trunc();
        }
        5 => {
            // years
            p.compute_ymd_hms();
            p.y += r as i64;
            p.valid_jd = false;
            r -= r.trunc();
        }
        _ => {}
    }
    p.compute_jd();
    p.ijd += (r * 1000.0 * rxform + rounder) as i64;
    p.clear_derived();
    true
}

/// Build a `DateTime` from the function arguments (`first` is the time string,
/// the rest are modifiers). Returns `None` on any error (→ SQL `NULL`).
fn build(first: &Value, modifiers: &[Value]) -> Option<DateTime> {
    let mut p = DateTime::blank();
    match first {
        Value::Integer(_) | Value::Real(_) => {
            set_raw_number(&mut p, first.as_f64()?);
        }
        Value::Null => return None,
        other => {
            let s = other.to_display_string();
            if !parse_date_or_time(&s, &mut p) {
                return None;
            }
        }
    }
    for (i, m) in modifiers.iter().enumerate() {
        let s = match m {
            Value::Null => return None,
            v => v.to_display_string(),
        };
        if !parse_modifier(&s, &mut p, i + 1) {
            return None;
        }
    }
    p.compute_jd();
    if p.error || !(0..=MAX_IJD).contains(&p.ijd) {
        return None;
    }
    // A bare YYYY-MM-DD with day-of-month overflow is normalized via the JD.
    if modifiers.is_empty() && p.valid_ymd && p.d > 28 {
        p.valid_ymd = false;
    }
    Some(p)
}

fn split_args(args: &[Value]) -> Option<(&Value, &[Value])> {
    match args.split_first() {
        Some((first, rest)) => Some((first, rest)),
        // No arguments → current time.
        None => None,
    }
}

fn current() -> DateTime {
    let mut p = DateTime::blank();
    set_to_current(&mut p);
    p
}

pub fn date(args: &[Value]) -> Value {
    let mut p = match split_args(args) {
        Some((f, m)) => match build(f, m) {
            Some(p) => p,
            None => return Value::Null,
        },
        None => current(),
    };
    p.compute_ymd();
    Value::Text(format!("{:04}-{:02}-{:02}", p.y, p.m, p.d))
}

pub fn time(args: &[Value]) -> Value {
    let mut p = match split_args(args) {
        Some((f, m)) => match build(f, m) {
            Some(p) => p,
            None => return Value::Null,
        },
        None => current(),
    };
    p.compute_hms();
    if p.use_subsec {
        let s = (1000.0 * p.s + 0.5) as i64;
        Value::Text(format!("{:02}:{:02}:{:02}.{:03}", p.h, p.min, s / 1000, s % 1000))
    } else {
        Value::Text(format!("{:02}:{:02}:{:02}", p.h, p.min, p.s as i64))
    }
}

pub fn datetime(args: &[Value]) -> Value {
    let mut p = match split_args(args) {
        Some((f, m)) => match build(f, m) {
            Some(p) => p,
            None => return Value::Null,
        },
        None => current(),
    };
    p.compute_ymd_hms();
    let head = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        p.y, p.m, p.d, p.h, p.min
    );
    if p.use_subsec {
        let s = (1000.0 * p.s + 0.5) as i64;
        Value::Text(format!("{head}:{:02}.{:03}", s / 1000, s % 1000))
    } else {
        Value::Text(format!("{head}:{:02}", p.s as i64))
    }
}

pub fn julianday(args: &[Value]) -> Value {
    let mut p = match split_args(args) {
        Some((f, m)) => match build(f, m) {
            Some(p) => p,
            None => return Value::Null,
        },
        None => current(),
    };
    p.compute_jd();
    Value::Real(p.ijd as f64 / 86_400_000.0)
}

pub fn unixepoch(args: &[Value]) -> Value {
    let mut p = match split_args(args) {
        Some((f, m)) => match build(f, m) {
            Some(p) => p,
            None => return Value::Null,
        },
        None => current(),
    };
    p.compute_jd();
    if p.use_subsec {
        Value::Real((p.ijd - UNIX_EPOCH_MS) as f64 / 1000.0)
    } else {
        Value::Integer(p.ijd / 1000 - UNIX_EPOCH_MS / 1000)
    }
}

pub fn strftime(args: &[Value]) -> Value {
    let (fmt, rest) = match args.split_first() {
        Some((f, r)) => (f, r),
        None => return Value::Null,
    };
    if fmt.is_null() {
        return Value::Null;
    }
    let fmt = fmt.to_display_string();
    let mut p = match split_args(rest) {
        Some((f, m)) => match build(f, m) {
            Some(p) => p,
            None => return Value::Null,
        },
        None => current(),
    };
    p.compute_ymd_hms();
    match format_strftime(&fmt, &mut p) {
        Some(out) => Value::Text(out),
        None => Value::Null,
    }
}

fn format_strftime(fmt: &str, p: &mut DateTime) -> Option<String> {
    let mut out = String::with_capacity(fmt.len() + 16);
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let cf = match chars.next() {
            Some(c) => c,
            None => return Some(out),
        };
        match cf {
            'd' => out.push_str(&format!("{:02}", p.d)),
            'e' => out.push_str(&format!("{:2}", p.d)),
            'f' => {
                let s = p.s.min(59.999);
                out.push_str(&format!("{:06.3}", s));
            }
            'F' => out.push_str(&format!("{:04}-{:02}-{:02}", p.y, p.m, p.d)),
            'G' => out.push_str(&format!("{:04}", p.iso_thursday().y)),
            'g' => out.push_str(&format!("{:02}", p.iso_thursday().y % 100)),
            'V' => out.push_str(&format!("{:02}", p.iso_thursday().days_after_jan01() / 7 + 1)),
            'H' => out.push_str(&format!("{:02}", p.h)),
            'k' => out.push_str(&format!("{:2}", p.h)),
            'I' | 'l' => {
                let mut h = p.h;
                if h > 12 {
                    h -= 12;
                }
                if h == 0 {
                    h = 12;
                }
                if cf == 'I' {
                    out.push_str(&format!("{h:02}"));
                } else {
                    out.push_str(&format!("{h:2}"));
                }
            }
            'j' => out.push_str(&format!("{:03}", p.days_after_jan01() + 1)),
            'J' => out.push_str(&format_g(p.ijd as f64 / 86_400_000.0, 16)),
            'm' => out.push_str(&format!("{:02}", p.m)),
            'M' => out.push_str(&format!("{:02}", p.min)),
            'p' => out.push_str(if p.h >= 12 { "PM" } else { "AM" }),
            'P' => out.push_str(if p.h >= 12 { "pm" } else { "am" }),
            'R' => out.push_str(&format!("{:02}:{:02}", p.h, p.min)),
            's' => {
                if p.use_subsec {
                    out.push_str(&format!("{:.3}", (p.ijd - UNIX_EPOCH_MS) as f64 / 1000.0));
                } else {
                    out.push_str(&format!("{}", p.ijd / 1000 - UNIX_EPOCH_MS / 1000));
                }
            }
            'S' => out.push_str(&format!("{:02}", p.s as i64)),
            'T' => out.push_str(&format!("{:02}:{:02}:{:02}", p.h, p.min, p.s as i64)),
            'u' | 'w' => {
                let mut wd = p.days_after_sunday();
                if wd == 0 && cf == 'u' {
                    wd = 7;
                }
                out.push_str(&format!("{wd}"));
            }
            'U' => out.push_str(&format!(
                "{:02}",
                (p.days_after_jan01() - p.days_after_sunday() + 7) / 7
            )),
            'W' => out.push_str(&format!(
                "{:02}",
                (p.days_after_jan01() - p.days_after_monday() + 7) / 7
            )),
            'Y' => out.push_str(&format!("{:04}", p.y)),
            '%' => out.push('%'),
            _ => return None,
        }
    }
    Some(out)
}

/// Emulate C `printf("%.*g")` (significant digits, trailing zeros trimmed) —
/// used for the non-standard `%J` Julian-day strftime code.
fn format_g(v: f64, sig: usize) -> String {
    if v == 0.0 {
        return "0".to_string();
    }
    let exp = v.abs().log10().floor() as i32;
    if exp < -4 || exp >= sig as i32 {
        let mut s = format!("{:.*e}", sig - 1, v);
        if let Some(epos) = s.find('e') {
            let (mantissa, e) = s.split_at(epos);
            let mantissa = trim_frac(mantissa);
            s = format!("{mantissa}{e}");
        }
        s
    } else {
        let decimals = (sig as i32 - 1 - exp).max(0) as usize;
        trim_frac(&format!("{:.*}", decimals, v))
    }
}

fn trim_frac(s: &str) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s.to_string()
    }
}
