//! A small, bounded arithmetic and unit evaluator.
//!
//! This is deliberately not a general expression language. It supports
//! decimal numbers, `+`, `-`, `*`, `/`, `^` (right-associative
//! exponentiation), postfix `%`, parentheses, unary signs, two named
//! constants (`pi`, `e`) and a closed set of units. It never calls a shell,
//! `eval`, a process, or a network service.
//!
//! # Grammar
//!
//! ```text
//! expression := term (('+' | '-') term)*
//! term       := unary (('*' | '/') unary)*
//! unary      := ('+' | '-') unary | power
//! power      := postfix ('^' unary)?
//! postfix    := primary '%'*
//! primary    := number [unit] | constant | '(' expression ')'
//! ```
//!
//! `^` is right-associative: `2^3^2` is `2^(3^2) = 512`. Unary minus binds
//! outside exponentiation: `-2^2` is `-(2^2) = -4`.
//!
//! Number literals accept a bounded decimal exponent: `1e15`, `1.5e-3`,
//! `+2E4`. The written exponent magnitude is limited to
//! [`MAX_SCIENTIFIC_EXPONENT`].
//!
//! # Percent
//!
//! `%` is a postfix unary operator, never binary modulo. It divides the value
//! to its left by 100: `50%` is `0.5`, `200 * 10%` is `20`, and `50%%` is
//! `0.005`. Percent applies only to unitless numbers; `5 kg%` is rejected as
//! unsupported rather than being interpreted as modulo.
//!
//! # Constants
//!
//! `pi` and `e` are ASCII case-insensitive. There is no implicit
//! multiplication, so `2 pi` is rejected while `2 * pi` is accepted.
//!
//! # Units (bounded policy)
//!
//! Units are written as a suffix on a number literal: `10 km`, `5 kg`,
//! `20 C`. A trailing `to` converts the whole expression: `10 km to mi`,
//! `5 kg + 200 g to oz`, `20 C to F`. `to` is a top-level operator; write
//! `(5 kg + 200 g) to oz` rather than putting a conversion inside parentheses.
//! Addition and subtraction require both
//! operands to carry the same dimension and keep the left operand's unit.
//! Multiplying or dividing a quantity by a unitless number scales it;
//! products or quotients of two quantities, and powers of quantities, are
//! rejected as unsupported. Temperature quantities support conversion and
//! unary signs only; temperature arithmetic is rejected.
//!
//! Supported units, case-sensitive unless noted:
//!
//! - length: `m`, `km`, `cm`, `mm`, `in`, `ft`, `yd`, `mi`
//! - mass: `kg`, `g`, `mg`, `lb`, `oz`
//! - time: `s`, `min`, `h`, `day`
//! - temperature: `C`, `F`, `K` (also accepted lowercase `c`, `f`, `k`)
//!
//! Currencies, dates, time zones, arbitrary natural-language phrasing and
//! unit products are not supported. Inputs that look like dates, times or
//! currencies, and common number words, return [`CalcError::Unsupported`]
//! instead of being partially evaluated.
//!
//! # Display policy
//!
//! [`format_result`] is the deterministic display helper:
//!
//! - zero (including negative zero) formats as `"0"`;
//! - finite magnitudes in `[1e-9, 1e15)` format as plain decimal. The value is
//!   rounded to at most 12 significant digits (ties away from zero), then
//!   trailing fractional zeros are removed and the result is never scientific.
//!   At magnitudes of `1e12` and above this quantizes to tens, hundreds or
//!   thousands, so `123456789012345` displays as `123456789012000`;
//! - rounding can move a value up to the `1e15` boundary. When the rounded
//!   magnitude reaches `1e15` (for example `999999999999999.9`), the result is
//!   rendered with the scientific rule below instead of crossing the plain
//!   boundary;
//! - finite magnitudes outside that range use shortest lowercase scientific
//!   notation (for example `1e15`, `1e-10`);
//! - non-finite values use the standard `f64` spellings (`inf`, `-inf`,
//!   `NaN`).
//!
//! Both input size and nesting depth are bounded so a pasted adversarial
//! expression cannot cause unbounded parser work or stack growth.

use std::fmt;

/// Maximum number of Unicode scalar values accepted in one expression.
pub const MAX_EXPRESSION_CHARS: usize = 256;

/// Maximum nested unary, parenthesized or exponentiation depth.
pub const MAX_EXPRESSION_DEPTH: usize = 32;

/// Largest accepted exponent magnitude. Larger exponents are rejected before
/// `powf` runs.
pub const MAX_EXPONENT_MAGNITUDE: f64 = 1024.0;

/// Largest accepted written exponent magnitude in a number literal such as
/// `1e15` or `1.5e-3`.
pub const MAX_SCIENTIFIC_EXPONENT: i32 = 308;

/// Significant digits kept by [`format_result`] inside the plain range.
pub const FORMAT_SIGNIFICANT_DIGITS: usize = 12;

/// Upper bound (exclusive) of the plain-decimal display range.
pub const FORMAT_PLAIN_MAX_MAGNITUDE: f64 = 1e15;

/// Lower bound (inclusive) of the plain-decimal display range.
pub const FORMAT_PLAIN_MIN_MAGNITUDE: f64 = 1e-9;

const FORMAT_MAX_DECIMALS: usize = 20;

/// A diagnostic code for a [`CalcError`], stable across releases and suitable
/// for fixtures and machine-readable reports.
impl CalcError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::TooLong { .. } => "too_long",
            Self::TooDeep { .. } => "too_deep",
            Self::UnexpectedCharacter { .. } => "unexpected_character",
            Self::UnexpectedEnd => "unexpected_end",
            Self::ExpectedOperand => "expected_operand",
            Self::UnmatchedOpenParenthesis => "unmatched_open_parenthesis",
            Self::UnmatchedCloseParenthesis => "unmatched_close_parenthesis",
            Self::DivisionByZero => "division_by_zero",
            Self::NonFiniteResult => "non_finite_result",
            Self::ExponentOutOfRange => "exponent_out_of_range",
            Self::ScientificExponentOutOfRange { .. } => "scientific_exponent_out_of_range",
            Self::ExpectedUnit => "expected_unit",
            Self::UnitMismatch { .. } => "unit_mismatch",
            Self::UnknownUnit { .. } => "unknown_unit",
            Self::Unsupported { .. } => "unsupported",
        }
    }
}

