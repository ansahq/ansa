//! Deterministic fixture runner for the bounded calculator policy.
//!
//! The corpus is versioned data, not generated at test time. Every case is
//! evaluated through the public API and compared byte-for-byte.

use ansa_core::calculator::{
    CalcResult, evaluate, evaluate_extended, evaluate_quantity, format_result,
};
use ansa_core::query::{QueryRoute, route_query};
use serde_json::Value;

const CORPUS: &str = include_str!("../data/calculator_corpus.json");

fn corpus() -> Value {
    serde_json::from_str(CORPUS).expect("calculator corpus must be valid JSON")
}

fn array<'a>(corpus: &'a Value, key: &str) -> &'a [Value] {
    corpus[key]
        .as_array()
        .unwrap_or_else(|| panic!("corpus field {key:?} must be an array"))
}

#[test]
fn records_corpus_version_and_shape() {
    let corpus = corpus();
    let version = corpus["version"]
        .as_str()
        .expect("corpus must record a version");
    eprintln!("calculator corpus version: {version}");
    assert_eq!(version, "1.2.0");
    assert!(!array(&corpus, "valid").is_empty());
    assert!(!array(&corpus, "unit_valid").is_empty());
    assert!(!array(&corpus, "invalid").is_empty());
    assert!(!array(&corpus, "formatting").is_empty());
    assert!(!array(&corpus, "round_trip").is_empty());
}

#[test]
fn valid_arithmetic_cases_match_exactly() {
    let corpus = corpus();
    for case in array(&corpus, "valid") {
        let expression = case["expression"].as_str().expect("expression");
        let expected = case["formatted"].as_str().expect("formatted");
        let value = evaluate(expression).unwrap_or_else(|error| {
            panic!("{expression:?} should evaluate, got {error:?}");
        });
        assert_eq!(format_result(value), expected, "{expression:?}");
        assert!(
            matches!(evaluate_extended(expression), Ok(CalcResult::Number(_))),
            "{expression:?} should stay unitless"
        );
    }
}

#[test]
fn unit_cases_match_value_unit_and_display() {
    let corpus = corpus();
    for case in array(&corpus, "unit_valid") {
        let expression = case["expression"].as_str().expect("expression");
        let expected_value = case["formatted"].as_str().expect("formatted");
        let expected_unit = case["unit"].as_str().expect("unit");
        let quantity = evaluate_quantity(expression).unwrap_or_else(|error| {
            panic!("{expression:?} should evaluate, got {error:?}");
        });
        assert_eq!(quantity.unit().symbol(), expected_unit, "{expression:?}");
        assert_eq!(
            format_result(quantity.value()),
            expected_value,
            "{expression:?}"
        );
        assert_eq!(
            quantity.format(),
            format!("{expected_value} {expected_unit}"),
            "{expression:?}"
        );
        assert_eq!(
            evaluate(expression).unwrap(),
            quantity.value(),
            "{expression:?}"
        );
    }
}

#[test]
fn invalid_cases_match_typed_codes() {
    let corpus = corpus();
    for case in array(&corpus, "invalid") {
        let expression = case["expression"].as_str().expect("expression");
        let expected = case["code"].as_str().expect("code");
        let error = evaluate(expression)
            .err()
            .unwrap_or_else(|| panic!("{expression:?} should fail"));
        assert_eq!(error.code(), expected, "{expression:?} -> {error:?}");
        assert!(!error.to_string().is_empty(), "{expression:?}");
    }
}

#[test]
fn formatting_cases_match_exactly() {
    let corpus = corpus();
    for case in array(&corpus, "formatting") {
        let value = match &case["value"] {
            Value::Number(number) => number.as_f64().expect("finite number"),
            Value::String(text) if text == "-0" => -0.0,
            other => panic!("unsupported formatting value {other:?}"),
        };
        let expected = case["formatted"].as_str().expect("formatted");
        assert_eq!(format_result(value), expected, "{value:?}");
    }
}

#[test]
fn formatted_values_round_trip_through_parser_and_router() {
    let corpus = corpus();
    for case in array(&corpus, "round_trip") {
        let value = case["value"].as_f64().expect("finite number");
        let formatted = format_result(value);
        let parsed = evaluate(&formatted)
            .unwrap_or_else(|error| panic!("{formatted:?} should parse, got {error:?}"));
        assert!(parsed.is_finite(), "{formatted:?}");
        assert_eq!(format_result(parsed), formatted, "{value:?}");

        match route_query(&format!("={formatted}")) {
            QueryRoute::Calculator(Ok(result)) => {
                assert!(result.value().is_finite(), "{formatted:?}");
                assert_eq!(result.format(), formatted, "{formatted:?}");
            }
            other => panic!("{formatted:?} should route to the calculator, got {other:?}"),
        }
    }
}

#[test]
fn routed_results_use_the_display_policy_and_keep_units() {
    let QueryRoute::Calculator(Ok(result)) = route_query("0.1 + 0.2") else {
        panic!("expected a calculator result");
    };
    assert_eq!(result.format(), "0.3");

    let QueryRoute::Calculator(Ok(result)) = route_query("=10 km to mi") else {
        panic!("expected a unit result");
    };
    assert_eq!(result.unit().map(|unit| unit.symbol()), Some("mi"));
    assert_eq!(result.format(), "6.21371192237 mi");

    let QueryRoute::Calculator(Ok(result)) = route_query("=20 C to F") else {
        panic!("expected a temperature result");
    };
    assert_eq!(result.unit().map(|unit| unit.symbol()), Some("F"));
    assert_eq!(result.format(), "68 F");
}

#[test]
fn parser_invariants_and_malformed_inputs_do_not_panic() {
    for a in 0..12 {
        for b in 0..12 {
            for c in 0..12 {
                let left = evaluate(&format!("({a}+{b})+{c}")).unwrap();
                let right = evaluate(&format!("{a}+({b}+{c})")).unwrap();
                assert_eq!(left, right);

                let left = evaluate(&format!("({a}*{b})*{c}")).unwrap();
                let right = evaluate(&format!("{a}*({b}*{c})")).unwrap();
                assert_eq!(left, right);
            }
        }
    }

    let alphabet = b"0123456789+-*/().^%eE ";
    let mut state: u64 = 0x5eed_1234_abcd_9876;
    for _ in 0..2_000 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let length = (state % 40) as usize;
        let mut expression = String::with_capacity(length);
        for _ in 0..length {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            expression.push(alphabet[(state % alphabet.len() as u64) as usize] as char);
        }
        match evaluate(&expression) {
            Ok(value) => assert!(value.is_finite(), "{expression:?}"),
            Err(error) => assert!(!error.to_string().is_empty(), "{expression:?}"),
        }
        let _ = route_query(&expression);
    }
}
