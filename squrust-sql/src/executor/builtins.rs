//! Miscellaneous SQLite scalar builtins: `printf`/`format`, `glob`, and the
//! `random`/`randomblob`/`zeroblob` functions.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::types::Value;

// ---- random ----------------------------------------------------------------

/// A process-wide xorshift64* PRNG, seeded from the clock. Good enough for
/// SQLite's `random()`/`randomblob()`, whose values are unspecified.
static RNG: AtomicU64 = AtomicU64::new(0);

fn next_u64() -> u64 {
    let mut x = RNG.load(Ordering::Relaxed);
    if x == 0 {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9e3779b97f4a7c15)
            | 1;
        x = seed;
    }
    // xorshift64*
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    RNG.store(x, Ordering::Relaxed);
    x.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

pub fn random() -> i64 {
    next_u64() as i64
}

pub fn randomblob(n: i64) -> Value {
    let n = n.max(1) as usize;
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        out.extend_from_slice(&next_u64().to_le_bytes());
    }
    out.truncate(n);
    Value::Blob(out)
}

pub fn zeroblob(n: i64) -> Value {
    Value::Blob(vec![0u8; n.max(0) as usize])
}

// ---- glob ------------------------------------------------------------------

/// SQLite `GLOB`: case-sensitive Unix-style matching with `*`, `?`, and
/// `[...]` / `[^...]` character classes.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_rec(&p, &t)
}

fn glob_rec(p: &[char], t: &[char]) -> bool {
    match p.first() {
        None => t.is_empty(),
        Some('*') => {
            // Match zero or more characters.
            glob_rec(&p[1..], t) || (!t.is_empty() && glob_rec(p, &t[1..]))
        }
        Some('?') => !t.is_empty() && glob_rec(&p[1..], &t[1..]),
        Some('[') => {
            if t.is_empty() {
                return false;
            }
            match class_match(&p[1..], t[0]) {
                Some((matched, rest_off)) => matched && glob_rec(&p[1 + rest_off..], &t[1..]),
                None => false, // malformed class: no match
            }
        }
        Some(&c) => !t.is_empty() && c == t[0] && glob_rec(&p[1..], &t[1..]),
    }
}

/// Match a character class starting just after `[`. Returns `(matched, offset)`
/// where `offset` is the number of pattern chars consumed up to and including
/// the closing `]`.
fn class_match(p: &[char], ch: char) -> Option<(bool, usize)> {
    let mut i = 0;
    let negate = p.first() == Some(&'^');
    if negate {
        i += 1;
    }
    let mut matched = false;
    let start = i;
    while i < p.len() {
        // A `]` as the very first class member is a literal.
        if p[i] == ']' && i > start {
            // Chars consumed within the post-`[` slice, up to and including `]`.
            return Some((matched != negate, i + 1));
        }
        // Range a-b.
        if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
            if p[i] <= ch && ch <= p[i + 2] {
                matched = true;
            }
            i += 3;
        } else {
            if p[i] == ch {
                matched = true;
            }
            i += 1;
        }
    }
    None // unterminated class
}

// ---- printf / format -------------------------------------------------------

pub fn printf(args: &[Value]) -> Value {
    let fmt = match args.first() {
        Some(Value::Null) | None => return Value::Null,
        Some(v) => v.to_display_string(),
    };
    let chars: Vec<char> = fmt.chars().collect();
    let mut out = String::new();
    let mut ai = 1usize; // args[0] is the format
    let mut i = 0usize;

    while i < chars.len() {
        if chars[i] != '%' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        i += 1;
        if i >= chars.len() {
            break;
        }
        // Flags.
        let (mut left, mut zero, mut plus, mut space, mut alt) =
            (false, false, false, false, false);
        while i < chars.len() {
            match chars[i] {
                '-' => left = true,
                '0' => zero = true,
                '+' => plus = true,
                ' ' => space = true,
                '#' => alt = true,
                _ => break,
            }
            i += 1;
        }
        // Width (digits or `*`).
        let width = read_num(&chars, &mut i, args, &mut ai);
        // Precision.
        let mut precision = None;
        if i < chars.len() && chars[i] == '.' {
            i += 1;
            precision = Some(read_num(&chars, &mut i, args, &mut ai).unwrap_or(0));
        }
        if i >= chars.len() {
            break;
        }
        let conv = chars[i];
        i += 1;

        let flags = Flags {
            left,
            zero,
            plus,
            space,
            alt,
            width,
            precision,
        };
        match conv {
            '%' => out.push('%'),
            'd' | 'i' => {
                let n = arg_at(args, &mut ai).and_then(|v| v.as_f64()).map(|f| f as i64).unwrap_or(0);
                out.push_str(&fmt_int(n as i128, 10, false, &flags));
            }
            'u' => {
                let n = arg_at(args, &mut ai).and_then(|v| v.as_f64()).map(|f| f as i64).unwrap_or(0);
                out.push_str(&fmt_int(n as u64 as i128, 10, false, &flags));
            }
            'x' | 'X' | 'o' => {
                let n = arg_at(args, &mut ai).and_then(|v| v.as_f64()).map(|f| f as i64).unwrap_or(0);
                let base = if conv == 'o' { 8 } else { 16 };
                let mut s = fmt_radix(n as u64, base, conv == 'X');
                if flags.alt && n != 0 {
                    s = match conv {
                        'o' => format!("0{s}"),
                        'x' => format!("0x{s}"),
                        _ => format!("0X{s}"),
                    };
                }
                out.push_str(&pad(s, &flags, false));
            }
            'c' => {
                // SQLite's %c renders the first character of the argument text.
                let s = arg_at(args, &mut ai).map(|v| v.to_display_string()).unwrap_or_default();
                out.push_str(&pad(s.chars().next().map(String::from).unwrap_or_default(), &flags, false));
            }
            's' => {
                let mut s = arg_at(args, &mut ai).map(|v| v.to_display_string()).unwrap_or_default();
                if let Some(p) = flags.precision {
                    s = s.chars().take(p).collect();
                }
                out.push_str(&pad(s, &flags, false));
            }
            'f' | 'e' | 'E' | 'g' | 'G' => {
                let x = arg_at(args, &mut ai).and_then(|v| v.as_f64()).unwrap_or(0.0);
                out.push_str(&fmt_float(x, conv, &flags));
            }
            other => {
                out.push('%');
                out.push(other);
            }
        }
    }
    Value::Text(out)
}