/// A deterministic arithmetic failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalcError {
    Empty,
    TooLong {
        limit: usize,
    },
    TooDeep {
        limit: usize,
    },
    UnexpectedCharacter {
        character: char,
        position: usize,
    },
    UnexpectedEnd,
    ExpectedOperand,
    UnmatchedOpenParenthesis,
    UnmatchedCloseParenthesis,
    DivisionByZero,
    NonFiniteResult,
    /// The exponent's magnitude exceeds [`MAX_EXPONENT_MAGNITUDE`].
    ExponentOutOfRange,
    /// A scientific literal's written exponent magnitude exceeds
    /// [`MAX_SCIENTIFIC_EXPONENT`].
    ScientificExponentOutOfRange {
        limit: i32,
    },
    /// A conversion was requested but the left side carried no unit.
    ExpectedUnit,
    /// Two quantities with incompatible dimensions were combined, or a
    /// quantity was combined with a unitless number.
    UnitMismatch {
        left: &'static str,
        right: &'static str,
    },
    /// A unit suffix was not one of the supported symbols.
    UnknownUnit {
        unit: String,
    },
    /// The expression uses a documented-but-unsupported feature.
    Unsupported {
        reason: &'static str,
    },
}

impl fmt::Display for CalcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("enter an arithmetic expression"),
            Self::TooLong { limit } => {
                write!(f, "expression is longer than {limit} characters")
            }
            Self::TooDeep { limit } => {
                write!(f, "expression nests deeper than {limit} levels")
            }
            Self::UnexpectedCharacter {
                character,
                position,
            } => write!(
                f,
                "unexpected character {character:?} at position {position}"
            ),
            Self::UnexpectedEnd => f.write_str("expression ended unexpectedly"),
            Self::ExpectedOperand => f.write_str("expected a number or '('"),
            Self::UnmatchedOpenParenthesis => f.write_str("missing closing ')'"),
            Self::UnmatchedCloseParenthesis => f.write_str("unmatched closing ')'"),
            Self::DivisionByZero => f.write_str("division by zero"),
            Self::NonFiniteResult => f.write_str("result is not a finite number"),
            Self::ExponentOutOfRange => {
                write!(
                    f,
                    "exponent magnitude is larger than {MAX_EXPONENT_MAGNITUDE}"
                )
            }
            Self::ScientificExponentOutOfRange { limit } => {
                write!(f, "scientific exponent is larger than {limit}")
            }
            Self::ExpectedUnit => f.write_str("expected a quantity with a unit"),
            Self::UnitMismatch { left, right } => {
                write!(f, "cannot combine {left} and {right}")
            }
            Self::UnknownUnit { unit } => write!(f, "unknown unit {unit:?}"),
            Self::Unsupported { reason } => write!(f, "unsupported expression: {reason}"),
        }
    }
}

impl std::error::Error for CalcError {}

/// The result of evaluating an expression: either a plain number or a
/// quantity carrying a unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CalcResult {
    Number(f64),
    Quantity(units::Quantity),
}

impl CalcResult {
    /// The numeric magnitude, in the result's own unit.
    pub fn value(self) -> f64 {
        match self {
            Self::Number(value) => value,
            Self::Quantity(quantity) => quantity.value(),
        }
    }

    /// The result unit, if any.
    pub fn unit(self) -> Option<units::Unit> {
        match self {
            Self::Number(_) => None,
            Self::Quantity(quantity) => Some(quantity.unit()),
        }
    }

    /// Deterministic display text, including the unit symbol when present.
    pub fn format(self) -> String {
        match self {
            Self::Number(value) => format_result(value),
            Self::Quantity(quantity) => quantity.format(),
        }
    }
}

/// Evaluates a bounded arithmetic expression, returning only its numeric
/// magnitude. Unit expressions are accepted and return the converted value in
/// the result unit; use [`evaluate_extended`] or [`evaluate_quantity`] when
/// the unit itself matters.
///
/// ```
/// use ansa_core::calculator::evaluate;
/// assert_eq!(evaluate("(1 + 2) * 3").unwrap(), 9.0);
/// assert!((evaluate("10 km to mi").unwrap() - 6.213_711_922_373_34).abs() < 1e-12);
/// ```
pub fn evaluate(input: &str) -> Result<f64, CalcError> {
    Ok(evaluate_extended(input)?.value())
}

/// Evaluates a bounded expression and preserves whether the result carries a
/// unit.
pub fn evaluate_extended(input: &str) -> Result<CalcResult, CalcError> {
    let characters: Vec<char> = input.chars().take(MAX_EXPRESSION_CHARS + 1).collect();
    if characters.len() > MAX_EXPRESSION_CHARS {
        return Err(CalcError::TooLong {
            limit: MAX_EXPRESSION_CHARS,
        });
    }

    let bounded: String = characters.into_iter().collect();
    if let Some(error) = classify_unsupported(&bounded) {
        return Err(error);
    }

    Parser::new(bounded).parse_all()
}

/// Evaluates an expression that must produce a quantity with a unit.
pub fn evaluate_quantity(input: &str) -> Result<units::Quantity, CalcError> {
    match evaluate_extended(input)? {
        CalcResult::Quantity(quantity) => Ok(quantity),
        CalcResult::Number(_) => Err(CalcError::ExpectedUnit),
    }
}

