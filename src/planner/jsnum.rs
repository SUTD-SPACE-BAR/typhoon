//! JavaScript-compatible number formatting.
//!
//! Cyclone builds G-code with template strings, so every number goes through
//! `Number.prototype.toString`, usually after `parseFloat(x.toFixed(6))`.
//! These functions reproduce both exactly so the output matches cyclone
//! byte for byte.

/// `String(x)` / `${x}` in JavaScript (ECMAScript Number::toString, radix 10).
pub fn to_js_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if x == 0.0 {
        // Covers -0 as well: JS prints it as "0".
        return "0".into();
    }

    // Rust's `{:e}` gives the shortest round-tripping digits, like JS.
    let sci = format!("{:e}", x.abs());
    let (mantissa, exp) = sci.split_once('e').expect("LowerExp always has an exponent");
    let exp: i32 = exp.parse().expect("valid exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let digits = prefer_even_on_tie(x.abs(), digits);
    let k = digits.len() as i32;
    let n = exp + 1; // position of the decimal point relative to the digits

    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        let (int, frac) = digits.split_at(n as usize);
        format!("{int}.{frac}")
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let sign = if e < 0 { '-' } else { '+' };
        let (first, rest) = digits.split_at(1);
        if rest.is_empty() {
            format!("{first}e{sign}{}", e.abs())
        } else {
            format!("{first}.{rest}e{sign}{}", e.abs())
        }
    };

    if x < 0.0 { format!("-{body}") } else { body }
}

/// When `x` lies exactly halfway between two equally short decimal
/// candidates, ECMAScript picks the one with an even last digit; Rust's
/// shortest-digits algorithm may pick the odd one. This can only happen with
/// 16+ significant digits, so cheaper cases skip the exact check.
fn prefer_even_on_tie(x_abs: f64, digits: String) -> String {
    let k = digits.len();
    let last = digits.as_bytes()[k - 1] - b'0';
    if k < 16 || last.is_multiple_of(2) {
        return digits;
    }
    // Exact decimal expansion (an f64 has at most 767 significant digits).
    let exact = format!("{:.800e}", x_abs);
    let exact_digits: String = exact
        .split_once('e')
        .unwrap()
        .0
        .chars()
        .filter(|c| *c != '.')
        .collect();
    let rest = &exact_digits[k..];
    let is_tie = rest.starts_with('5') && rest[1..].bytes().all(|b| b == b'0');
    if !is_tie {
        return digits;
    }
    // The two candidates are the exact digits truncated, and that plus one in
    // the last place. Ours is odd; switch to its even neighbour if no carry is needed.
    let truncated = &exact_digits[..k];
    let mut alt = digits.into_bytes();
    if truncated.as_bytes()[k - 1] == alt[k - 1] {
        if alt[k - 1] == b'9' {
            return String::from_utf8(alt).unwrap();
        }
        alt[k - 1] += 1;
    } else {
        alt[k - 1] -= 1;
    }
    String::from_utf8(alt).unwrap()
}

/// `x.toFixed(6)` in JavaScript.
///
/// Rust's `{:.6}` rounds exact ties to even; JS rounds them up (away from
/// zero, since the sign is handled first). Ties only happen when `x * 1e6`
/// is exactly half an integer, i.e. when `x * 128` is an odd integer.
pub fn to_fixed_6(x: f64) -> String {
    if !x.is_finite() || x.abs() >= 1e21 {
        return to_js_string(x);
    }
    if x < 0.0 {
        return format!("-{}", to_fixed_6(-x));
    }
    if x == 0.0 {
        // -0 is not < 0 in JS, so it prints without a sign; Rust would print "-0.000000".
        return "0.000000".into();
    }
    let m = x * 128.0; // exact: scaling by a power of two
    if m.fract() == 0.0 && (m as u128) % 2 == 1 {
        // x * 1e6 = m * 7812.5, so round half up gives (m * 15625 + 1) / 2.
        let n = (m as u128 * 15_625).div_ceil(2);
        return format!("{}.{:06}", n / 1_000_000, n % 1_000_000);
    }
    format!("{x:.6}")
}

/// Cyclone's `stripPrecision(x)`: `Number.parseFloat(x.toFixed(6))`.
pub fn strip_precision(x: f64) -> f64 {
    to_fixed_6(x).parse().unwrap_or(x)
}

/// What cyclone writes for a coordinate: `${stripPrecision(x)}`.
pub fn gcode_number(x: f64) -> String {
    to_js_string(strip_precision(x))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Expected strings were produced by Node.js (v20) from the same inputs.
    #[test]
    fn matches_node() {
        let vectors = include_str!("../../tests/fixtures/jsnum_vectors.txt");
        let mut checked = 0;
        for line in vectors.lines().filter(|l| !l.trim().is_empty()) {
            let mut parts = line.split('\t');
            let bits = u64::from_str_radix(parts.next().unwrap(), 16).unwrap();
            let x = f64::from_bits(bits);
            let js_string = parts.next().unwrap();
            let js_fixed = parts.next().unwrap();
            let js_gcode = parts.next().unwrap();
            assert_eq!(to_js_string(x), js_string, "String({x:?})");
            assert_eq!(to_fixed_6(x), js_fixed, "({x:?}).toFixed(6)");
            assert_eq!(gcode_number(x), js_gcode, "`${{stripPrecision({x:?})}}`");
            checked += 1;
        }
        assert!(checked > 50, "only {checked} vectors");
    }
}
