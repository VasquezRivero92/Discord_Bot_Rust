//! Las conversiones numéricas de JS que usan las rutas del legacy para sus
//! `limit` (`parseInt(x) || n`, `String(n)`, el `| 0` de protobufjs).
//!
//! Los números de JS son `f64`: `NaN` representa "no es un número".

/// Radix de `parseInt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Radix {
    /// `parseInt(s)`: base 10, o 16 si (tras el signo) empieza por `0x`/`0X`.
    Auto,
    /// `parseInt(s, 10)`: `0x10` vale 0.
    Ten,
}

/// `parseInt(s, radix)` de JS (ECMA-262 §19.2.5) sobre una cadena: espacios
/// iniciales de JS fuera, signo opcional, el prefijo más largo de dígitos
/// válidos; `NaN` si no hay ninguno. Nunca da `-0` distinto de `0` a efectos
/// del `|| n` (los dos son falsy). Cadenas decimales enormes dan `Infinity`,
/// como en JS.
pub(crate) fn parse_int(s: &str, radix: Radix) -> f64 {
    let t = s.trim_start_matches(is_js_whitespace);
    let (neg, t) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let (base, t) = match radix {
        Radix::Auto if t.starts_with("0x") || t.starts_with("0X") => (16, &t[2..]),
        _ => (10, t),
    };
    let len = t
        .bytes()
        .take_while(|b| (*b as char).is_digit(base))
        .count();
    let digits = &t[..len];
    if digits.is_empty() {
        return f64::NAN;
    }
    let v = if base == 10 {
        // Redondeo correcto, como V8 en base 10.
        digits.parse::<f64>().unwrap_or(f64::NAN)
    } else if let Ok(n) = u128::from_str_radix(digits, 16) {
        // Exacto hasta 32 cifras y un solo redondeo al pasar a f64.
        n as f64
    } else {
        digits
            .bytes()
            .filter_map(|b| (b as char).to_digit(16))
            .fold(0.0, |acc, d| acc * 16.0 + f64::from(d))
    };
    if neg {
        -v
    } else {
        v
    }
}

/// `x || default` para un número: `NaN` y `0` (también `-0`) son falsy.
pub(crate) fn or_default(x: f64, default: f64) -> f64 {
    if x.is_nan() || x == 0.0 {
        default
    } else {
        x
    }
}

/// `String(x)` para los números que salen de `parseInt` (enteros, `±Infinity`):
/// decimal sin exponente hasta `1e21` y notación `1.5e+22` desde ahí.
pub(crate) fn number_to_string(x: f64) -> String {
    if x.is_nan() {
        "NaN".to_owned()
    } else if x.is_infinite() {
        if x > 0.0 { "Infinity" } else { "-Infinity" }.to_owned()
    } else if x.abs() < 1e21 {
        // `Display` de f64 no usa exponente y escribe `5` para `5.0`.
        format!("{}", x + 0.0)
    } else {
        // Rust da la representación más corta (como JS) sin `+` en el exponente.
        let s = format!("{x:e}");
        s.replacen('e', "e+", 1)
    }
}

/// `ToInt32(x)` (el `d.value | 0` con que protobufjs codifica un `int32`).
pub(crate) fn to_int32(x: f64) -> i32 {
    if !x.is_finite() {
        return 0;
    }
    const TWO_32: f64 = 4_294_967_296.0;
    let m = x.trunc() % TWO_32;
    let m = if m < 0.0 { m + TWO_32 } else { m };
    if m >= 2_147_483_648.0 {
        (m - TWO_32) as i32
    } else {
        m as i32
    }
}

/// `StrWhiteSpaceChar` de ECMA-262: `WhiteSpace` (incluidos `U+FEFF` y la
/// categoría `Zs`) y `LineTerminator`.
fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{9}' | '\u{A}' | '\u{B}' | '\u{C}' | '\u{D}' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Valores comprobados con Node (`parseInt(x)` y `parseInt(x, 10)`).
    #[test]
    fn parse_int_matches_node() {
        let auto = |s: &str| parse_int(s, Radix::Auto);
        let ten = |s: &str| parse_int(s, Radix::Ten);
        assert_eq!(auto("50"), 50.0);
        assert!(auto("abc").is_nan());
        assert!(auto("").is_nan());
        assert!(auto(" ").is_nan());
        assert!(auto("-").is_nan());
        assert!(auto("+").is_nan());
        assert_eq!(auto("0"), 0.0);
        assert_eq!(auto("-5"), -5.0);
        assert_eq!(auto("7x"), 7.0);
        assert_eq!(auto("+5"), 5.0);
        assert_eq!(auto("  \t\n12"), 12.0);
        assert_eq!(auto("\u{FEFF}\u{A0}\u{3000}9"), 9.0);
        assert!(auto("\u{85}9").is_nan(), "NEL no es espacio en JS");
        assert_eq!(auto("007"), 7.0);
        assert_eq!(auto("3.9"), 3.0);
        assert_eq!(auto("-3.9"), -3.0);
        assert_eq!(auto("1e3"), 1.0);
        assert_eq!(auto("0x1A"), 26.0);
        assert_eq!(auto("-0X1a"), -26.0);
        assert!(auto("0x").is_nan());
        assert!(auto("0xg").is_nan());
        assert_eq!(ten("0x1A"), 0.0);
        assert_eq!(ten("5,7"), 5.0);
        assert_eq!(auto("99999999999"), 99_999_999_999.0);
        assert_eq!(auto("100000000000000000000000"), 1e23);
        assert_eq!(auto(&"9".repeat(400)), f64::INFINITY);
        assert_eq!(auto(&format!("-{}", "9".repeat(400))), f64::NEG_INFINITY);
        assert!(auto("٣").is_nan(), "solo dígitos ASCII");
    }

    #[test]
    fn or_default_treats_nan_and_zero_as_falsy() {
        assert_eq!(or_default(f64::NAN, 50.0), 50.0);
        assert_eq!(or_default(0.0, 50.0), 50.0);
        assert_eq!(or_default(-0.0, 50.0), 50.0);
        assert_eq!(or_default(-5.0, 50.0), -5.0);
        assert_eq!(or_default(f64::INFINITY, 50.0), f64::INFINITY);
    }

    #[test]
    fn number_to_string_matches_js() {
        assert_eq!(number_to_string(50.0), "50");
        assert_eq!(number_to_string(-5.0), "-5");
        assert_eq!(number_to_string(-0.0), "0");
        assert_eq!(number_to_string(1e20), "100000000000000000000");
        assert_eq!(number_to_string(1e21), "1e+21");
        assert_eq!(number_to_string(1.5e22), "1.5e+22");
        assert_eq!(number_to_string(-1e21), "-1e+21");
        assert_eq!(number_to_string(f64::INFINITY), "Infinity");
        assert_eq!(number_to_string(f64::NEG_INFINITY), "-Infinity");
    }

    #[test]
    fn to_int32_wraps_like_js() {
        assert_eq!(to_int32(5.0), 5);
        assert_eq!(to_int32(-5.0), -5);
        assert_eq!(to_int32(2_147_483_647.0), i32::MAX);
        assert_eq!(to_int32(2_147_483_648.0), i32::MIN);
        assert_eq!(to_int32(3_000_000_000.0), -1_294_967_296);
        assert_eq!(to_int32(4_294_967_301.0), 5);
        assert_eq!(to_int32(4_294_967_296.0), 0);
        assert_eq!(to_int32(-4_294_967_291.0), 5);
        assert_eq!(to_int32(1e21), -559_939_584);
        assert_eq!(to_int32(f64::INFINITY), 0);
    }
}