/// Formats a value with the launcher display policy documented at the module
/// level. This function is total: it accepts non-finite values and produces a
/// deterministic string for them.
pub fn format_result(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    if !value.is_finite() {
        return value.to_string();
    }

    let magnitude = value.abs();
    if !(FORMAT_PLAIN_MIN_MAGNITUDE..FORMAT_PLAIN_MAX_MAGNITUDE).contains(&magnitude) {
        return format!("{value:e}");
    }

    let exponent = decimal_exponent(magnitude);
    let shift = FORMAT_SIGNIFICANT_DIGITS as i32 - 1 - exponent;
    let rounded = if shift >= 0 {
        let factor = pow10(shift);
        (value * factor).round() / factor
    } else {
        let factor = pow10(-shift);
        (value / factor).round() * factor
    };

    if rounded == 0.0 {
        return "0".to_owned();
    }
    // Rounding to 12 significant digits can move a value up to the plain
    // boundary (for example 999_999_999_999_999.9). Use the scientific rule
    // for the rounded value rather than crossing the documented plain bound.
    if !(FORMAT_PLAIN_MIN_MAGNITUDE..FORMAT_PLAIN_MAX_MAGNITUDE).contains(&rounded.abs()) {
        return format!("{rounded:e}");
    }

    let mut text = if shift >= 0 {
        let decimals = (shift as usize).min(FORMAT_MAX_DECIMALS);
        format!("{rounded:.decimals$}")
    } else {
        format!("{rounded:.0}")
    };
    if text.contains('.') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    if text == "-0" { "0".to_owned() } else { text }
}

fn decimal_exponent(magnitude: f64) -> i32 {
    let candidate = (magnitude.log10().floor() as i32).clamp(-323, 308);
    if pow10(candidate) > magnitude {
        candidate - 1
    } else if pow10(candidate + 1) <= magnitude {
        candidate + 1
    } else {
        candidate
    }
}

fn pow10(exponent: i32) -> f64 {
    10_f64.powi(exponent)
}

fn finite(value: f64) -> Result<f64, CalcError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(CalcError::NonFiniteResult)
    }
}

/// Closed unit catalogue and the conversion rules for the supported
/// dimensions.
pub mod units {
    use super::CalcError;

