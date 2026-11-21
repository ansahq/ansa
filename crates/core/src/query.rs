//! Explicit local-calculator routing without swallowing numeric application names.
use crate::calculator::{self, CalcError, CalcResult};

#[derive(Debug, PartialEq)]
pub enum QueryRoute {
    Search,
    /// The routed value keeps its unit and is meant to be displayed through
    /// [`CalcResult::format`], never through `f64::to_string`.
    Calculator(Result<CalcResult, CalcError>),
}

/// `=` explicitly requests arithmetic, including errors for unsupported syntax.
/// Otherwise route only digit-containing strings made entirely of arithmetic
/// characters. Names such as "7-Zip" and "1Password" remain application queries.
///
/// Unit expressions and constants still require the explicit `=` prefix,
/// because their letters overlap application names. The routed [`CalcResult`]
/// preserves the result unit and the documented display policy.
pub fn route_query(query: &str) -> QueryRoute {
    let query = query.trim();
    if let Some(expression) = query.strip_prefix('=') {
        return QueryRoute::Calculator(calculator::evaluate_extended(expression));
    }
    let mut has_digit = false;
    for character in query.chars().take(calculator::MAX_EXPRESSION_CHARS + 1) {
        has_digit |= character.is_ascii_digit();
        if !(character.is_ascii_digit()
            || character.is_whitespace()
            || matches!(
                character,
                '+' | '-' | '*' | '/' | '^' | '%' | '(' | ')' | '.'
            ))
        {
            return QueryRoute::Search;
        }
    }
    if has_digit {
        QueryRoute::Calculator(calculator::evaluate_extended(query))
    } else {
        QueryRoute::Search
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calculator::units::Unit;

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
        assert_eq!(
            route_query("(24+8)/2"),
            QueryRoute::Calculator(Ok(CalcResult::Number(16.0)))
        );
        assert_eq!(
            route_query(".5"),
            QueryRoute::Calculator(Ok(CalcResult::Number(0.5)))
        );
        assert_eq!(
            route_query("1/0"),
            QueryRoute::Calculator(Err(CalcError::DivisionByZero))
        );
        assert_eq!(
            route_query("1+"),
            QueryRoute::Calculator(Err(CalcError::UnexpectedEnd))
        );
        assert_eq!(
            route_query("50%"),
            QueryRoute::Calculator(Ok(CalcResult::Number(0.5)))
        );
        assert_eq!(
            route_query("2^3"),
            QueryRoute::Calculator(Ok(CalcResult::Number(8.0)))
        );
    }

    #[test]
    fn equals_explicitly_requests_arithmetic() {
        assert_eq!(
            route_query("= 2*3"),
            QueryRoute::Calculator(Ok(CalcResult::Number(6.0)))
        );
        assert_eq!(
            route_query("="),
            QueryRoute::Calculator(Err(CalcError::Empty))
        );
        assert!(matches!(
            route_query("=foo"),
            QueryRoute::Calculator(Err(CalcError::UnknownUnit { .. }))
        ));
        assert_eq!(
            route_query("=1e15"),
            QueryRoute::Calculator(Ok(CalcResult::Number(1e15)))
        );
    }

    #[test]
    fn routed_results_preserve_display_and_units() {
        let QueryRoute::Calculator(Ok(result)) = route_query("0.1 + 0.2") else {
            panic!("expected calculator result");
        };
        assert_eq!(result.format(), "0.3");
        assert_eq!(result.unit(), None);

        let QueryRoute::Calculator(Ok(result)) = route_query("=10 km to mi") else {
            panic!("expected calculator result");
        };
        assert_eq!(result.unit(), Some(Unit::Mile));
        assert_eq!(result.format(), "6.21371192237 mi");

        let QueryRoute::Calculator(Ok(result)) = route_query("=20 C to F") else {
            panic!("expected calculator result");
        };
        assert_eq!(result.unit(), Some(Unit::Fahrenheit));
        assert_eq!(result.format(), "68 F");
    }

    #[test]
    fn oversized_expressions_fail_without_evaluation() {
        assert!(matches!(
            route_query(&"1".repeat(10_000)),
            QueryRoute::Calculator(Err(CalcError::TooLong { .. }))
        ));
    }
}
