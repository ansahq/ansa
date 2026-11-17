//! A small, bounded arithmetic evaluator.
//!
//! This is deliberately not a general expression language. It supports
//! decimal numbers, `+`, `-`, `*`, `/`, parentheses, and unary signs. It never
//! calls a shell, `eval`, a process, or a network service.
//!
//! Both input size and nesting depth are bounded so a pasted adversarial
//! expression cannot cause unbounded parser work or stack growth.

use std::fmt;

/// Maximum number of Unicode scalar values accepted in one expression.
pub const MAX_EXPRESSION_CHARS: usize = 256;

/// Maximum nested unary/parenthesized depth.
pub const MAX_EXPRESSION_DEPTH: usize = 32;

/// A deterministic arithmetic failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalcError {
    Empty,
    TooLong { limit: usize },
    TooDeep { limit: usize },
    UnexpectedCharacter { character: char, position: usize },
    UnexpectedEnd,
    ExpectedOperand,
    UnmatchedOpenParenthesis,
    UnmatchedCloseParenthesis,
    DivisionByZero,
    NonFiniteResult,
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
        }
    }
}

impl std::error::Error for CalcError {}

/// Evaluates a bounded arithmetic expression.
///
/// ```
/// use ansa_core::calculator::evaluate;
/// assert_eq!(evaluate("(1 + 2) * 3").unwrap(), 9.0);
/// ```
pub fn evaluate(input: &str) -> Result<f64, CalcError> {
    let characters: Vec<char> = input.chars().take(MAX_EXPRESSION_CHARS + 1).collect();
    if characters.len() > MAX_EXPRESSION_CHARS {
        return Err(CalcError::TooLong {
            limit: MAX_EXPRESSION_CHARS,
        });
    }

    let mut parser = Parser {
        characters,
        position: 0,
        depth: 0,
    };
    parser.skip_whitespace();
    if parser.at_end() {
        return Err(CalcError::Empty);
    }

    let value = parser.parse_expression()?;
    parser.skip_whitespace();
    if let Some(character) = parser.peek() {
        return Err(if character == ')' {
            CalcError::UnmatchedCloseParenthesis
        } else {
            CalcError::UnexpectedCharacter {
                character,
                position: parser.position,
            }
        });
    }
    finite(value)
}

fn finite(value: f64) -> Result<f64, CalcError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(CalcError::NonFiniteResult)
    }
}

struct Parser {
    characters: Vec<char>,
    position: usize,
    depth: usize,
}

impl Parser {
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

    fn parse_expression(&mut self) -> Result<f64, CalcError> {
        let mut value = self.parse_term()?;
        loop {
            self.skip_whitespace();
            match self.peek() {
                Some('+') => {
                    self.advance();
                    let right = self.parse_term()?;
                    value = finite(value + right)?;
                }
                Some('-') => {
                    self.advance();
                    let right = self.parse_term()?;
                    value = finite(value - right)?;
                }
                _ => break,
            }
        }
        Ok(value)
    }

    fn parse_term(&mut self) -> Result<f64, CalcError> {
        let mut value = self.parse_unary()?;
        loop {
            self.skip_whitespace();
            match self.peek() {
                Some('*') => {
                    self.advance();
                    let right = self.parse_unary()?;
                    value = finite(value * right)?;
                }
                Some('/') => {
                    self.advance();
                    let right = self.parse_unary()?;
                    if right == 0.0 {
                        return Err(CalcError::DivisionByZero);
                    }
                    value = finite(value / right)?;
                }
                _ => break,
            }
        }
        Ok(value)
    }

    fn parse_unary(&mut self) -> Result<f64, CalcError> {
        self.skip_whitespace();
        match self.peek() {
            Some('-') => {
                self.advance();
                self.enter()?;
                let result = self.parse_unary();
                self.leave();
                finite(-result?)
            }
            Some('+') => {
                self.advance();
                self.enter()?;
                let result = self.parse_unary();
                self.leave();
                result
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<f64, CalcError> {
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
                    finite(value)
                } else {
                    Err(CalcError::UnmatchedOpenParenthesis)
                }
            }
            ')' => Err(CalcError::ExpectedOperand),
            '*' | '/' => Err(CalcError::ExpectedOperand),
            character if character.is_ascii_digit() || character == '.' => self.parse_number(),
            character => Err(CalcError::UnexpectedCharacter {
                character,
                position: self.position,
            }),
        }
    }

    fn parse_number(&mut self) -> Result<f64, CalcError> {
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

        let text: String = self.characters[start..self.position].iter().collect();
        text.parse::<f64>()
            .map_err(|_| CalcError::ExpectedOperand)
            .and_then(finite)
    }
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
}