    /// The four supported physical dimensions.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Dimension {
        Length,
        Mass,
        Time,
        Temperature,
    }

    impl Dimension {
        /// Stable English name used in errors and diagnostics.
        pub fn name(self) -> &'static str {
            match self {
                Self::Length => "length",
                Self::Mass => "mass",
                Self::Time => "time",
                Self::Temperature => "temperature",
            }
        }
    }

    /// The closed set of supported unit symbols.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Unit {
        Meter,
        Kilometer,
        Centimeter,
        Millimeter,
        Inch,
        Foot,
        Yard,
        Mile,
        Kilogram,
        Gram,
        Milligram,
        Pound,
        Ounce,
        Second,
        Minute,
        Hour,
        Day,
        Celsius,
        Fahrenheit,
        Kelvin,
    }

    impl Unit {
        /// Canonical symbol shown to users.
        pub fn symbol(self) -> &'static str {
            match self {
                Self::Meter => "m",
                Self::Kilometer => "km",
                Self::Centimeter => "cm",
                Self::Millimeter => "mm",
                Self::Inch => "in",
                Self::Foot => "ft",
                Self::Yard => "yd",
                Self::Mile => "mi",
                Self::Kilogram => "kg",
                Self::Gram => "g",
                Self::Milligram => "mg",
                Self::Pound => "lb",
                Self::Ounce => "oz",
                Self::Second => "s",
                Self::Minute => "min",
                Self::Hour => "h",
                Self::Day => "day",
                Self::Celsius => "C",
                Self::Fahrenheit => "F",
                Self::Kelvin => "K",
            }
        }

        /// Physical dimension of this unit.
        pub fn dimension(self) -> Dimension {
            match self {
                Self::Meter
                | Self::Kilometer
                | Self::Centimeter
                | Self::Millimeter
                | Self::Inch
                | Self::Foot
                | Self::Yard
                | Self::Mile => Dimension::Length,
                Self::Kilogram | Self::Gram | Self::Milligram | Self::Pound | Self::Ounce => {
                    Dimension::Mass
                }
                Self::Second | Self::Minute | Self::Hour | Self::Day => Dimension::Time,
                Self::Celsius | Self::Fahrenheit | Self::Kelvin => Dimension::Temperature,
            }
        }

        /// Resolves a supported symbol. Temperature symbols also accept
        /// lowercase; every other symbol is case-sensitive.
        pub fn from_symbol(symbol: &str) -> Option<Self> {
            match symbol {
                "m" => Some(Self::Meter),
                "km" => Some(Self::Kilometer),
                "cm" => Some(Self::Centimeter),
                "mm" => Some(Self::Millimeter),
                "in" => Some(Self::Inch),
                "ft" => Some(Self::Foot),
                "yd" => Some(Self::Yard),
                "mi" => Some(Self::Mile),
                "kg" => Some(Self::Kilogram),
                "g" => Some(Self::Gram),
                "mg" => Some(Self::Milligram),
                "lb" => Some(Self::Pound),
                "oz" => Some(Self::Ounce),
                "s" => Some(Self::Second),
                "min" => Some(Self::Minute),
                "h" => Some(Self::Hour),
                "day" => Some(Self::Day),
                "C" | "c" => Some(Self::Celsius),
                "F" | "f" => Some(Self::Fahrenheit),
                "K" | "k" => Some(Self::Kelvin),
                _ => None,
            }
        }

        /// Converts `value` from this unit into `to`, rejecting incompatible
        /// dimensions.
        pub fn convert(self, value: f64, to: Self) -> Result<f64, CalcError> {
            if self.dimension() != to.dimension() {
                return Err(CalcError::UnitMismatch {
                    left: self.dimension().name(),
                    right: to.dimension().name(),
                });
            }
            if self.dimension() == Dimension::Temperature {
                Ok(convert_temperature(value, self, to))
            } else {
                Ok(value * self.linear_factor() / to.linear_factor())
            }
        }

        fn linear_factor(self) -> f64 {
            match self {
                Self::Meter => 1.0,
                Self::Kilometer => 1_000.0,
                Self::Centimeter => 0.01,
                Self::Millimeter => 0.001,
                Self::Inch => 0.0254,
                Self::Foot => 0.3048,
                Self::Yard => 0.9144,
                Self::Mile => 1_609.344,
                Self::Kilogram => 1.0,
                Self::Gram => 0.001,
                Self::Milligram => 0.000_001,
                Self::Pound => 0.453_592_37,
                Self::Ounce => 0.028_349_523_125,
                Self::Second => 1.0,
                Self::Minute => 60.0,
                Self::Hour => 3_600.0,
                Self::Day => 86_400.0,
                Self::Celsius | Self::Fahrenheit | Self::Kelvin => 1.0,
            }
        }
    }

    // Direct affine formulas avoid a Kelvin round-trip residual such as
    // `32 F -> 0.0000000000000568 C`.
    fn convert_temperature(value: f64, from: Unit, to: Unit) -> f64 {
        let celsius = match from {
            Unit::Celsius => value,
            Unit::Fahrenheit => (value - 32.0) * 5.0 / 9.0,
            Unit::Kelvin => value - 273.15,
            _ => value,
        };
        match to {
            Unit::Celsius => celsius,
            Unit::Fahrenheit => celsius * 9.0 / 5.0 + 32.0,
            Unit::Kelvin => celsius + 273.15,
            _ => celsius,
        }
    }

    /// A numeric magnitude paired with a supported unit.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Quantity {
        value: f64,
        unit: Unit,
    }

    impl Quantity {
        pub fn new(value: f64, unit: Unit) -> Self {
            Self { value, unit }
        }

        pub fn value(self) -> f64 {
            self.value
        }

        pub fn unit(self) -> Unit {
            self.unit
        }

        /// Deterministic display text, e.g. `"5.2 kg"`.
        pub fn format(self) -> String {
            format!(
                "{} {}",
                super::format_result(self.value),
                self.unit.symbol()
            )
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Value {
    Number(f64),
    Quantity { value: f64, unit: units::Unit },
}

impl Value {
    fn dimension_name(self) -> &'static str {
        match self {
            Self::Number(_) => "dimensionless",
            Self::Quantity { unit, .. } => unit.dimension().name(),
        }
    }
}

struct Parser {
    characters: Vec<char>,
    position: usize,
    depth: usize,
}

impl Parser {
    fn new(input: String) -> Self {
        Self {
            characters: input.chars().collect(),
            position: 0,
            depth: 0,
        }
    }

    fn parse_all(mut self) -> Result<CalcResult, CalcError> {
        self.skip_whitespace();
        if self.at_end() {
            return Err(CalcError::Empty);
        }

        let mut value = self.parse_expression()?;
        self.skip_whitespace();
        if self.peek_keyword("to") {
            self.consume_keyword("to");
            self.skip_whitespace();
            let target = self.parse_unit_word()?;
            value = match value {
                Value::Quantity { value, unit } => Value::Quantity {
                    value: unit.convert(value, target)?,
                    unit: target,
                },
                Value::Number(_) => {
                    return Err(CalcError::UnitMismatch {
                        left: "dimensionless",
                        right: target.dimension().name(),
                    });
                }
            };
        }

        self.skip_whitespace();
        if let Some(character) = self.peek() {
            return Err(if character == ')' {
                CalcError::UnmatchedCloseParenthesis
            } else {
                CalcError::UnexpectedCharacter {
                    character,
                    position: self.position,
                }
            });
        }

        Ok(match value {
            Value::Number(value) => CalcResult::Number(value),
            Value::Quantity { value, unit } => {
                CalcResult::Quantity(units::Quantity::new(value, unit))
            }
        })
    }

    fn at_end(&self) -> bool {
        self.position >= self.characters.len()
    }

    fn peek(&self) -> Option<char> {
        self.characters.get(self.position).copied()
    }

    fn advance(&mut self) {
        self.position += 1;
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(character) if character.is_whitespace()) {
            self.advance();
        }
    }

    fn enter(&mut self) -> Result<(), CalcError> {
        self.depth += 1;
        if self.depth > MAX_EXPRESSION_DEPTH {
            return Err(CalcError::TooDeep {
                limit: MAX_EXPRESSION_DEPTH,
            });
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn peek_keyword(&self, keyword: &str) -> bool {
        if !keyword.chars().enumerate().all(|(offset, character)| {
            self.characters.get(self.position + offset) == Some(&character)
        }) {
            return false;
        }
        !matches!(
            self.characters.get(self.position + keyword.chars().count()),
            Some(character) if character.is_ascii_alphanumeric() || *character == '_'
        )
    }

    fn consume_keyword(&mut self, keyword: &str) {
        self.position += keyword.chars().count();
    }

    fn parse_unit_word(&mut self) -> Result<units::Unit, CalcError> {
        self.skip_whitespace();
        let start = self.position;
        let Some(first) = self.peek() else {
            return Err(CalcError::UnexpectedEnd);
        };
        if !first.is_ascii_alphabetic() {
            return Err(CalcError::ExpectedUnit);
        }
        while matches!(self.peek(), Some(character) if character.is_ascii_alphabetic()) {
            self.advance();
        }
        let symbol: String = self.characters[start..self.position].iter().collect();
        units::Unit::from_symbol(&symbol).ok_or(CalcError::UnknownUnit { unit: symbol })
    }

    fn parse_expression(&mut self) -> Result<Value, CalcError> {
        let mut value = self.parse_term()?;
        loop {
            self.skip_whitespace();
            match self.peek() {
                Some('+') => {
                    self.advance();
                    let right = self.parse_term()?;
                    value = add(value, right, false)?;
                }
                Some('-') => {
                    self.advance();
                    let right = self.parse_term()?;
                    value = add(value, right, true)?;
                }
                _ => break,
            }
        }
        Ok(value)
    }

    fn parse_term(&mut self) -> Result<Value, CalcError> {
        let mut value = self.parse_unary()?;
        loop {
            self.skip_whitespace();
            match self.peek() {
                Some('*') => {
                    self.advance();
                    let right = self.parse_unary()?;
                    value = multiply(value, right, false)?;
                }
                Some('/') => {
                    self.advance();
                    let right = self.parse_unary()?;
                    value = multiply(value, right, true)?;
                }
                _ => break,
            }
        }
        Ok(value)
    }

    fn parse_unary(&mut self) -> Result<Value, CalcError> {
        self.skip_whitespace();
        match self.peek() {
            Some('-') => {
                self.advance();
                self.enter()?;
                let result = self.parse_unary();
                self.leave();
                negate(result?)
            }
            Some('+') => {
                self.advance();
                self.enter()?;
                let result = self.parse_unary();
                self.leave();
                result
            }
            _ => self.parse_power(),
        }
    }

    fn parse_power(&mut self) -> Result<Value, CalcError> {
        let base = self.parse_postfix()?;
        self.skip_whitespace();
        if self.peek() == Some('^') {
            self.advance();
            self.enter()?;
            let exponent = self.parse_unary();
            self.leave();
            return power(base, exponent?);
        }
        Ok(base)
    }

    fn parse_postfix(&mut self) -> Result<Value, CalcError> {
        let mut value = self.parse_primary()?;
        loop {
            self.skip_whitespace();
            if self.peek() == Some('%') {
                self.advance();
                value = percent(value)?;
            } else {
                break;
            }
        }
        Ok(value)
    }

    fn parse_primary(&mut self) -> Result<Value, CalcError> {
        self.skip_whitespace();
        let Some(character) = self.peek() else {
            return Err(if self.position == 0 {
                CalcError::Empty
            } else {
                CalcError::UnexpectedEnd
            });
        };

        match character {
            '(' => {
                self.advance();
                self.enter()?;
                let result = self.parse_expression();
                self.leave();
                let value = result?;
                self.skip_whitespace();
                if self.peek() == Some(')') {
                    self.advance();
                    Ok(value)
                } else {
                    Err(CalcError::UnmatchedOpenParenthesis)
                }
            }
            ')' => Err(CalcError::ExpectedOperand),
            '*' | '/' | '^' | '%' => Err(CalcError::ExpectedOperand),
            character if character.is_ascii_digit() || character == '.' => {
                self.parse_number_value()
            }
            character if character.is_ascii_alphabetic() => self.parse_identifier_value(),
            character => Err(CalcError::UnexpectedCharacter {
                character,
                position: self.position,
            }),
        }
    }

    fn parse_identifier_value(&mut self) -> Result<Value, CalcError> {
        let start = self.position;
        while matches!(self.peek(), Some(character) if character.is_ascii_alphabetic()) {
            self.advance();
        }
        let identifier: String = self.characters[start..self.position].iter().collect();
        if identifier.eq_ignore_ascii_case("pi") {
            Ok(Value::Number(std::f64::consts::PI))
        } else if identifier.eq_ignore_ascii_case("e") {
            Ok(Value::Number(std::f64::consts::E))
        } else if units::Unit::from_symbol(&identifier).is_some() {
            Err(CalcError::Unsupported {
                reason: "a unit must follow a number; unit quotients and standalone units are not supported",
            })
        } else {
            Err(CalcError::UnknownUnit { unit: identifier })
        }
    }

    fn parse_number_value(&mut self) -> Result<Value, CalcError> {
        let number = self.parse_number_literal()?;
        self.skip_whitespace();
        let start = self.position;
        let Some(first) = self.peek() else {
            return Ok(Value::Number(number));
        };
        if !first.is_ascii_alphabetic() {
            return Ok(Value::Number(number));
        }

        let mut end = start;
        while matches!(self.characters.get(end), Some(character) if character.is_ascii_alphabetic())
        {
            end += 1;
        }
        let identifier: String = self.characters[start..end].iter().collect();
        if identifier == "to"
            || identifier.eq_ignore_ascii_case("pi")
            || identifier.eq_ignore_ascii_case("e")
        {
            return Ok(Value::Number(number));
        }
        match units::Unit::from_symbol(&identifier) {
            Some(unit) => {
                self.position = end;
                Ok(Value::Quantity {
                    value: number,
                    unit,
                })
            }
            None => Err(CalcError::UnknownUnit { unit: identifier }),
        }
    }

    fn parse_number_literal(&mut self) -> Result<f64, CalcError> {
        let start = self.position;
        let mut seen_dot = false;
        let mut seen_digit = false;

        while let Some(character) = self.peek() {
            if character.is_ascii_digit() {
                seen_digit = true;
                self.advance();
            } else if character == '.' {
                if seen_dot {
                    return Err(CalcError::UnexpectedCharacter {
                        character,
                        position: self.position,
                    });
                }
                seen_dot = true;
                self.advance();
            } else {
                break;
            }
        }

        if !seen_digit {
            return Err(CalcError::ExpectedOperand);
        }

        if matches!(self.peek(), Some('e' | 'E')) {
            self.advance();
            if matches!(self.peek(), Some('+' | '-')) {
                self.advance();
            }
            let digits_start = self.position;
            let mut magnitude: i64 = 0;
            while let Some(character) = self.peek() {
                if !character.is_ascii_digit() {
                    break;
                }
                magnitude = magnitude
                    .saturating_mul(10)
                    .saturating_add(i64::from(character as u8 - b'0'));
                if magnitude > i64::from(MAX_SCIENTIFIC_EXPONENT) {
                    return Err(CalcError::ScientificExponentOutOfRange {
                        limit: MAX_SCIENTIFIC_EXPONENT,
                    });
                }
                self.advance();
            }
            if self.position == digits_start {
                return Err(match self.peek() {
                    Some(character) => CalcError::UnexpectedCharacter {
                        character,
                        position: self.position,
                    },
                    None => CalcError::UnexpectedEnd,
                });
            }
        }

        let text: String = self.characters[start..self.position].iter().collect();
        text.parse::<f64>()
            .map_err(|_| CalcError::ExpectedOperand)
            .and_then(finite)
    }
}

fn add(left: Value, right: Value, subtract: bool) -> Result<Value, CalcError> {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => {
            finite(if subtract { left - right } else { left + right }).map(Value::Number)
        }
        (
            Value::Quantity {
                value: left,
                unit: left_unit,
            },
            Value::Quantity {
                value: right,
                unit: right_unit,
            },
        ) => {
            if left_unit.dimension() != right_unit.dimension() {
                return Err(CalcError::UnitMismatch {
                    left: left_unit.dimension().name(),
                    right: right_unit.dimension().name(),
                });
            }
            if left_unit.dimension() == units::Dimension::Temperature {
                return Err(temperature_arithmetic());
            }
            let converted = right_unit.convert(right, left_unit)?;
            let result = if subtract {
                left - converted
            } else {
                left + converted
            };
            Ok(Value::Quantity {
                value: finite(result)?,
                unit: left_unit,
            })
        }
        (left, right) => Err(CalcError::UnitMismatch {
            left: left.dimension_name(),
            right: right.dimension_name(),
        }),
    }
}

fn multiply(left: Value, right: Value, divide: bool) -> Result<Value, CalcError> {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => {
            if divide {
                if right == 0.0 {
                    return Err(CalcError::DivisionByZero);
                }
                finite(left / right).map(Value::Number)
            } else {
                finite(left * right).map(Value::Number)
            }
        }
        (Value::Quantity { value, unit }, Value::Number(scalar)) => {
            if unit.dimension() == units::Dimension::Temperature {
                return Err(temperature_arithmetic());
            }
            if divide {
                if scalar == 0.0 {
                    return Err(CalcError::DivisionByZero);
                }
                Ok(Value::Quantity {
                    value: finite(value / scalar)?,
                    unit,
                })
            } else {
                Ok(Value::Quantity {
                    value: finite(value * scalar)?,
                    unit,
                })
            }
        }
        (Value::Number(scalar), Value::Quantity { value, unit }) => {
            if divide {
                return Err(CalcError::Unsupported {
                    reason: "dividing by a unit is not supported",
                });
            }
            if unit.dimension() == units::Dimension::Temperature {
                return Err(temperature_arithmetic());
            }
            Ok(Value::Quantity {
                value: finite(scalar * value)?,
                unit,
            })
        }
        (Value::Quantity { .. }, Value::Quantity { .. }) => Err(CalcError::Unsupported {
            reason: "products or quotients of units are not supported",
        }),
    }
}

