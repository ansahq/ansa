//! Explicit local-calculator routing without swallowing numeric application names.
use crate::calculator::{self, CalcError};

#[derive(Debug, PartialEq)]
pub enum QueryRoute {
    Search,
    Calculator(Result<f64, CalcError>),
}

/// `=` explicitly requests arithmetic, including errors for unsupported syntax.
/// Otherwise route only digit-containing strings made entirely of arithmetic
/// characters. Names such as "7-Zip" and "1Password" remain application queries.
pub fn route_query(query: &str) -> QueryRoute {
    let query = query.trim();
    if let Some(expression) = query.strip_prefix('=') {
        return QueryRoute::Calculator(calculator::evaluate(expression));
    }
    let mut has_digit = false;
    for character in query.chars().take(calculator::MAX_EXPRESSION_CHARS + 1) {
        has_digit |= character.is_ascii_digit();
        if !(character.is_ascii_digit()
            || character.is_whitespace()
            || matches!(character, '+' | '-' | '*' | '/' | '(' | ')' | '.'))
        {
            return QueryRoute::Search;
        }
    }
    if has_digit {
        QueryRoute::Calculator(calculator::evaluate(query))
    } else {
        QueryRoute::Search
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_names_are_search_queries() {
        for input in [
            "",
            "terminal",
            "7-Zip",
            "1Password",
            "Project 42",
            "-",
            "😀",
        ] {
            assert_eq!(route_query(input), QueryRoute::Search, "{input}");
        }
    }

    #[test]
    fn arithmetic_routes_successes_and_errors_consistently() {
        assert_eq!(route_query("(24+8)/2"), QueryRoute::Calculator(Ok(16.)));
        assert_eq!(route_query(".5"), QueryRoute::Calculator(Ok(0.5)));
        assert_eq!(
            route_query("1/0"),
            QueryRoute::Calculator(Err(CalcError::DivisionByZero))
        );
        assert_eq!(
            route_query("1+"),
            QueryRoute::Calculator(Err(CalcError::UnexpectedEnd))
        );
    }

    #[test]
    fn equals_explicitly_requests_arithmetic() {
        assert_eq!(route_query("= 2*3"), QueryRoute::Calculator(Ok(6.)));
        assert_eq!(
            route_query("="),
            QueryRoute::Calculator(Err(CalcError::Empty))
        );
        assert!(matches!(
            route_query("=foo"),
            QueryRoute::Calculator(Err(_))
        ));
    }

    #[test]
    fn oversized_expressions_fail_without_evaluation() {
        assert!(matches!(
            route_query(&"1".repeat(10_000)),
            QueryRoute::Calculator(Err(CalcError::TooLong { .. }))
        ));
    }
}