struct Flags {
    left: bool,
    zero: bool,
    plus: bool,
    space: bool,
    alt: bool,
    width: Option<usize>,
    precision: Option<usize>,
}

fn arg_at<'a>(args: &'a [Value], ai: &mut usize) -> Option<&'a Value> {
    let v = args.get(*ai);
    *ai += 1;
    v
}

fn read_num(chars: &[char], i: &mut usize, args: &[Value], ai: &mut usize) -> Option<usize> {
    if *i < chars.len() && chars[*i] == '*' {
        *i += 1;
        return arg_at(args, ai).and_then(|v| v.as_f64()).map(|f| f as usize);
    }
    let mut n = None;
    while *i < chars.len() && chars[*i].is_ascii_digit() {
        n = Some(n.unwrap_or(0) * 10 + (chars[*i] as usize - '0' as usize));
        *i += 1;
    }
    n
}

fn fmt_radix(n: u64, base: u32, upper: bool) -> String {
    if n == 0 {
        return "0".to_string();
    }
    let digits = b"0123456789abcdef";
    let mut v = n;
    let mut buf = Vec::new();
    while v > 0 {
        let d = digits[(v % base as u64) as usize];
        buf.push(if upper { d.to_ascii_uppercase() } else { d });
        v /= base as u64;
    }
    buf.reverse();
    String::from_utf8(buf).unwrap()
}

fn fmt_int(n: i128, base: u32, _unsigned: bool, flags: &Flags) -> String {
    let neg = n < 0;
    let mut digits = fmt_radix(n.unsigned_abs() as u64, base, false);
    if let Some(p) = flags.precision {
        while digits.len() < p {
            digits.insert(0, '0');
        }
    }
    let sign = if neg {
        "-"
    } else if flags.plus {
        "+"
    } else if flags.space {
        " "
    } else {
        ""
    };
    // Zero-padding only applies when there's no explicit precision.
    let zero = flags.zero && !flags.left && flags.precision.is_none();
    if zero {
        if let Some(w) = flags.width {
            let total = sign.len() + digits.len();
            if total < w {
                digits = format!("{}{}", "0".repeat(w - total), digits);
            }
        }
    }
    pad(format!("{sign}{digits}"), flags, true)
}

fn fmt_float(x: f64, conv: char, flags: &Flags) -> String {
    let prec = flags.precision.unwrap_or(6);
    let neg = x.is_sign_negative() && x != 0.0;
    let mag = x.abs();
    let body = match conv {
        'f' => format!("{mag:.prec$}"),
        'e' | 'E' => fmt_exp(mag, prec, conv == 'E'),
        _ => fmt_g(mag, if prec == 0 { 1 } else { prec }, conv == 'G'),
    };
    let sign = if neg {
        "-"
    } else if flags.plus {
        "+"
    } else if flags.space {
        " "
    } else {
        ""
    };
    let zero = flags.zero && !flags.left;
    let mut s = format!("{sign}{body}");
    if zero {
        if let Some(w) = flags.width {
            if s.len() < w {
                let pad0 = "0".repeat(w - s.len());
                s = format!("{sign}{pad0}{body}");
            }
        }
    }
    pad(s, flags, true)
}

fn fmt_exp(mag: f64, prec: usize, upper: bool) -> String {
    let s = format!("{mag:.prec$e}");
    let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let e: i32 = exp.parse().unwrap_or(0);
    let ec = if upper { 'E' } else { 'e' };
    format!("{mant}{ec}{}{:02}", if e < 0 { '-' } else { '+' }, e.abs())
}

fn fmt_g(mag: f64, sig: usize, upper: bool) -> String {
    if mag == 0.0 {
        return "0".to_string();
    }
    let exp = mag.log10().floor() as i32;
    let s = if exp < -4 || exp >= sig as i32 {
        let e = fmt_exp(mag, sig.saturating_sub(1), upper);
        // Trim trailing zeros in the mantissa.
        trim_g(&e)
    } else {
        let decimals = (sig as i32 - 1 - exp).max(0) as usize;
        trim_g(&format!("{mag:.decimals$}"))
    };
    s
}

fn trim_g(s: &str) -> String {
    if let Some(epos) = s.find(['e', 'E']) {
        let (m, e) = s.split_at(epos);
        format!("{}{}", trim_frac(m), e)
    } else {
        trim_frac(s)
    }
}

fn trim_frac(s: &str) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s.to_string()
    }
}

/// Apply width padding (space, or right-align) to an already-signed body.
fn pad(s: String, flags: &Flags, _numeric: bool) -> String {
    match flags.width {
        Some(w) if s.chars().count() < w => {
            let fill = " ".repeat(w - s.chars().count());
            if flags.left {
                format!("{s}{fill}")
            } else {
                format!("{fill}{s}")
            }
        }
        _ => s,
    }
}