fn power(base: Value, exponent: Value) -> Result<Value, CalcError> {
    match (base, exponent) {
        (Value::Number(base), Value::Number(exponent)) => {
            if !exponent.is_finite() {
                return Err(CalcError::NonFiniteResult);
            }
            if exponent.abs() > MAX_EXPONENT_MAGNITUDE {
                return Err(CalcError::ExponentOutOfRange);
            }
            if base == 0.0 && exponent < 0.0 {
                return Err(CalcError::DivisionByZero);
            }
            finite(base.powf(exponent)).map(Value::Number)
        }
        _ => Err(CalcError::Unsupported {
            reason: "powers of quantities are not supported",
        }),
    }
}

fn percent(value: Value) -> Result<Value, CalcError> {
    match value {
        Value::Number(value) => finite(value / 100.0).map(Value::Number),
        Value::Quantity { .. } => Err(CalcError::Unsupported {
            reason: "percent applies to unitless numbers",
        }),
    }
}

fn negate(value: Value) -> Result<Value, CalcError> {
    match value {
        Value::Number(value) => finite(-value).map(Value::Number),
        Value::Quantity { value, unit } => Ok(Value::Quantity {
            value: finite(-value)?,
            unit,
        }),
    }
}

fn temperature_arithmetic() -> CalcError {
    CalcError::Unsupported {
        reason: "temperature arithmetic is limited to conversion",
    }
}

