//! JavaScript semantics for the binary operators, and the type conversions
//! they rely on (ToNumber, ToBoolean, ToInt32, ...).
//!
//! Operators that can fail return `Result`: mixing BigInt with other types is
//! a TypeError, BigInt division by zero a RangeError, and so on.

use std::{
    borrow::Cow,
    cmp::Ordering,
    ops::{BitAnd, BitOr, BitXor, Div, Mul, Rem, Shl, Shr, Sub},
    rc::Rc,
};

use crate::{
    error::JSError,
    primitive::{JsValue},
};

/// A value after ToNumeric: arithmetic works on either, but never a mix.
enum Numeric {
    Number(f64),
    BigInt(i128),
}

const RADIX_PREFIXES: [(&str, u32); 3] = [("0x", 16), ("0o", 8), ("0b", 2)];

fn type_error(message: impl Into<String>) -> JSError {
    JSError::TypeError(message.into())
}

fn mix_error() -> JSError {
    type_error("Cannot mix BigInt and other types, use explicit conversions")
}

/// BigInts here are `i128` rather than arbitrary precision.
fn overflow() -> JSError {
    JSError::RangeError("Maximum BigInt size exceeded".into())
}

fn division_by_zero() -> JSError {
    JSError::RangeError("Division by zero".into())
}

/// JS whitespace for string-to-number conversion: Unicode whitespace, line
/// terminators and the byte order mark.
fn trim_js(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}')
}

/// The digits and radix of a `0x` / `0o` / `0b` literal.
fn radix_digits(s: &str) -> Option<(&str, u32)> {
    RADIX_PREFIXES.iter().find_map(|(prefix, radix)| {
        s.get(..2)
            .filter(|p| p.eq_ignore_ascii_case(prefix))
            .map(|_| (&s[2..], *radix))
    })
}

/// StringToNumber: `" 42 "` is 42, `""` is 0, `"0x1F"` is 31, `"1e3"` is 1000,
/// anything else NaN.
fn string_to_number(s: &str) -> f64 {
    let s = trim_js(s);
    if s.is_empty() {
        return 0.0;
    }
    if let Some((digits, radix)) = radix_digits(s) {
        let value = digits.chars().try_fold(0.0, |acc: f64, c| {
            c.to_digit(radix).map(|d| acc * radix as f64 + d as f64)
        });
        return match value {
            Some(v) if !digits.is_empty() => v,
            _ => f64::NAN,
        };
    }
    let (sign, unsigned) = match s.strip_prefix('-') {
        Some(rest) => (-1.0, rest),
        None => (1.0, s.strip_prefix('+').unwrap_or(s)),
    };
    if unsigned == "Infinity" {
        return sign * f64::INFINITY;
    }
    // Rust's parser also accepts `inf`, `infinity` and `nan`, which JS does
    // not, so only hand it plain decimal syntax.
    let decimal = unsigned.starts_with(|c: char| c.is_ascii_digit() || c == '.')
        && unsigned
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'));
    match unsigned.parse::<f64>() {
        Ok(n) if decimal => sign * n,
        _ => f64::NAN,
    }
}

/// StringToBigInt: like [`string_to_number`] but integers only. `None` when
/// the string isn't a valid BigInt (or doesn't fit in an `i128`).
fn string_to_bigint(s: &str) -> Option<i128> {
    let s = trim_js(s);
    if s.is_empty() {
        return Some(0);
    }
    match radix_digits(s) {
        Some((digits, radix))
            if !digits.is_empty() && digits.chars().all(|c| c.is_digit(radix)) =>
        {
            i128::from_str_radix(digits, radix).ok()
        }
        Some(_) => None,
        None => s.parse().ok(),
    }
}