const CURRENCY_SYMBOLS: &[char] = &['$', '€', '£', '¥', '₹', '₩', '₽', '₺', '₪', '₫', '₴'];

const CURRENCY_CODES: &[&str] = &[
    "usd", "eur", "gbp", "jpy", "cad", "aud", "chf", "cny", "inr", "krw", "rub", "brl", "nzd",
    "sek", "nok", "mxn", "sgd", "hkd", "zar", "try", "dollars", "euros", "cents", "rupees", "yen",
];

const DATE_WORDS: &[&str] = &[
    "today",
    "tomorrow",
    "yesterday",
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
    "utc",
    "gmt",
    "timezone",
    "tz",
    "noon",
    "midnight",
    "am",
    "pm",
];

const NATURAL_LANGUAGE_WORDS: &[&str] = &[
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
    "twenty",
    "thirty",
    "forty",
    "fifty",
    "sixty",
    "seventy",
    "eighty",
    "ninety",
    "hundred",
    "thousand",
    "million",
    "billion",
    "plus",
    "minus",
    "times",
    "multiplied",
    "divided",
    "over",
    "percent",
    "per",
    "mod",
    "modulo",
    "of",
    "by",
    "half",
    "dozen",
];

fn classify_unsupported(input: &str) -> Option<CalcError> {
    if input
        .chars()
        .any(|character| CURRENCY_SYMBOLS.contains(&character))
    {
        return Some(CalcError::Unsupported {
            reason: "currency conversion is not supported",
        });
    }

    let lower = input.to_ascii_lowercase();
    let tokens: Vec<&str> = lower
        .split(|character: char| !character.is_ascii_alphabetic() && !character.is_ascii_digit())
        .filter(|token| !token.is_empty())
        .collect();

    if tokens.iter().any(|token| CURRENCY_CODES.contains(token)) {
        return Some(CalcError::Unsupported {
            reason: "currency conversion is not supported",
        });
    }

    if looks_like_date_or_time(input) || tokens.iter().any(|token| DATE_WORDS.contains(token)) {
        return Some(CalcError::Unsupported {
            reason: "dates and time zones are not supported",
        });
    }

    if tokens
        .iter()
        .any(|token| NATURAL_LANGUAGE_WORDS.contains(token))
    {
        return Some(CalcError::Unsupported {
            reason: "natural-language arithmetic is not supported",
        });
    }

    None
}

fn looks_like_date_or_time(input: &str) -> bool {
    let trimmed = input.trim();
    if trimmed.contains(':') {
        return true;
    }

    let characters: Vec<char> = trimmed.chars().collect();
    if characters.len() >= 10
        && characters[..4].iter().all(char::is_ascii_digit)
        && matches!(characters[4], '-' | '/')
        && characters[5..7].iter().all(char::is_ascii_digit)
        && matches!(characters[7], '-' | '/')
        && characters[8..10].iter().all(char::is_ascii_digit)
    {
        return true;
    }

    for separator in ['-', '/'] {
        let parts: Vec<&str> = trimmed.split(separator).collect();
        if parts.len() != 3 {
            continue;
        }
        if !parts.iter().all(|part| {
            !part.is_empty() && part.chars().all(|character| character.is_ascii_digit())
        }) {
            continue;
        }
        let lengths: Vec<usize> = parts.iter().map(|part| part.chars().count()).collect();
        let year_first =
            lengths[0] == 4 && matches!(lengths[1], 1 | 2) && matches!(lengths[2], 1 | 2);
        let year_last = matches!(lengths[0], 1 | 2)
            && matches!(lengths[1], 1 | 2)
            && matches!(lengths[2], 2 | 4);
        if year_first || year_last {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn respects_precedence_and_parentheses() {
        assert_close(evaluate("1 + 2 * 3").unwrap(), 7.0);
        assert_close(evaluate("(1 + 2) * 3").unwrap(), 9.0);
        assert_close(evaluate("10 / 4").unwrap(), 2.5);
        assert_close(evaluate("2 * (3 + (4 - 1)) / 3").unwrap(), 4.0);
    }

    #[test]
    fn supports_unary_signs_and_decimals() {
        assert_close(evaluate("-3 + 5").unwrap(), 2.0);
        assert_close(evaluate("--2").unwrap(), 2.0);
        assert_close(evaluate("-(2 + 3)").unwrap(), -5.0);
        assert_close(evaluate(".5 + 1.5").unwrap(), 2.0);
        assert_close(evaluate("5. * 2").unwrap(), 10.0);
        assert_close(evaluate("  7\t-\n2 ").unwrap(), 5.0);
    }

    #[test]
    fn reports_division_by_zero() {
        assert_eq!(evaluate("1 / 0"), Err(CalcError::DivisionByZero));
        assert_eq!(evaluate("1 / (2 - 2)"), Err(CalcError::DivisionByZero));
        assert_eq!(evaluate("1 / -0.0"), Err(CalcError::DivisionByZero));
    }

    #[test]
    fn reports_malformed_expressions() {
        assert_eq!(evaluate(""), Err(CalcError::Empty));
        assert_eq!(evaluate("   "), Err(CalcError::Empty));
        assert_eq!(evaluate("2 +"), Err(CalcError::UnexpectedEnd));
        assert_eq!(evaluate("2 ** 3"), Err(CalcError::ExpectedOperand));
        assert_eq!(evaluate("(1 + 2"), Err(CalcError::UnmatchedOpenParenthesis));
        assert_eq!(
            evaluate("1 + 2)"),
            Err(CalcError::UnmatchedCloseParenthesis)
        );
        assert_eq!(
            evaluate("1.2.3"),
            Err(CalcError::UnexpectedCharacter {
                character: '.',
                position: 3
            })
        );
    }

    #[test]
    fn rejects_non_ascii_digits_without_panicking() {
        assert_eq!(
            evaluate("２ + 2"),
            Err(CalcError::UnexpectedCharacter {
                character: '２',
                position: 0
            })
        );
        assert_eq!(
            evaluate("1 + 漢"),
            Err(CalcError::UnexpectedCharacter {
                character: '漢',
                position: 4
            })
        );
    }

    #[test]
    fn bounds_input_length_and_depth() {
        let long = "1+".repeat(MAX_EXPRESSION_CHARS);
        assert!(matches!(evaluate(&long), Err(CalcError::TooLong { .. })));

        let mut nested = "(".repeat(MAX_EXPRESSION_DEPTH + 2);
        nested.push('1');
        nested.push_str(&")".repeat(MAX_EXPRESSION_DEPTH + 2));
        assert!(matches!(evaluate(&nested), Err(CalcError::TooDeep { .. })));
    }

    #[test]
    fn supports_percent_postfix() {
        assert_close(evaluate("50%").unwrap(), 0.5);
        assert_close(evaluate("200 * 10%").unwrap(), 20.0);
        assert_close(evaluate("50%%").unwrap(), 0.005);
        assert_close(evaluate("-50%").unwrap(), -0.5);
        assert_eq!(
            evaluate("5%3"),
            Err(CalcError::UnexpectedCharacter {
                character: '3',
                position: 2
            })
        );
    }

    #[test]
    fn supports_right_associative_exponentiation() {
        assert_close(evaluate("2^3").unwrap(), 8.0);
        assert_close(evaluate("2^3^2").unwrap(), 512.0);
        assert_close(evaluate("-2^2").unwrap(), -4.0);
        assert_close(evaluate("2^-1").unwrap(), 0.5);
        assert_eq!(evaluate("2^2000"), Err(CalcError::ExponentOutOfRange));
        assert_eq!(evaluate("10^400"), Err(CalcError::NonFiniteResult));
        assert_eq!(evaluate("(-2)^0.5"), Err(CalcError::NonFiniteResult));
    }

    #[test]
    fn supports_named_constants() {
        assert_close(evaluate("pi").unwrap(), std::f64::consts::PI);
        assert_close(evaluate("2 * PI").unwrap(), std::f64::consts::TAU);
        assert_close(evaluate("e").unwrap(), std::f64::consts::E);
    }

    #[test]
    fn converts_supported_units() {
        assert_close(evaluate("10 km to mi").unwrap(), 6.213_711_922_373_34);
        assert_close(evaluate("5 kg + 200 g").unwrap(), 5.2);
        assert_close(evaluate("20 C to F").unwrap(), 68.0);
        assert_close(evaluate("32 F to C").unwrap(), 0.0);
        assert_close(evaluate("1 day to h").unwrap(), 24.0);
        assert_close(evaluate("200 g + 1 kg").unwrap(), 1200.0);
    }

    #[test]
    fn reports_unit_errors() {
        assert_eq!(
            evaluate("10 km to kg"),
            Err(CalcError::UnitMismatch {
                left: "length",
                right: "mass"
            })
        );
        assert_eq!(
            evaluate("5 kg + 2 m"),
            Err(CalcError::UnitMismatch {
                left: "mass",
                right: "length"
            })
        );
        assert_eq!(
            evaluate("10 furlong to m"),
            Err(CalcError::UnknownUnit {
                unit: "furlong".to_owned()
            })
        );
        assert_eq!(
            evaluate("5 kg + 2"),
            Err(CalcError::UnitMismatch {
                left: "mass",
                right: "dimensionless"
            })
        );
        assert!(matches!(
            evaluate("20 C + 5 C"),
            Err(CalcError::Unsupported { .. })
        ));
        assert!(matches!(
            evaluate("10 km * 2 km"),
            Err(CalcError::Unsupported { .. })
        ));
        assert!(matches!(
            evaluate("2 kg ^ 2"),
            Err(CalcError::Unsupported { .. })
        ));
        assert!(matches!(
            evaluate("5 kg%"),
            Err(CalcError::Unsupported { .. })
        ));
    }

    #[test]
    fn rejects_unsupported_categories_explicitly() {
        for input in [
            "today",
            "2024-01-01",
            "12:30",
            "5 usd to eur",
            "$5",
            "five plus three",
        ] {
            assert!(
                matches!(evaluate(input), Err(CalcError::Unsupported { .. })),
                "{input}"
            );
        }
    }

    #[test]
    fn formats_results_deterministically() {
        assert_eq!(format_result(0.0), "0");
        assert_eq!(format_result(-0.0), "0");
        assert_eq!(format_result(1.0), "1");
        assert_eq!(format_result(0.1 + 0.2), "0.3");
        assert_eq!(format_result(1.0 / 3.0), "0.333333333333");
        assert_eq!(format_result(2.0 / 3.0), "0.666666666667");
        assert_eq!(format_result(1e-9), "0.000000001");
        assert_eq!(format_result(1e15), "1e15");
        assert_eq!(format_result(1e-10), "1e-10");
        assert_eq!(format_result(f64::INFINITY), "inf");
        assert_eq!(format_result(f64::NAN), "NaN");
    }

    #[test]
    fn formats_twelve_significant_digits_at_large_magnitudes() {
        assert_eq!(format_result(1e11), "100000000000");
        assert_eq!(format_result(1e12), "1000000000000");
        assert_eq!(format_result(123456789012.5), "123456789013");
        assert_eq!(format_result(1e14), "100000000000000");
        assert_eq!(format_result(123456789012345.0), "123456789012000");
        assert_eq!(format_result(123456789012500.0), "123456789013000");
        assert_eq!(format_result(-123456789012345.0), "-123456789012000");
        assert_eq!(format_result(999999999999999.9), "1e15");
        assert_eq!(format_result(-999999999999999.9), "-1e15");
        assert_eq!(format_result(1e15), "1e15");
    }

    #[test]
    fn parses_bounded_scientific_literals() {
        assert_close(evaluate("1e15").unwrap(), 1e15);
        assert_close(evaluate("1.5e-3").unwrap(), 0.0015);
        assert_close(evaluate("+2E4").unwrap(), 20_000.0);
        assert_close(evaluate("-2.5e2").unwrap(), -250.0);
        assert_eq!(
            evaluate("1e309"),
            Err(CalcError::ScientificExponentOutOfRange { limit: 308 })
        );
        assert_eq!(
            evaluate("1e-309"),
            Err(CalcError::ScientificExponentOutOfRange { limit: 308 })
        );
        assert_eq!(evaluate("1e"), Err(CalcError::UnexpectedEnd));
        assert_eq!(
            evaluate("1efoo"),
            Err(CalcError::UnexpectedCharacter {
                character: 'f',
                position: 2
            })
        );
    }

    #[test]
    fn formatted_results_round_trip_through_the_parser() {
        for value in [
            0.0,
            0.3,
            1e15,
            -1e15,
            1e-10,
            -1e-10,
            42.5,
            123456789012345.0,
            999999999999999.9,
            1.5e-3,
            2e4,
        ] {
            let formatted = format_result(value);
            let parsed = evaluate(&formatted)
                .unwrap_or_else(|error| panic!("{formatted:?} should parse, got {error:?}"));
            assert!(parsed.is_finite(), "{formatted:?}");
            assert_eq!(format_result(parsed), formatted);
        }
    }

    #[test]
    fn additive_percent_policy_is_explicit() {
        assert_close(evaluate("200 + 10%").unwrap(), 200.1);
        assert_close(evaluate("200 - 10%").unwrap(), 199.9);
    }

    #[test]
    fn unit_quotients_report_unsupported_not_unknown_unit() {
        for input in ["5 m/s", "5 m * s", "s", "5 kg / lb"] {
            assert!(
                matches!(evaluate(input), Err(CalcError::Unsupported { .. })),
                "{input}"
            );
        }
    }
}