/// Number::toString: `1e21` is `"1e+21"`, `0.000001` is `"0.000001"` but
/// `1e-7` is `"1e-7"`, `-0` is `"0"`.
pub fn number_to_string(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n == 0.0 {
        return "0".into();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    // `{:e}` gives the shortest round-tripping digits, e.g. `1.23e2`.
    let sci = format!("{:e}", n.abs());
    let (mantissa, exp) = sci.split_once('e').expect("`{:e}` always has an exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    // The decimal point goes after `point` digits.
    let point = exp.parse::<i32>().expect("`{:e}` exponent is an integer") + 1;
    let body = if k <= point && point <= 21 {
        format!("{digits}{}", "0".repeat((point - k) as usize))
    } else if 0 < point && point <= 21 {
        format!(
            "{}.{}",
            &digits[..point as usize],
            &digits[point as usize..]
        )
    } else if -6 < point && point <= 0 {
        format!("0.{}{digits}", "0".repeat((-point) as usize))
    } else {
        let sign = if point >= 1 { '+' } else { '-' };
        let rest = if k == 1 {
            String::new()
        } else {
            format!(".{}", &digits[1..])
        };
        format!("{}{rest}e{sign}{}", &digits[..1], (point - 1).abs())
    };
    if n < 0.0 { format!("-{body}") } else { body }
}

/// ToUint32: wraps to 32 bits; NaN and infinities become 0.
fn to_uint32(n: f64) -> u32 {
    if n.is_finite() {
        n.trunc().rem_euclid(4294967296.0) as u32
    } else {
        0
    }
}

/// ToInt32: like [`to_uint32`], reinterpreted as signed.
fn to_int32(n: f64) -> i32 {
    to_uint32(n) as i32
}

/// The shift count of `<<`, `>>` and `>>>`: only the low 5 bits count.
fn shift_count(n: f64) -> u32 {
    to_uint32(n) & 31
}

/// `a << by` for BigInts; a negative `by` shifts right.
fn bigint_shl(a: i128, by: i128) -> Result<i128, JSError> {
    if by < 0 {
        let by = by.unsigned_abs();
        return Ok(if by >= 128 {
            if a < 0 { -1 } else { 0 }
        } else {
            a >> by
        });
    }
    if a == 0 {
        return Ok(0);
    }
    if by >= 128 {
        return Err(overflow());
    }
    let shifted = a << by;
    if shifted >> by == a {
        Ok(shifted)
    } else {
        Err(overflow())
    }
}

/// `base ** exp`. `powf` follows C's `pow`, which returns 1 where JS returns
/// NaN: `1 ** NaN` and `(±1) ** ±Infinity`.
fn number_pow(base: f64, exp: f64) -> f64 {
    if exp.is_nan() || (base.abs() == 1.0 && exp.is_infinite()) {
        f64::NAN
    } else {
        base.powf(exp)
    }
}

fn bigint_pow(base: i128, exp: i128) -> Result<i128, JSError> {
    if exp < 0 {
        return Err(JSError::RangeError("Exponent must be non-negative".into()));
    }
    match base {
        0 | 1 => Ok(if exp == 0 { 1 } else { base }),
        -1 => Ok(if exp % 2 == 0 { 1 } else { -1 }),
        _ => u32::try_from(exp)
            .ok()
            .and_then(|e| base.checked_pow(e))
            .ok_or_else(overflow),
    }
}

/// Compare a BigInt with a Number by exact mathematical value. `None` if the
/// Number is NaN.
fn bigint_cmp_number(x: i128, y: f64) -> Option<Ordering> {
    let limit = 2f64.powi(127);
    if y.is_nan() {
        None
    } else if y >= limit {
        Some(Ordering::Less)
    } else if y < -limit {
        Some(Ordering::Greater)
    } else {
        let whole = y.trunc();
        let fraction = if y > whole {
            Ordering::Less
        } else if y < whole {
            Ordering::Greater
        } else {
            Ordering::Equal
        };
        Some(x.cmp(&(whole as i128)).then(fraction))
    }
}

impl JsValue {
    /// `null` or `undefined`: the values `??` and `?.` treat as missing.
    pub fn is_nullish(&self) -> bool {
        matches!(self, JsValue::Null | JsValue::Undefined)
    }

    /// ToBoolean: `false`, `0`, `NaN`, `""`, `0n`, `null` and `undefined` are
    /// falsy; everything else is truthy.
    pub fn to_boolean(&self) -> bool {
        match self {
            JsValue::Number(n) => *n != 0.0 && !n.is_nan(),
            JsValue::BigInt(n) => **n != 0,
            JsValue::Str(s) => !s.is_empty(),
            JsValue::Bool(b) => *b,
            JsValue::Null | JsValue::Undefined => false,
            JsValue::Symbol(_) | JsValue::Object(_) | JsValue::Function(_) => true,
        }
    }

    /// ToPrimitive. Objects and functions have no user-defined `valueOf` or
    /// `toString` in this VM, so they become their default string form.
    fn to_primitive(&self) -> Cow<'_, JsValue> {
        match self {
            JsValue::Object(_) => Cow::Owned(JsValue::Str("[object Object]".into())),
            JsValue::Function(f) => Cow::Owned(JsValue::Str(
                format!("function {f}() {{ [native code] }}").into(),
            )),
            other => Cow::Borrowed(other),
        }
    }

    /// ToString.
    pub fn to_js_string(&self) -> Result<String, JSError> {
        Ok(match self {
            JsValue::Number(n) => number_to_string(*n),
            JsValue::BigInt(n) => n.to_string(),
            JsValue::Str(s) => s.to_string(),
            JsValue::Bool(b) => b.to_string(),
            JsValue::Null => "null".into(),
            JsValue::Undefined => "undefined".into(),
            JsValue::Symbol(_) => {
                return Err(type_error("Cannot convert a Symbol value to a string"));
            }
            JsValue::Object(_) | JsValue::Function(_) => {
                return self.to_primitive().to_js_string();
            }
        })
    }

    /// ToNumber.
    pub fn to_number(&self) -> Result<f64, JSError> {
        match self {
            JsValue::Number(n) => Ok(*n),
            JsValue::Str(s) => Ok(string_to_number(s)),
            JsValue::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            JsValue::Null => Ok(0.0),
            JsValue::Undefined => Ok(f64::NAN),
            JsValue::BigInt(_) => Err(type_error("Cannot convert a BigInt value to a number")),
            JsValue::Symbol(_) => Err(type_error("Cannot convert a Symbol value to a number")),
            JsValue::Object(_) | JsValue::Function(_) => self.to_primitive().to_number(),
        }
    }

    /// ToNumeric: a BigInt stays a BigInt, anything else goes through
    /// ToNumber.
    fn to_numeric(&self) -> Result<Numeric, JSError> {
        match self {
            JsValue::BigInt(n) => Ok(Numeric::BigInt(**n)),
            other => other.to_number().map(Numeric::Number),
        }
    }
}

/// An arithmetic or bitwise operator: `number` when both sides are Numbers,
/// `bigint` when both are BigInts, a TypeError for a mix.
fn numeric_op(
    lhs: &JsValue,
    rhs: &JsValue,
    number: impl FnOnce(f64, f64) -> f64,
    bigint: impl FnOnce(i128, i128) -> Result<i128, JSError>,
) -> Result<JsValue, JSError> {
    match (lhs.to_numeric()?, rhs.to_numeric()?) {
        (Numeric::Number(a), Numeric::Number(b)) => Ok(JsValue::Number(number(a, b))),
        (Numeric::BigInt(a), Numeric::BigInt(b)) => Ok(JsValue::bigint(bigint(a, b)?)),
        _ => Err(mix_error()),
    }
}

/// IsLessThan: `None` when the comparison is undefined, i.e. a NaN is
/// involved, which makes `<`, `>`, `<=` and `>=` all false.
fn less_than(a: &JsValue, b: &JsValue) -> Result<Option<bool>, JSError> {
    let (a, b) = (a.to_primitive(), b.to_primitive());
    match (a.as_ref(), b.as_ref()) {
        // Two strings compare by UTF-16 code units, not numerically:
        // "10" < "9".
        (JsValue::Str(x), JsValue::Str(y)) => {
            return Ok(Some(x.encode_utf16().lt(y.encode_utf16())));
        }
        (JsValue::BigInt(x), JsValue::Str(y)) => {
            return Ok(string_to_bigint(y).map(|y| **x < y));
        }
        (JsValue::Str(x), JsValue::BigInt(y)) => {
            return Ok(string_to_bigint(x).map(|x| x < **y));
        }
        _ => {}
    }
    Ok(match (a.to_numeric()?, b.to_numeric()?) {
        (Numeric::Number(x), Numeric::Number(y)) => x.partial_cmp(&y).map(Ordering::is_lt),
        (Numeric::BigInt(x), Numeric::BigInt(y)) => Some(x < y),
        (Numeric::BigInt(x), Numeric::Number(y)) => bigint_cmp_number(x, y).map(Ordering::is_lt),
        (Numeric::Number(x), Numeric::BigInt(y)) => bigint_cmp_number(y, x).map(Ordering::is_gt),
    })
}

macro_rules! numeric_operator {
    ($trait:ident, $method:ident, $number:expr, $bigint:expr) => {
        impl $trait for JsValue {
            type Output = Result<JsValue, JSError>;

            fn $method(self, rhs: Self) -> Self::Output {
                numeric_op(&self, &rhs, $number, $bigint)
            }
        }
    };
}

numeric_operator!(Sub, sub, |a, b| a - b, |a: i128, b| a
    .checked_sub(b)
    .ok_or_else(overflow));
numeric_operator!(Mul, mul, |a, b| a * b, |a: i128, b| a
    .checked_mul(b)
    .ok_or_else(overflow));
numeric_operator!(Div, div, |a, b| a / b, |a: i128, b| match b {
    0 => Err(division_by_zero()),
    _ => a.checked_div(b).ok_or_else(overflow),
});
// Both truncate, so the result takes the dividend's sign: `-7 % 3` is -1.
numeric_operator!(Rem, rem, |a, b| a % b, |a: i128, b| match b {
    0 => Err(division_by_zero()),
    _ => Ok(a.wrapping_rem(b)),
});
numeric_operator!(
    BitAnd,
    bitand,
    |a, b| (to_int32(a) & to_int32(b)) as f64,
    |a: i128, b| Ok(a & b)
);
numeric_operator!(
    BitOr,
    bitor,
    |a, b| (to_int32(a) | to_int32(b)) as f64,
    |a: i128, b| Ok(a | b)
);
numeric_operator!(
    BitXor,
    bitxor,
    |a, b| (to_int32(a) ^ to_int32(b)) as f64,
    |a: i128, b| Ok(a ^ b)
);
numeric_operator!(
    Shl,
    shl,
    |a, b| (to_int32(a) << shift_count(b)) as f64,
    bigint_shl
);
numeric_operator!(
    Shr,
    shr,
    |a, b| (to_int32(a) >> shift_count(b)) as f64,
    |a: i128, b: i128| bigint_shl(a, b.checked_neg().unwrap_or(i128::MAX))
);

impl JsValue {
    /// `self ** rhs`
    pub fn pow(&self, rhs: &Self) -> Result<Self, JSError> {
        numeric_op(self, rhs, number_pow, bigint_pow)
    }

    /// `self >>> rhs`: shifts in zeros, so the result is always non-negative.
    pub fn unsigned_shr(&self, rhs: &Self) -> Result<Self, JSError> {
        numeric_op(
            self,
            rhs,
            |a, b| (to_uint32(a) >> shift_count(b)) as f64,
            |_, _| {
                Err(type_error(
                    "BigInts have no unsigned right shift, use >> instead",
                ))
            },
        )
    }

    /// `self < rhs`
    pub fn less_than(&self, rhs: &Self) -> Result<Self, JSError> {
        Ok(less_than(self, rhs)?.unwrap_or(false).into())
    }

    /// `self > rhs`
    pub fn greater_than(&self, rhs: &Self) -> Result<Self, JSError> {
        Ok(less_than(rhs, self)?.unwrap_or(false).into())
    }

    /// `self <= rhs`: not `rhs < self`, unless a NaN makes both false.
    pub fn less_eq(&self, rhs: &Self) -> Result<Self, JSError> {
        Ok(less_than(rhs, self)?.is_some_and(|gt| !gt).into())
    }

    /// `self >= rhs`
    pub fn greater_eq(&self, rhs: &Self) -> Result<Self, JSError> {
        Ok(less_than(self, rhs)?.is_some_and(|lt| !lt).into())
    }

    /// `self === rhs`: no type conversion. `NaN !== NaN`, `0 === -0`, and
    /// objects are equal only to themselves.
    pub fn strict_equals(&self, rhs: &Self) -> bool {
        match (self, rhs) {
            (JsValue::Number(a), JsValue::Number(b)) => a == b,
            (JsValue::BigInt(a), JsValue::BigInt(b)) => a == b,
            (JsValue::Str(a), JsValue::Str(b)) => a == b,
            (JsValue::Bool(a), JsValue::Bool(b)) => a == b,
            (JsValue::Symbol(a), JsValue::Symbol(b)) => a == b,
            (JsValue::Null, JsValue::Null) | (JsValue::Undefined, JsValue::Undefined) => {
                true
            }
            (JsValue::Object(a), JsValue::Object(b)) => Rc::ptr_eq(a, b),
            (JsValue::Function(f1), JsValue::Function(f2)) => f1 == f2,
            _ => false,
        }
    }

    /// `self == rhs`: converts types first. `1 == "1"`, `0 == ""`,
    /// `null == undefined`, but `null != 0`.
    pub fn loose_equals(&self, rhs: &Self) -> bool {
        use JsValue as P;
        let (x, y) = (self, rhs);
        if std::mem::discriminant(x) == std::mem::discriminant(y) {
            return self.strict_equals(rhs);
        }
        match (x, y) {
            (P::Null | P::Undefined, P::Null | P::Undefined) => true,
            (P::Null | P::Undefined, _) | (_, P::Null | P::Undefined) => false,
            (P::Number(n), P::Str(s)) | (P::Str(s), P::Number(n)) => *n == string_to_number(s),
            (P::BigInt(n), P::Str(s)) | (P::Str(s), P::BigInt(n)) => {
                string_to_bigint(s) == Some(**n)
            }
            (P::Bool(b), _) => JsValue::from(if *b { 1.0 } else { 0.0 }).loose_equals(rhs),
            (_, P::Bool(b)) => self.loose_equals(&JsValue::from(if *b { 1.0 } else { 0.0 })),
            (P::Object(_) | P::Function(_), P::Object(_) | P::Function(_)) => false,
            (P::Object(_) | P::Function(_), _) => {
                x.to_primitive().loose_equals(rhs)
            }
            (_, P::Object(_) | P::Function(_)) => {
                self.loose_equals(&y.to_primitive())
            }
            (P::BigInt(a), P::Number(b)) | (P::Number(b), P::BigInt(a)) => {
                bigint_cmp_number(**a, *b) == Some(Ordering::Equal)
            }
            _ => false,
        }
    }

    /// `self && rhs`: `rhs` if `self` is truthy, else `self`.
    pub fn logical_and(self, rhs: Self) -> Self {
        if self.to_boolean() { rhs } else { self }
    }

    /// `self || rhs`: `self` if truthy, else `rhs`.
    pub fn logical_or(self, rhs: Self) -> Self {
        if self.to_boolean() { self } else { rhs }
    }

    /// `self ?? rhs`: `self` unless it's `null`/`undefined`.
    pub fn nullish_or(self, rhs: Self) -> Self {
        if self.is_nullish() { rhs } else { self }
    }

    /// `+self`: ToNumber, so a BigInt is a TypeError.
    pub fn unary_plus(&self) -> Result<Self, JSError> {
        Ok(JsValue::Number(self.to_number()?))
    }

    /// `-self`.
    pub fn negate(&self) -> Result<Self, JSError> {
        Ok(match self.to_numeric()? {
            Numeric::Number(n) => JsValue::Number(-n),
            Numeric::BigInt(n) => JsValue::bigint(n.checked_neg().ok_or_else(overflow)?),
        })
    }

    /// `~self`: bitwise not of the 32-bit integer value.
    pub fn bit_not(&self) -> Result<Self, JSError> {
        Ok(match self.to_numeric()? {
            Numeric::Number(n) => JsValue::Number(f64::from(!to_int32(n))),
            Numeric::BigInt(n) => JsValue::bigint(!n),
        })
    }

    /// `!self`.
    pub fn logical_not(&self) -> Self {
        JsValue::Bool(!self.to_boolean())
    }

    /// `typeof self`.
    pub fn type_name(&self) -> &'static str {
        match self {
            JsValue::Number(_) => "number",
            JsValue::BigInt(_) => "bigint",
            JsValue::Str(_) => "string",
            JsValue::Bool(_) => "boolean",
            JsValue::Undefined => "undefined",
            JsValue::Symbol(_) => "symbol",
            JsValue::Function(_) => "function",
            JsValue::Null | JsValue::Object(_) => "object",
        }
    }

    /// ToNumeric as a value: what `x++` evaluates to, the old value as a
    /// Number or BigInt.
    pub fn to_numeric_value(&self) -> Result<Self, JSError> {
        Ok(match self.to_numeric()? {
            Numeric::Number(n) => JsValue::Number(n),
            Numeric::BigInt(n) => JsValue::bigint(n),
        })
    }

    /// The value `++` (`by` = 1) or `--` (`by` = -1) stores: ToNumeric, then
    /// one more or less, keeping a BigInt a BigInt.
    pub fn step(&self, by: i8) -> Result<Self, JSError> {
        Ok(match self.to_numeric()? {
            Numeric::Number(n) => JsValue::Number(n + f64::from(by)),
            Numeric::BigInt(n) => {
                JsValue::bigint(n.checked_add(i128::from(by)).ok_or_else(overflow)?)
            }
        })
    }

    /// `self in rhs`: whether the object `rhs` has a property named `self`.
    pub fn is_in(&self, rhs: &Self) -> Result<Self, JSError> {
        let found = match rhs {
            JsValue::Object(object) => match self {
                // Properties are keyed by string; no object holds a symbol key.
                JsValue::Symbol(_) => false,
                key => object.has_prop(&key.to_js_string()?),
            },
            // Functions carry no properties in this VM.
            JsValue::Function(_) => false,
            other => {
                let key = self.to_js_string().unwrap_or_else(|_| "Symbol()".into());
                let target = other.to_js_string().unwrap_or_else(|_| "Symbol()".into());
                return Err(type_error(format!(
                    "Cannot use 'in' operator to search for '{key}' in {target}"
                )));
            }
        };
        Ok(found.into())
    }

    /// `self instanceof rhs`.
    pub fn instance_of(&self, rhs: &Self) -> Result<Self, JSError> {
        match rhs {
            // Objects have no prototype chain in this VM yet, so nothing is
            // an instance of a function. A primitive never is, in any VM.
            JsValue::Function(_) => Ok(false.into()),
            JsValue::Object(_) => Err(type_error(
                "Right-hand side of 'instanceof' is not callable",
            )),
            _ => Err(type_error(
                "Right-hand side of 'instanceof' is not an object",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{number_to_string, string_to_bigint, string_to_number};
    use crate::{error::JSError, primitive::JsValue};

    #[test]
    fn string_to_number_follows_js() {
        for (s, n) in [
            ("42", 42.0),
            ("  -1.5\n", -1.5),
            ("", 0.0),
            ("   ", 0.0),
            ("0x1F", 31.0),
            ("0b101", 5.0),
            ("0o17", 15.0),
            ("1e3", 1000.0),
            (".5", 0.5),
            ("5.", 5.0),
            ("+7", 7.0),
            ("Infinity", f64::INFINITY),
            ("-Infinity", f64::NEG_INFINITY),
        ] {
            assert_eq!(string_to_number(s), n, "{s:?}");
        }
        for s in [
            "abc", "inf", "infinity", "NaN", "1_000", "0x", "-0x10", "+-5", ".", "1e", "12px",
        ] {
            assert!(string_to_number(s).is_nan(), "{s:?}");
        }
    }

    #[test]
    fn string_to_bigint_follows_js() {
        assert_eq!(string_to_bigint(" 12 "), Some(12));
        assert_eq!(string_to_bigint("-12"), Some(-12));
        assert_eq!(string_to_bigint(""), Some(0));
        assert_eq!(string_to_bigint("0xff"), Some(255));
        assert_eq!(string_to_bigint("1.5"), None);
        assert_eq!(string_to_bigint("1e3"), None);
        assert_eq!(string_to_bigint("0x-1"), None);
    }

    #[test]
    fn number_to_string_follows_js() {
        for (n, s) in [
            (1.0, "1"),
            (-1.5, "-1.5"),
            (0.1 + 0.2, "0.30000000000000004"),
            (-0.0, "0"),
            (1e21, "1e+21"),
            (123e20, "1.23e+22"),
            (1e20, "100000000000000000000"),
            (0.000001, "0.000001"),
            (1e-7, "1e-7"),
            (-2.5e-8, "-2.5e-8"),
            (f64::NAN, "NaN"),
            (f64::NEG_INFINITY, "-Infinity"),
        ] {
            assert_eq!(number_to_string(n), s, "{n}");
        }
    }

    #[test]
    fn unary_ops_keep_bigints_bigints() {
        // The parser has no BigInt literals yet, so these aren't reachable
        // from JS.
        let b = JsValue::bigint(5);
        assert_eq!(b.negate().unwrap(), JsValue::bigint(-5));
        assert_eq!(b.bit_not().unwrap(), JsValue::bigint(-6));
        assert_eq!(b.step(1).unwrap(), JsValue::bigint(6));
        assert_eq!(b.step(-1).unwrap(), JsValue::bigint(4));
        assert_eq!(b.to_numeric_value().unwrap(), b);
        assert_eq!(b.type_name(), "bigint");
        assert!(matches!(b.unary_plus(), Err(JSError::TypeError(_))));
        assert!(matches!(
            JsValue::bigint(i128::MIN).negate(),
            Err(JSError::RangeError(_))
        ));
    }
}
