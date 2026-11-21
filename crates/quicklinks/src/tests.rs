//! Unit tests for the quicklinks model.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::encoding::encode_literal_segment;

use super::*;

fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

fn probe() -> StaticOpenerProbe {
    StaticOpenerProbe::all_available()
}

fn literal_template(url: &str) -> Template {
    Template::parse(url, Vec::new()).unwrap()
}

fn query_template() -> Template {
    Template::parse(
        "https://example.test/search?q={query}",
        vec![ArgSpec::new(
            "query",
            ArgKind::Text,
            Encoding::UrlQueryValue,
        )],
    )
    .unwrap()
}

fn query_link() -> Quicklink {
    Quicklink::new(
        QuicklinkId::from_slug("search").unwrap(),
        "Search",
        query_template(),
    )
}

fn path_template() -> Template {
    Template::parse(
        "C:\\temp\\{name}",
        vec![ArgSpec::new(
            "name",
            ArgKind::Text,
            Encoding::LiteralPathSegment,
        )],
    )
    .unwrap()
}

fn insert_link(library: &mut Library, link: &Quicklink, supplied: &BTreeMap<String, String>) {
    let preview = library
        .preview(link, supplied, OpenPolicy::web_only(), &probe())
        .unwrap();
    library.commit(&preview).unwrap();
}

fn sample_library() -> Library {
    let mut library = Library::new();
    let docs = Quicklink::new(
        QuicklinkId::from_slug("docs").unwrap(),
        "Docs",
        literal_template("https://example.test/docs"),
    )
    .with_alias("docs")
    .with_tags(["z", "a"]);
    insert_link(&mut library, &docs, &BTreeMap::new());

    let repo_template = Template::parse(
        "https://github.com/{owner}",
        vec![ArgSpec::new(
            "owner",
            ArgKind::Text,
            Encoding::UrlPathSegment,
        )],
    )
    .unwrap();
    let repo = Quicklink::new(
        QuicklinkId::from_slug("repo").unwrap(),
        "Repo",
        repo_template,
    )
    .with_tags(["dev"]);
    insert_link(&mut library, &repo, &values(&[("owner", "ansa")]));
    library
}

fn decode(encoded: &str) -> String {
    fn nibble(byte: u8) -> u8 {
        match byte {
            b'0'..=b'9' => byte - b'0',
            b'A'..=b'F' => byte - b'A' + 10,
            _ => panic!("unexpected percent-encoding digit"),
        }
    }
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            decoded.push(nibble(bytes[index + 1]) * 16 + nibble(bytes[index + 2]));
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).unwrap()
}

fn next_random(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state
}

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

#[test]
fn identity_is_stable_and_ignores_display_text() {
    let first = QuicklinkId::from_slug("docs").unwrap();
    let second = QuicklinkId::from_creation_id("docs").unwrap();
    assert_eq!(first, second);

    let template = literal_template("https://example.test/docs");
    let original = Quicklink::new(first.clone(), "Docs", template.clone());
    let renamed = Quicklink::new(first.clone(), "Completely different title", template);
    assert_eq!(original.id(), renamed.id());
    assert_eq!(first.local_hex().len(), 32);
    assert!(first.as_str().starts_with(ID_PREFIX));
}

#[test]
fn identity_has_a_fixed_known_vector() {
    assert_eq!(
        QuicklinkId::from_slug("docs").unwrap().as_str(),
        "quicklink:695b881d7c757277b806e97060b33d8a"
    );
    assert_eq!(
        QuicklinkId::from_slug("my-link").unwrap().as_str(),
        "quicklink:4cf2d32bd74ff78d76a7f91d79beecc2"
    );
}

#[test]
fn identity_rejects_invalid_slugs() {
    assert_eq!(QuicklinkId::from_slug(""), Err(IdError::EmptySlug));
    assert!(matches!(
        QuicklinkId::from_slug("has space"),
        Err(IdError::InvalidSlugCharacter { .. })
    ));
    assert!(matches!(
        QuicklinkId::from_slug("café"),
        Err(IdError::InvalidSlugCharacter { .. })
    ));
    assert!(matches!(
        QuicklinkId::from_slug(&"a".repeat(MAX_SLUG_CHARS + 1)),
        Err(IdError::SlugTooLong { .. })
    ));
}

#[test]
fn identity_parsing_is_canonical() {
    let id = QuicklinkId::from_slug("docs").unwrap();
    assert_eq!(QuicklinkId::parse(id.as_str()).unwrap(), id);
    assert_eq!(id.as_str().parse::<QuicklinkId>().unwrap(), id);
    assert_eq!(
        QuicklinkId::parse("quicklink:abc"),
        Err(IdError::BadHexLength { actual: 3 })
    );
    assert_eq!(QuicklinkId::parse("docs"), Err(IdError::MissingPrefix));
    assert!(matches!(
        QuicklinkId::parse("quicklink:695B881D7C757277B806E97060B33D8A"),
        Err(IdError::InvalidHexCharacter { .. })
    ));
}

// ---------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------

#[test]
fn unreserved_bytes_pass_through() {
    let unreserved = "AZaz09-._~";
    assert_eq!(encode_component(unreserved), unreserved);
}

#[test]
fn reserved_bytes_are_percent_encoded_without_plus() {
    assert_eq!(encode_component("a b+c/d?e#f"), "a%20b%2Bc%2Fd%3Fe%23f");
    assert_eq!(encode_component("100%"), "100%25");
    assert_eq!(encode_component("\"quoted\""), "%22quoted%22");
    assert!(!encode_component("a + b").contains('+'));
}

#[test]
fn unicode_space_quote_and_percent_round_trip() {
    let corpus = [
        "café",
        "漢字",
        "🙂 emoji",
        "100% ready",
        "two words",
        "\"double\" and 'single'",
        "a/b?c#d",
        "{other}",
        "$(rm -rf /); echo pwned",
    ];
    for value in corpus {
        let encoded = encode_component(value);
        assert_eq!(decode(&encoded), value);
        assert!(!encoded.contains('+'), "space must never become '+'");
    }
}

#[test]
fn percent_encoding_property_round_trips_and_only_emits_unreserved_bytes() {
    let alphabet: Vec<char> = "aZ09 -._~%\"'+/?#[]@!$&()*,;=:{}é漢🙂\t".chars().collect();
    let mut state = 0x1234_5678_9abc_def0_u64;
    for length in 0..40 {
        let mut input = String::new();
        for _ in 0..length {
            let index = (next_random(&mut state) as usize) % alphabet.len();
            input.push(alphabet[index]);
        }
        let encoded = encode_component(&input);
        assert!(!encoded.contains('+'));
        let bytes = encoded.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%' {
                assert!(index + 2 < bytes.len());
                for digit in &bytes[index + 1..=index + 2] {
                    assert!(
                        digit.is_ascii_digit() || (b'A'..=b'F').contains(digit),
                        "percent escapes must use uppercase hex"
                    );
                }
                index += 3;
            } else {
                assert!(is_unreserved(bytes[index]));
                index += 1;
            }
        }
        assert_eq!(decode(&encoded), input);
    }
}

#[test]
fn literal_path_segments_preserve_data_but_reject_structure() {
    let value = "résumé 100% \"quoted\" 'single' {other} $HOME";
    assert_eq!(encode_literal_segment(value).unwrap(), value);
    assert!(matches!(
        encode_literal_segment("a/b"),
        Err(EncodeError::PathSeparator { .. })
    ));
    assert!(matches!(
        encode_literal_segment("a\\b"),
        Err(EncodeError::PathSeparator { .. })
    ));
    assert_eq!(
        encode_literal_segment(".."),
        Err(EncodeError::ParentTraversal)
    );
    assert!(matches!(
        encode_literal_segment("nul\0byte"),
        Err(EncodeError::ControlCharacter { .. })
    ));
    assert!(matches!(
        encode_literal_segment("line\nbreak"),
        Err(EncodeError::ControlCharacter { .. })
    ));
}

// ---------------------------------------------------------------------------
// Template grammar
// ---------------------------------------------------------------------------

#[test]
fn template_parses_literals_placeholders_and_escaped_braces() {
    let template = Template::parse(
        "https://example.test/{{literal}}/{query:url_query_value}?n={count}",
        vec![
            ArgSpec::new("query", ArgKind::Text, Encoding::UrlQueryValue),
            ArgSpec::new("count", ArgKind::Number, Encoding::UrlQueryValue),
        ],
    )
    .unwrap();
    assert_eq!(template.parts().len(), 4);
    assert_eq!(
        template.source(),
        "https://example.test/{{literal}}/{query:url_query_value}?n={count:url_query_value}"
    );
    assert_eq!(
        template
            .resolve_text(&values(&[("query", "a b"), ("count", "2")]))
            .unwrap(),
        "https://example.test/{literal}/a%20b?n=2"
    );

    let reparsed = Template::parse(&template.source(), template.arguments().to_vec()).unwrap();
    assert_eq!(reparsed, template);
}

#[test]
fn template_rejects_unknown_placeholders() {
    assert_eq!(
        Template::parse("{missing}", Vec::new()),
        Err(TemplateError::UnknownPlaceholder {
            name: "missing".to_owned(),
            position: 0,
        })
    );
}

#[test]
fn template_rejects_duplicate_argument_declarations() {
    let argument = ArgSpec::new("x", ArgKind::Text, Encoding::UrlPathSegment);
    assert_eq!(
        Template::parse("{x}", vec![argument.clone(), argument]),
        Err(TemplateError::DuplicateArgument {
            name: "x".to_owned(),
        })
    );
}

#[test]
fn template_rejects_duplicate_placeholders_and_unused_arguments() {
    let argument = ArgSpec::new("x", ArgKind::Text, Encoding::UrlPathSegment);
    assert!(matches!(
        Template::parse("{x}-{x}", vec![argument.clone()]),
        Err(TemplateError::DuplicatePlaceholder { .. })
    ));
    assert_eq!(
        Template::parse(
            "{x}",
            vec![
                argument,
                ArgSpec::new("y", ArgKind::Text, Encoding::UrlPathSegment),
            ],
        ),
        Err(TemplateError::UnusedArgument {
            name: "y".to_owned(),
        })
    );
}

#[test]
fn template_rejects_malformed_braces() {
    assert!(matches!(
        Template::parse("{x", Vec::new()),
        Err(TemplateError::UnclosedPlaceholder { .. })
    ));
    assert!(matches!(
        Template::parse("x}", Vec::new()),
        Err(TemplateError::UnexpectedClosingBrace { .. })
    ));
    assert!(matches!(
        Template::parse("{}", Vec::new()),
        Err(TemplateError::EmptyPlaceholder { .. })
    ));
    let argument = ArgSpec::new("x", ArgKind::Text, Encoding::UrlPathSegment);
    assert!(matches!(
        Template::parse("{x{y}}", vec![argument]),
        Err(TemplateError::NestedPlaceholder { .. })
    ));
}

#[test]
fn template_rejects_bad_names_and_modes() {
    assert!(matches!(
        Template::parse("{1x}", Vec::new()),
        Err(TemplateError::InvalidName { .. })
    ));
    let argument = ArgSpec::new("x", ArgKind::Text, Encoding::UrlPathSegment);
    assert!(matches!(
        Template::parse("{x:not-a-mode}", vec![argument.clone()]),
        Err(TemplateError::UnknownMode { .. })
    ));
    assert!(matches!(
        Template::parse("{x:}", vec![argument]),
        Err(TemplateError::UnknownMode { .. })
    ));
    assert!(matches!(
        Template::parse(
            "{x:x:y}",
            vec![ArgSpec::new("x", ArgKind::Text, Encoding::UrlPathSegment)]
        ),
        Err(TemplateError::InvalidName { .. })
    ));
}

#[test]
fn template_rejects_bad_defaults() {
    let encoding = Encoding::UrlPathSegment;
    assert!(matches!(
        Template::parse(
            "{x}",
            vec![ArgSpec::new("x", ArgKind::Text, encoding).with_default(ArgumentValue::text("v"))]
        ),
        Err(TemplateError::RequiredWithDefault { .. })
    ));

    let mut optional_without_default = ArgSpec::new("x", ArgKind::Text, encoding);
    optional_without_default.required = false;
    assert!(matches!(
        Template::parse("{x}", vec![optional_without_default]),
        Err(TemplateError::OptionalWithoutDefault { .. })
    ));

    assert!(matches!(
        Template::parse(
            "{x}",
            vec![
                ArgSpec::new("x", ArgKind::Text, encoding)
                    .optional(ArgumentValue::number("1").unwrap())
            ]
        ),
        Err(TemplateError::DefaultKindMismatch { .. })
    ));
    assert!(matches!(
        Template::parse(
            "{x}",
            vec![
                ArgSpec::new("x", ArgKind::Number, encoding)
                    .optional(ArgumentValue::Number("not-a-number".to_owned()))
            ]
        ),
        Err(TemplateError::InvalidDefaultNumber { .. })
    ));
    assert!(matches!(
        Template::parse(
            "{x}",
            vec![
                ArgSpec::new("x", ArgKind::Number, encoding)
                    .optional(ArgumentValue::Number(" 1 ".to_owned()))
            ]
        ),
        Err(TemplateError::InvalidDefaultNumber { .. })
    ));
    assert!(matches!(
        Template::parse("{x}", vec![ArgSpec::new("x", ArgKind::Choice, encoding)]),
        Err(TemplateError::MissingChoices { .. })
    ));

    let mut unexpected_choices = ArgSpec::new("x", ArgKind::Text, encoding);
    unexpected_choices.choices = vec!["a".to_owned()];
    assert!(matches!(
        Template::parse("{x}", vec![unexpected_choices]),
        Err(TemplateError::UnexpectedChoices { .. })
    ));

    assert!(matches!(
        Template::parse(
            "{x}",
            vec![ArgSpec::choice("x", encoding, ["a", "b"]).optional(ArgumentValue::choice("z"))]
        ),
        Err(TemplateError::DefaultNotInChoices { .. })
    ));
    assert!(matches!(
        Template::parse("{x}", vec![ArgSpec::choice("x", encoding, ["a", "a"])]),
        Err(TemplateError::DuplicateChoice { .. })
    ));
    assert!(matches!(
        Template::parse("{x}", vec![ArgSpec::choice("x", encoding, ["a", ""])]),
        Err(TemplateError::EmptyChoice { .. })
    ));
}

#[test]
fn template_limits_are_enforced() {
    assert!(matches!(
        Template::parse(&"a".repeat(MAX_TEMPLATE_CHARS + 1), Vec::new()),
        Err(TemplateError::TooLong { .. })
    ));
    let argument = ArgSpec::new(
        "a".repeat(MAX_ARGUMENT_NAME_CHARS + 1),
        ArgKind::Text,
        Encoding::UrlPathSegment,
    );
    assert!(matches!(
        Template::parse("", vec![argument]),
        Err(TemplateError::NameTooLong { .. })
    ));

    let long_value = Template::parse(
        "https://example.test/{q}",
        vec![ArgSpec::new("q", ArgKind::Text, Encoding::UrlPathSegment)],
    )
    .unwrap();
    let oversized = "a".repeat(MAX_RESOLVED_TARGET_CHARS);
    assert!(matches!(
        long_value.resolve_text(&values(&[("q", oversized.as_str())])),
        Err(ResolveError::TargetTooLong { .. })
    ));
}

#[test]
fn optional_arguments_use_defaults_and_required_arguments_fail_when_missing() {
    let template = Template::parse(
        "https://example.test/{lang}",
        vec![
            ArgSpec::new("lang", ArgKind::Text, Encoding::UrlPathSegment)
                .optional(ArgumentValue::text("en")),
        ],
    )
    .unwrap();
    assert_eq!(
        template.resolve_text(&BTreeMap::new()).unwrap(),
        "https://example.test/en"
    );

    let required = Template::parse(
        "https://example.test/{lang}",
        vec![ArgSpec::new(
            "lang",
            ArgKind::Text,
            Encoding::UrlPathSegment,
        )],
    )
    .unwrap();
    assert_eq!(
        required.resolve_text(&BTreeMap::new()),
        Err(ResolveError::MissingRequired {
            name: "lang".to_owned(),
        })
    );
}

#[test]
fn text_values_are_parsed_according_to_declared_types() {
    let template = Template::parse(
        "https://example.test/{n}/{flag}/{choice}",
        vec![
            ArgSpec::new("n", ArgKind::Number, Encoding::UrlPathSegment),
            ArgSpec::new("flag", ArgKind::Boolean, Encoding::UrlPathSegment),
            ArgSpec::choice("choice", Encoding::UrlPathSegment, ["a", "b"]),
        ],
    )
    .unwrap();

    assert_eq!(
        template
            .resolve_text(&values(&[
                ("n", " 1.50 "),
                ("flag", "TRUE"),
                ("choice", "b")
            ]))
            .unwrap(),
        "https://example.test/1.50/true/b"
    );
    assert!(matches!(
        template.resolve_text(&values(&[("n", "abc"), ("flag", "true"), ("choice", "a")])),
        Err(ResolveError::InvalidNumber { .. })
    ));
    assert!(matches!(
        template.resolve_text(&values(&[("n", "1"), ("flag", "yes"), ("choice", "a")])),
        Err(ResolveError::InvalidBoolean { .. })
    ));
    assert!(matches!(
        template.resolve_text(&values(&[("n", "1"), ("flag", "true"), ("choice", "c")])),
        Err(ResolveError::NotAChoice { .. })
    ));
}

#[test]
fn typed_resolution_rejects_kind_mismatches() {
    let template = Template::parse(
        "https://example.test/{q}",
        vec![ArgSpec::new("q", ArgKind::Text, Encoding::UrlQueryValue)],
    )
    .unwrap();
    let mut supplied = BTreeMap::new();
    supplied.insert("q".to_owned(), ArgumentValue::Boolean(true));
    assert!(matches!(
        template.resolve(&supplied),
        Err(ResolveError::KindMismatch { .. })
    ));
}

#[test]
fn unknown_argument_values_are_rejected() {
    let template = query_template();
    assert_eq!(
        template.resolve_text(&values(&[("q", "x"), ("extra", "y")])),
        Err(ResolveError::UnknownArgument {
            name: "extra".to_owned(),
        })
    );
}

#[test]
fn substitution_is_single_pass_and_never_reexpands_values() {
    let template = Template::parse(
        "/srv/{value}",
        vec![ArgSpec::new(
            "value",
            ArgKind::Text,
            Encoding::LiteralPathSegment,
        )],
    )
    .unwrap();
    assert_eq!(
        template
            .resolve_text(&values(&[("value", "{other}$(id)`whoami`")]))
            .unwrap(),
        "/srv/{other}$(id)`whoami`"
    );

    let url = Template::parse(
        "https://example.test/?v={value}",
        vec![ArgSpec::new(
            "value",
            ArgKind::Text,
            Encoding::UrlQueryValue,
        )],
    )
    .unwrap();
    assert_eq!(
        url.resolve_text(&values(&[("value", "{other}")])).unwrap(),
        "https://example.test/?v=%7Bother%7D"
    );
}

#[test]
fn prototype_pollution_style_names_are_unknown_placeholders() {
    for name in ["__proto__", "constructor", "prototype"] {
        let source = format!("{{{name}}}");
        assert_eq!(
            Template::parse(&source, Vec::new()),
            Err(TemplateError::UnknownPlaceholder {
                name: name.to_owned(),
                position: 0,
            })
        );
    }
}

#[test]
fn shell_metacharacters_are_literal_data() {
    let template = Template::parse(
        "https://example.test/{value}",
        vec![ArgSpec::new(
            "value",
            ArgKind::Text,
            Encoding::UrlPathSegment,
        )],
    )
    .unwrap();
    let resolved = template
        .resolve_text(&values(&[("value", "$(rm -rf /); echo pwned | cat && id")]))
        .unwrap();
    assert_eq!(
        resolved,
        "https://example.test/%24%28rm%20-rf%20%2F%29%3B%20echo%20pwned%20%7C%20cat%20%26%26%20id"
    );

    let path = Template::parse(
        "C:\\temp\\{name}",
        vec![ArgSpec::new(
            "name",
            ArgKind::Text,
            Encoding::LiteralPathSegment,
        )],
    )
    .unwrap();
    let value = "$(rm -rf); echo `pwned` \"quoted\" 100% café";
    assert_eq!(
        path.resolve_text(&values(&[("name", value)])).unwrap(),
        format!("C:\\temp\\{value}")
    );
}

// ---------------------------------------------------------------------------
// Scheme and opener policy
// ---------------------------------------------------------------------------

#[test]
fn http_and_https_are_accepted() {
    assert_eq!(
        resolve_target(
            "https://example.test/a?b=c#d",
            Opener::Browser,
            OpenPolicy::web_only()
        ),
        Ok(TargetKind::WebUrl)
    );
    assert_eq!(
        resolve_target(
            "HTTP://example.test",
            Opener::Browser,
            OpenPolicy::web_only()
        ),
        Ok(TargetKind::WebUrl)
    );
    assert_eq!(
        resolve_target(
            "https://example.test/a%20b",
            Opener::Browser,
            OpenPolicy::web_only()
        ),
        Ok(TargetKind::WebUrl)
    );
}

#[test]
fn blocked_schemes_are_rejected() {
    for target in [
        "javascript:alert(1)",
        "JaVaScRiPt:alert(1)",
        "data:text/html,hi",
        "file:///C:/Windows",
        "file:C:/Windows",
        "vbscript:msgbox(1)",
        "shell:startup",
    ] {
        assert!(
            matches!(
                resolve_target(target, Opener::Browser, OpenPolicy::with_local_paths()),
                Err(TargetError::BlockedScheme { .. })
            ),
            "expected {target} to be blocked even with local paths enabled"
        );
    }
}

#[test]
fn web_access_can_be_disabled_independently() {
    let policy = OpenPolicy {
        allow_web: false,
        ..OpenPolicy::with_local_paths()
    };
    assert_eq!(
        resolve_target("https://example.test", Opener::Browser, policy),
        Err(TargetError::WebDisallowed)
    );
    assert_eq!(
        resolve_target("C:\\data", Opener::FileManager, policy),
        Ok(TargetKind::LocalPath)
    );
}

#[test]
fn unapproved_schemes_are_rejected() {
    for target in [
        "mailto:a@example.test",
        "ssh://host",
        "chrome-extension://abc",
    ] {
        assert!(matches!(
            resolve_target(target, Opener::DefaultApp, OpenPolicy::web_only()),
            Err(TargetError::UnapprovedScheme { .. })
        ));
    }
}

#[test]
fn local_paths_require_explicit_policy_and_a_compatible_opener() {
    let path = "C:\\Users\\me\\file.txt";
    assert!(matches!(
        resolve_target(path, Opener::Browser, OpenPolicy::web_only()),
        Err(TargetError::OpenerMismatch {
            kind: TargetKind::LocalPath,
            ..
        })
    ));
    assert_eq!(
        resolve_target(path, Opener::FileManager, OpenPolicy::web_only()),
        Err(TargetError::LocalPathsDisallowed)
    );
    assert_eq!(
        resolve_target(path, Opener::FileManager, OpenPolicy::with_local_paths()),
        Ok(TargetKind::LocalPath)
    );
    assert_eq!(
        resolve_target(
            "\\\\server\\share\\folder",
            Opener::DefaultApp,
            OpenPolicy::with_local_paths()
        ),
        Err(TargetError::NetworkPathsDisallowed)
    );
    assert_eq!(
        resolve_target(
            "\\\\server\\share\\folder",
            Opener::DefaultApp,
            OpenPolicy::with_local_paths().with_network_paths()
        ),
        Ok(TargetKind::LocalPath)
    );
    assert_eq!(
        resolve_target(
            "/srv/data",
            Opener::FileManager,
            OpenPolicy::with_local_paths()
        ),
        Ok(TargetKind::LocalPath)
    );
}

#[test]
fn relative_targets_are_rejected() {
    assert_eq!(
        resolve_target("folder/file", Opener::Browser, OpenPolicy::web_only()),
        Err(TargetError::RelativeTarget)
    );
    assert_eq!(
        resolve_target(
            "folder/file",
            Opener::FileManager,
            OpenPolicy::with_local_paths()
        ),
        Err(TargetError::NotAbsolutePath)
    );
    assert_eq!(
        resolve_target(
            "C:folder",
            Opener::FileManager,
            OpenPolicy::with_local_paths()
        ),
        Err(TargetError::NotAbsolutePath)
    );
}

#[test]
fn path_traversal_control_characters_and_whitespace_are_rejected() {
    assert!(matches!(
        resolve_target(
            "C:\\Users\\..\\Windows",
            Opener::FileManager,
            OpenPolicy::with_local_paths()
        ),
        Err(TargetError::PathTraversal { .. })
    ));
    assert!(matches!(
        resolve_target(
            "/srv/a/../b",
            Opener::FileManager,
            OpenPolicy::with_local_paths()
        ),
        Err(TargetError::PathTraversal { .. })
    ));
    assert!(matches!(
        resolve_target(
            "C:\\a\0b",
            Opener::FileManager,
            OpenPolicy::with_local_paths()
        ),
        Err(TargetError::ControlCharacter { .. })
    ));
    assert_eq!(
        resolve_target(
            " https://example.test",
            Opener::Browser,
            OpenPolicy::web_only()
        ),
        Err(TargetError::SurroundingWhitespace)
    );
    assert_eq!(
        resolve_target("", Opener::Browser, OpenPolicy::web_only()),
        Err(TargetError::Empty)
    );
}

#[test]
fn openers_reject_incompatible_target_kinds() {
    assert!(matches!(
        resolve_target(
            "https://example.test",
            Opener::FileManager,
            OpenPolicy::web_only()
        ),
        Err(TargetError::OpenerMismatch {
            kind: TargetKind::WebUrl,
            ..
        })
    ));
    assert_eq!(
        resolve_target(
            "/srv/data",
            Opener::DefaultApp,
            OpenPolicy::with_local_paths()
        ),
        Ok(TargetKind::LocalPath)
    );
}

#[test]
fn malformed_urls_are_rejected() {
    assert!(matches!(
        resolve_target("https://", Opener::Browser, OpenPolicy::web_only()),
        Err(TargetError::InvalidUrl { .. })
    ));
    assert!(matches!(
        resolve_target("https:///path", Opener::Browser, OpenPolicy::web_only()),
        Err(TargetError::InvalidUrl { .. })
    ));
    assert!(matches!(
        resolve_target(
            "https://example.test/a b",
            Opener::Browser,
            OpenPolicy::web_only()
        ),
        Err(TargetError::InvalidUrlCharacter { character: ' ', .. })
    ));
    assert_eq!(
        resolve_target(
            "https://example.test/café",
            Opener::Browser,
            OpenPolicy::web_only()
        ),
        Ok(TargetKind::WebUrl)
    );
    assert!(matches!(
        resolve_target(
            "https://example.test/%zz",
            Opener::Browser,
            OpenPolicy::web_only()
        ),
        Err(TargetError::InvalidPercentEscape { .. })
    ));
}

#[test]
fn scheme_obfuscation_is_rejected() {
    for target in ["%6aavascript:alert(1)", " javascript:alert(1)"] {
        assert!(
            resolve_target(target, Opener::Browser, OpenPolicy::web_only()).is_err(),
            "expected {target} to be rejected"
        );
    }
    assert!(matches!(
        resolve_target(
            "java\tscript:alert(1)",
            Opener::Browser,
            OpenPolicy::web_only()
        ),
        Err(TargetError::ControlCharacter { .. })
    ));
    assert!(matches!(
        resolve_target(
            "HTTP://example.test",
            Opener::Browser,
            OpenPolicy::web_only()
        ),
        Ok(TargetKind::WebUrl)
    ));
}

#[test]
fn absolute_path_classification_is_explicit() {
    assert!(is_absolute_local_path("C:\\x"));
    assert!(is_absolute_local_path("C:/x"));
    assert!(is_absolute_local_path("\\\\server\\share"));
    assert!(is_absolute_local_path("//server/share"));
    assert!(is_absolute_local_path("/srv/x"));
    assert!(!is_absolute_local_path("C:x"));
    assert!(!is_absolute_local_path("folder/x"));
    assert_eq!(scheme_of("HTTPS://example.test"), Some("https".to_owned()));
    assert_eq!(scheme_of("C:\\x"), Some("c".to_owned()));
    assert_eq!(scheme_of("folder/x"), None);
}

// ---------------------------------------------------------------------------
// Preview and commit
// ---------------------------------------------------------------------------

#[test]
fn preview_resolves_the_exact_target_and_effect() {
    let library = Library::new();
    let link = query_link();
    let preview = library
        .preview(
            &link,
            &values(&[("query", "rust lang")]),
            OpenPolicy::web_only(),
            &probe(),
        )
        .unwrap();
    assert_eq!(
        preview.target(),
        "https://example.test/search?q=rust%20lang"
    );
    assert_eq!(preview.opener(), Opener::Browser);
    assert_eq!(preview.target_kind(), TargetKind::WebUrl);
    assert_eq!(preview.revision(), 0);
    assert_eq!(preview.request().target, preview.target());
    assert!(preview.effect().contains(preview.target()));
    assert_eq!(preview.fingerprint().as_str().len(), 32);
    assert!(preview.verify_fingerprint());

    let repeated = library
        .preview(
            &link,
            &values(&[("query", "rust lang")]),
            OpenPolicy::web_only(),
            &probe(),
        )
        .unwrap();
    assert_eq!(preview.fingerprint(), repeated.fingerprint());
}

#[test]
fn preview_fails_with_an_actionable_error_when_the_opener_is_unavailable() {
    let library = Library::new();
    let unavailable = StaticOpenerProbe::all_available()
        .with_unavailable(Opener::Browser, "no default browser is registered");
    let error = library
        .preview(
            &query_link(),
            &values(&[("query", "x")]),
            OpenPolicy::web_only(),
            &unavailable,
        )
        .unwrap_err();
    assert!(matches!(
        error,
        PreviewError::OpenerUnavailable {
            opener: Opener::Browser,
            ..
        }
    ));
    assert!(error.to_string().contains("register a handler"));
    assert!(
        !OpenerAvailability::Unavailable {
            reason: "x".to_owned()
        }
        .is_available()
    );
}

#[test]
fn stale_previews_are_rejected() {
    let mut library = Library::new();
    let link = query_link();
    let preview = library
        .preview(
            &link,
            &values(&[("query", "x")]),
            OpenPolicy::web_only(),
            &probe(),
        )
        .unwrap();
    library.commit(&preview).unwrap();
    assert_eq!(library.revision(), 1);
    assert_eq!(
        library.commit(&preview),
        Err(CommitError::Conflict {
            preview: 0,
            current: 1,
        })
    );
}

#[test]
fn fingerprint_mismatch_is_rejected_at_commit() {
    let mut library = Library::new();
    let link = query_link();
    let mut preview = library
        .preview(
            &link,
            &values(&[("query", "x")]),
            OpenPolicy::web_only(),
            &probe(),
        )
        .unwrap();
    preview.corrupt_fingerprint_for_test();
    assert!(!preview.verify_fingerprint());
    assert_eq!(
        library.commit(&preview),
        Err(CommitError::FingerprintMismatch)
    );
    assert!(library.is_empty());
}

#[test]
fn unrelated_mutations_make_an_old_preview_stale() {
    let mut library = Library::new();
    let first = query_link();
    let first_preview = library
        .preview(
            &first,
            &values(&[("query", "x")]),
            OpenPolicy::web_only(),
            &probe(),
        )
        .unwrap();
    library.commit(&first_preview).unwrap();

    let second = Quicklink::new(
        QuicklinkId::from_slug("second").unwrap(),
        "Second",
        literal_template("https://example.test/second"),
    );
    insert_link(&mut library, &second, &BTreeMap::new());
    assert!(matches!(
        library.commit(&first_preview),
        Err(CommitError::Conflict { .. })
    ));
}

#[test]
fn commit_saves_exactly_the_previewed_candidate() {
    let mut library = Library::new();
    let link = query_link();
    let preview = library
        .preview(
            &link,
            &values(&[("query", "x")]),
            OpenPolicy::web_only(),
            &probe(),
        )
        .unwrap();

    let replacement = Quicklink::new(
        link.id().clone(),
        "Different title built later",
        literal_template("https://example.test/different"),
    );
    assert_ne!(&replacement, preview.candidate());

    let saved = library.commit(&preview).unwrap();
    assert_eq!(saved.title(), "Search");
    assert_eq!(saved.template(), &query_template());
}

#[test]
fn aliases_are_unique_case_insensitively_and_reusable_after_removal() {
    let mut library = Library::new();
    let first = Quicklink::new(
        QuicklinkId::from_slug("alias-one").unwrap(),
        "One",
        literal_template("https://example.test/one"),
    )
    .with_alias("Docs");
    insert_link(&mut library, &first, &BTreeMap::new());

    let second = Quicklink::new(
        QuicklinkId::from_slug("alias-two").unwrap(),
        "Two",
        literal_template("https://example.test/two"),
    )
    .with_alias(" docs ");
    let error = library
        .preview(&second, &BTreeMap::new(), OpenPolicy::web_only(), &probe())
        .unwrap_err();
    assert!(matches!(error, PreviewError::AliasTaken { .. }));

    library.remove(first.id()).unwrap();
    library
        .preview(&second, &BTreeMap::new(), OpenPolicy::web_only(), &probe())
        .unwrap();
}

#[test]
fn commit_rechecks_alias_ownership() {
    let mut library = Library::new();
    let owner = Quicklink::new(
        QuicklinkId::from_slug("owner").unwrap(),
        "Owner",
        literal_template("https://example.test/owner"),
    )
    .with_alias("shared");
    insert_link(&mut library, &owner, &BTreeMap::new());

    let candidate = Quicklink::new(
        QuicklinkId::from_slug("candidate").unwrap(),
        "Candidate",
        literal_template("https://example.test/candidate"),
    )
    .with_alias("shared");
    assert!(matches!(
        library.preview(
            &candidate,
            &BTreeMap::new(),
            OpenPolicy::web_only(),
            &probe()
        ),
        Err(PreviewError::AliasTaken { .. })
    ));
}

#[test]
fn disabled_quicklinks_can_be_previewed_and_say_so() {
    let library = Library::new();
    let link = query_link().with_enabled(false);
    let preview = library
        .preview(
            &link,
            &values(&[("query", "x")]),
            OpenPolicy::web_only(),
            &probe(),
        )
        .unwrap();
    assert!(!preview.candidate().enabled());
    assert!(preview.effect().starts_with("Disabled."));
    assert!(preview.effect().contains(preview.target()));
}

#[test]
fn preview_rejects_invalid_titles_and_unsafe_literal_schemes() {
    let library = Library::new();
    let empty_title = Quicklink::new(
        QuicklinkId::from_slug("empty-title").unwrap(),
        "   ",
        literal_template("https://example.test/x"),
    );
    assert!(matches!(
        library.preview(
            &empty_title,
            &BTreeMap::new(),
            OpenPolicy::web_only(),
            &probe()
        ),
        Err(PreviewError::Model(ModelError::EmptyTitle))
    ));

    let unsafe_link = Quicklink::new(
        QuicklinkId::from_slug("unsafe").unwrap(),
        "Unsafe",
        literal_template("javascript:alert(1)"),
    );
    assert!(matches!(
        library.preview(
            &unsafe_link,
            &BTreeMap::new(),
            OpenPolicy::web_only(),
            &probe()
        ),
        Err(PreviewError::Model(ModelError::UnsafeLiteralScheme { .. }))
    ));
}

#[test]
fn local_path_previews_require_policy_and_report_the_file_manager() {
    let library = Library::new();
    let link = Quicklink::new(
        QuicklinkId::from_slug("report").unwrap(),
        "Report",
        path_template(),
    )
    .with_opener(Opener::FileManager);
    let supplied = values(&[("name", "résumé 100%.txt")]);

    assert!(matches!(
        library.preview(&link, &supplied, OpenPolicy::web_only(), &probe()),
        Err(PreviewError::Target(TargetError::LocalPathsDisallowed))
    ));

    let preview = library
        .preview(&link, &supplied, OpenPolicy::with_local_paths(), &probe())
        .unwrap();
    assert_eq!(preview.target(), "C:\\temp\\résumé 100%.txt");
    assert_eq!(preview.target_kind(), TargetKind::LocalPath);
    assert!(preview.effect().contains("file manager"));
    assert_eq!(preview.request().target, "C:\\temp\\résumé 100%.txt");
}

#[test]
fn insert_update_and_remove_have_explicit_semantics() {
    let mut library = Library::new();
    let link = query_link();
    let supplied = values(&[("query", "a")]);

    let preview = library
        .preview(&link, &supplied, OpenPolicy::web_only(), &probe())
        .unwrap();
    library.insert(&preview).unwrap();
    let second = library
        .preview(&link, &supplied, OpenPolicy::web_only(), &probe())
        .unwrap();
    assert!(matches!(
        library.insert(&second),
        Err(CommitError::DuplicateId { .. })
    ));
    assert_eq!(library.update(&second).unwrap().title(), "Search");

    let unknown = Quicklink::new(
        QuicklinkId::from_slug("does-not-exist").unwrap(),
        "Missing",
        literal_template("https://example.test/missing"),
    );
    let unknown_preview = library
        .preview(&unknown, &BTreeMap::new(), OpenPolicy::web_only(), &probe())
        .unwrap();
    assert!(matches!(
        library.update(&unknown_preview),
        Err(CommitError::NotFound { .. })
    ));
    assert!(matches!(
        library.remove(&QuicklinkId::from_slug("does-not-exist").unwrap()),
        Err(LibraryError::NotFound { .. })
    ));

    let removed = library.remove(link.id()).unwrap();
    assert_eq!(removed.title(), "Search");
    assert!(library.is_empty());
}

#[test]
fn list_is_in_stable_identity_order() {
    let library = sample_library();
    let ids: Vec<&QuicklinkId> = library.list().iter().map(|link| link.id()).collect();
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(library.list_page(0, 1).len(), 1);
    assert_eq!(library.list_page(1, 10).len(), 1);
    assert!(library.list_page(usize::MAX, 10).is_empty());
}

// ---------------------------------------------------------------------------
// Import and export
// ---------------------------------------------------------------------------

#[test]
fn export_is_versioned_and_deterministic() {
    let library = sample_library();
    let first = library.export_json().unwrap();
    let second = library.export_json().unwrap();
    assert_eq!(first, second);

    let document: Value = serde_json::from_str(&first).unwrap();
    assert_eq!(document["schema_version"], json!(SCHEMA_VERSION));
    assert_eq!(SCHEMA_VERSION, 1);
    assert_eq!(document["quicklinks"].as_array().unwrap().len(), 2);
}

#[test]
fn schema_v1_imports_are_accepted_and_revalidated() {
    let safe = query_link();
    let mut library = Library::new();
    let safe_document = json!({
        "schema_version": MIN_SUPPORTED_SCHEMA_VERSION,
        "quicklinks": [serde_json::to_value(&safe).unwrap()],
    });
    let report = library.import_json(
        &safe_document.to_string(),
        ImportOptions::replace_confirmed(),
    );
    assert_eq!(report.applied, 1);
    assert!(!report.has_errors());

    let unsafe_link = placeholder_link("v1-unsafe", "javascript:alert(1)");
    let unsafe_document = json!({
        "schema_version": MIN_SUPPORTED_SCHEMA_VERSION,
        "quicklinks": [serde_json::to_value(&unsafe_link).unwrap()],
    });
    let mut library = Library::new();
    let report = library.import_json(
        &unsafe_document.to_string(),
        ImportOptions::replace_confirmed(),
    );
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.applied, 0);
    assert!(library.is_empty());
}

#[test]
fn export_round_trips_through_replace_import() {
    let library = sample_library();
    let exported = library.export_json().unwrap();
    let mut restored = Library::new();
    let report = restored.import_json(&exported, ImportOptions::replace_confirmed());
    assert_eq!(report.total, 2);
    assert_eq!(report.applied, 2);
    assert_eq!(report.skipped, 0);
    assert!(!report.has_errors());
    assert!(report.is_complete());
    assert!(report.replaced);
    assert_eq!(restored.list(), library.list());
    assert_eq!(restored.export_json().unwrap(), exported);
}

#[test]
fn merge_import_reports_per_item_errors_and_keeps_valid_items() {
    let mut library = sample_library();
    let new_link = Quicklink::new(
        QuicklinkId::from_slug("new-import").unwrap(),
        "New",
        literal_template("https://example.test/new"),
    );
    let mut broken: Value = serde_json::to_value(&new_link).unwrap();
    broken["id"] = json!("not-a-quicklink-identity");
    let document = json!({
        "schema_version": SCHEMA_VERSION,
        "quicklinks": [serde_json::to_value(&new_link).unwrap(), broken],
    });

    let report = library.import_json(&document.to_string(), ImportOptions::merge());
    assert_eq!(report.total, 2);
    assert_eq!(report.applied, 1);
    assert_eq!(report.skipped, 1);
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.errors[0].index, Some(1));
    assert_eq!(library.len(), 3);
    assert!(library.get(new_link.id()).is_some());
}

#[test]
fn replace_import_is_all_or_nothing() {
    let mut library = sample_library();
    let original = library.list().len();
    let new_link = Quicklink::new(
        QuicklinkId::from_slug("replace-target").unwrap(),
        "New",
        literal_template("https://example.test/new"),
    );
    let mut broken: Value = serde_json::to_value(&new_link).unwrap();
    broken["id"] = json!("not-a-quicklink-identity");
    let document = json!({
        "schema_version": SCHEMA_VERSION,
        "quicklinks": [serde_json::to_value(&new_link).unwrap(), broken],
    });

    let report = library.import_json(&document.to_string(), ImportOptions::replace_confirmed());
    assert_eq!(report.total, 2);
    assert_eq!(report.applied, 0);
    assert_eq!(report.skipped, 2);
    assert!(!report.replaced);
    assert!(!report.would_replace);
    assert!(report.has_errors());
    assert_eq!(library.len(), original);
}

#[test]
fn replace_import_replaces_the_whole_library_when_valid() {
    let mut library = sample_library();
    let new_link = Quicklink::new(
        QuicklinkId::from_slug("only-one").unwrap(),
        "Only One",
        literal_template("https://example.test/only"),
    );
    let document = json!({
        "schema_version": SCHEMA_VERSION,
        "quicklinks": [serde_json::to_value(&new_link).unwrap()],
    });

    let report = library.import_json(&document.to_string(), ImportOptions::replace_confirmed());
    assert!(report.replaced);
    assert!(report.would_replace);
    assert_eq!(report.applied, 1);
    assert_eq!(library.len(), 1);
    assert!(library.get(new_link.id()).is_some());
    assert!(
        library
            .get(&QuicklinkId::from_slug("docs").unwrap())
            .is_none()
    );
}

#[test]
fn dry_run_reports_without_mutating() {
    let new_link = Quicklink::new(
        QuicklinkId::from_slug("dry-run").unwrap(),
        "Dry Run",
        literal_template("https://example.test/dry"),
    );
    let document = json!({
        "schema_version": SCHEMA_VERSION,
        "quicklinks": [serde_json::to_value(&new_link).unwrap()],
    });

    let mut merged = sample_library();
    let merge_report =
        merged.import_json(&document.to_string(), ImportOptions::merge().with_dry_run());
    assert!(merge_report.dry_run);
    assert_eq!(merge_report.applied, 1);
    assert_eq!(merged.len(), 2);
    assert!(merged.get(new_link.id()).is_none());

    let mut replaced = sample_library();
    let replace_report = replaced.import_json(
        &document.to_string(),
        ImportOptions::replace_confirmed().with_dry_run(),
    );
    assert!(replace_report.dry_run);
    assert!(replace_report.would_replace);
    assert!(!replace_report.replaced);
    assert_eq!(replace_report.applied, 1);
    assert_eq!(replaced.len(), 2);
}

#[test]
fn duplicate_identities_and_aliases_inside_a_document_are_reported() {
    let first = Quicklink::new(
        QuicklinkId::from_slug("doc-one").unwrap(),
        "One",
        literal_template("https://example.test/one"),
    );
    let first_value = serde_json::to_value(&first).unwrap();
    let duplicate_id_document = json!({
        "schema_version": SCHEMA_VERSION,
        "quicklinks": [first_value.clone(), first_value],
    });
    let mut library = Library::new();
    let report = library.import_json(
        &duplicate_id_document.to_string(),
        ImportOptions::replace_confirmed(),
    );
    assert_eq!(report.errors.len(), 1);
    assert!(!report.replaced);
    assert!(library.is_empty());

    let second = Quicklink::new(
        QuicklinkId::from_slug("doc-two").unwrap(),
        "Two",
        literal_template("https://example.test/two"),
    )
    .with_alias("shared");
    let aliased = Quicklink::new(
        QuicklinkId::from_slug("doc-three").unwrap(),
        "Three",
        literal_template("https://example.test/three"),
    )
    .with_alias("SHARED");
    let duplicate_alias_document = json!({
        "schema_version": SCHEMA_VERSION,
        "quicklinks": [
            serde_json::to_value(&second).unwrap(),
            serde_json::to_value(&aliased).unwrap(),
        ],
    });
    let mut library = Library::new();
    let report = library.import_json(
        &duplicate_alias_document.to_string(),
        ImportOptions::replace_confirmed(),
    );
    assert_eq!(report.errors.len(), 1);
    assert!(!report.replaced);
    assert!(library.is_empty());
}

#[test]
fn merge_never_overwrites_a_conflicting_record() {
    let mut library = sample_library();
    let docs_id = QuicklinkId::from_slug("docs").unwrap();
    let mut document: Value = serde_json::from_str(&library.export_json().unwrap()).unwrap();
    for item in document["quicklinks"].as_array_mut().unwrap() {
        if item["id"] == json!(docs_id.to_string()) {
            item["title"] = json!("Hijacked");
        }
    }

    let report = library.import_json(&document.to_string(), ImportOptions::merge());
    assert!(report.has_errors());
    assert_eq!(report.applied, 1);
    assert_eq!(library.len(), 2);
    assert_eq!(library.get(&docs_id).unwrap().title(), "Docs");
}

#[test]
fn tags_and_aliases_are_canonicalised_for_deterministic_export() {
    let link = Quicklink::new(
        QuicklinkId::from_slug("canonical").unwrap(),
        "  Canonical  ",
        literal_template("https://example.test/canonical"),
    )
    .with_alias("  Alias  ")
    .with_tags([" z ", "a", "A", ""]);
    assert_eq!(link.title(), "Canonical");
    assert_eq!(link.alias(), Some("Alias"));
    assert_eq!(link.tags(), ["a", "z"]);
    link.validate().unwrap();
}

#[test]
fn malformed_documents_are_reported_without_mutation() {
    let documents = [
        "this is not json",
        "[]",
        r#"{"schema_version":3,"quicklinks":[]}"#,
        r#"{"schema_version":1}"#,
        r#"{"quicklinks":[]}"#,
        r#"{"schema_version":"one","quicklinks":[]}"#,
    ];
    for document in documents {
        let mut library = sample_library();
        let before = library.len();
        let report = library.import_json(document, ImportOptions::merge());
        assert!(report.has_errors(), "expected an error for {document}");
        assert_eq!(report.applied, 0);
        assert_eq!(library.len(), before);
    }
}

#[test]
fn structurally_incomplete_items_fail_individually() {
    let id = QuicklinkId::from_slug("incomplete").unwrap().to_string();
    let document = json!({
        "schema_version": SCHEMA_VERSION,
        "quicklinks": [{"id": id, "title": "Missing template"}],
    });
    let mut library = Library::new();
    let report = library.import_json(&document.to_string(), ImportOptions::merge());
    assert_eq!(report.total, 1);
    assert_eq!(report.applied, 0);
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.errors[0].index, Some(0));
    assert!(library.is_empty());
}

fn document_with(links: &[&Quicklink]) -> String {
    let items: Vec<Value> = links
        .iter()
        .map(|link| serde_json::to_value(link).unwrap())
        .collect();
    json!({
        "schema_version": SCHEMA_VERSION,
        "quicklinks": items,
    })
    .to_string()
}

fn placeholder_link(slug: &str, default: &str) -> Quicklink {
    let template = Template::parse(
        "{target:literal_path_segment}",
        vec![
            ArgSpec::new("target", ArgKind::Text, Encoding::LiteralPathSegment)
                .optional(ArgumentValue::text(default)),
        ],
    )
    .unwrap();
    Quicklink::new(QuicklinkId::from_slug(slug).unwrap(), "Imported", template)
}

// ---------------------------------------------------------------------------
// Q-1: import applies the same target policy as preview
// ---------------------------------------------------------------------------

#[test]
fn import_rejects_unsafe_defaults_with_per_item_errors() {
    let unsafe_links = [
        placeholder_link("bad-javascript", "javascript:alert(1)"),
        placeholder_link("bad-file", "file:C:"),
        placeholder_link("bad-ssh", "ssh:host"),
    ];
    for link in unsafe_links {
        let mut library = Library::new();
        let report =
            library.import_json(&document_with(&[&link]), ImportOptions::replace_confirmed());
        assert_eq!(report.total, 1);
        assert_eq!(report.applied, 0);
        assert_eq!(report.errors.len(), 1, "expected one error for {link:?}");
        assert_eq!(report.errors[0].index, Some(0));
        assert!(!report.replaced);
        assert!(library.is_empty());
    }
}

#[test]
fn import_rejects_browser_local_path_and_policy_mismatches() {
    let template = Template::parse(
        "C:\\temp\\{name:literal_path_segment}",
        vec![
            ArgSpec::new("name", ArgKind::Text, Encoding::LiteralPathSegment)
                .optional(ArgumentValue::text("report.txt")),
        ],
    )
    .unwrap();
    let browser_path = Quicklink::new(
        QuicklinkId::from_slug("browser-local").unwrap(),
        "Browser local",
        template,
    );
    let mut library = Library::new();
    let report = library.import_json(
        &document_with(&[&browser_path]),
        ImportOptions::replace_confirmed(),
    );
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.applied, 0);
    assert!(library.is_empty());

    let file_manager = Quicklink::new(
        QuicklinkId::from_slug("file-manager-local").unwrap(),
        "File manager local",
        path_template(),
    )
    .with_opener(Opener::FileManager);
    let mut web_only = Library::new();
    let report = web_only.import_json_with_policy(
        &document_with(&[&file_manager]),
        ImportOptions::replace_confirmed(),
        OpenPolicy::web_only(),
    );
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.applied, 0);
    assert!(web_only.is_empty());

    let mut local_allowed = Library::new();
    let report = local_allowed.import_json_with_policy(
        &document_with(&[&file_manager]),
        ImportOptions::replace_confirmed(),
        OpenPolicy::with_local_paths(),
    );
    assert_eq!(report.errors.len(), 0);
    assert_eq!(report.applied, 1);
}

#[test]
fn import_accepts_a_safe_required_argument_template() {
    let link = query_link();
    let mut library = Library::new();
    let report = library.import_json(&document_with(&[&link]), ImportOptions::replace_confirmed());
    assert_eq!(report.applied, 1);
    assert!(!report.has_errors());
    assert!(library.get(link.id()).is_some());
}

#[test]
fn merge_import_skips_unsafe_items_and_keeps_valid_ones() {
    let unsafe_link = placeholder_link("merge-unsafe", "javascript:alert(1)");
    let safe_link = query_link();
    let document = document_with(&[&unsafe_link, &safe_link]);
    let mut library = Library::new();
    let report = library.import_json(&document, ImportOptions::merge());
    assert_eq!(report.total, 2);
    assert_eq!(report.applied, 1);
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.errors[0].index, Some(0));
    assert_eq!(library.len(), 1);
    assert!(library.get(unsafe_link.id()).is_none());
    assert!(library.get(safe_link.id()).is_some());
}

// ---------------------------------------------------------------------------
// Q-2: merge never exceeds MAX_QUICKLINKS
// ---------------------------------------------------------------------------

fn minimal_item(index: usize) -> Value {
    let id = QuicklinkId::from_slug(&format!("fill-{index}")).unwrap();
    json!({
        "id": id.to_string(),
        "title": "Fill",
        "alias": null,
        "tags": [],
        "template": {"source": "https://example.test/fill", "arguments": []},
        "opener": "browser",
        "enabled": true,
    })
}

#[test]
fn merge_import_refuses_to_exceed_the_library_cap() {
    let items: Vec<Value> = (0..MAX_QUICKLINKS).map(minimal_item).collect();
    let document = json!({
        "schema_version": SCHEMA_VERSION,
        "quicklinks": items,
    })
    .to_string();
    assert!(document.len() < MAX_IMPORT_BYTES);

    let mut library = Library::new();
    let report = library.import_json(&document, ImportOptions::replace_confirmed());
    assert_eq!(report.applied, MAX_QUICKLINKS);
    assert!(!report.has_errors());
    assert_eq!(library.len(), MAX_QUICKLINKS);

    let extra = json!({
        "schema_version": SCHEMA_VERSION,
        "quicklinks": [minimal_item(MAX_QUICKLINKS)],
    })
    .to_string();
    let report = library.import_json(&extra, ImportOptions::merge());
    assert_eq!(report.applied, 0);
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.errors[0].index, None);
    assert!(report.errors[0].message.contains("maximum"));
    assert_eq!(library.len(), MAX_QUICKLINKS);
}

// ---------------------------------------------------------------------------
// Q-3: network/device paths and executables
// ---------------------------------------------------------------------------

#[test]
fn network_and_device_paths_require_explicit_opt_ins() {
    let local = OpenPolicy::with_local_paths();
    for target in ["\\\\server\\share", "//host/share"] {
        assert_eq!(
            resolve_target(target, Opener::FileManager, local),
            Err(TargetError::NetworkPathsDisallowed)
        );
        assert_eq!(
            resolve_target(target, Opener::FileManager, local.with_network_paths()),
            Ok(TargetKind::LocalPath)
        );
    }

    for target in [
        "\\\\?\\C:\\Windows",
        "\\\\.\\PhysicalDrive0",
        "//?/C:/Windows",
    ] {
        assert_eq!(
            resolve_target(target, Opener::FileManager, local),
            Err(TargetError::DevicePathsDisallowed)
        );
        assert_eq!(
            resolve_target(target, Opener::FileManager, local.with_device_paths()),
            Ok(TargetKind::LocalPath)
        );
    }

    let unc_device = "\\\\?\\UNC\\server\\share";
    assert_eq!(
        resolve_target(unc_device, Opener::FileManager, local),
        Err(TargetError::DevicePathsDisallowed)
    );
    assert_eq!(
        resolve_target(unc_device, Opener::FileManager, local.with_device_paths()),
        Err(TargetError::NetworkPathsDisallowed)
    );
    assert_eq!(
        resolve_target(
            unc_device,
            Opener::FileManager,
            local.with_device_paths().with_network_paths()
        ),
        Ok(TargetKind::LocalPath)
    );

    let old_policy: OpenPolicy =
        serde_json::from_str(r#"{"allow_web":true,"allow_local_paths":true}"#).unwrap();
    assert!(!old_policy.allow_network_paths);
    assert!(!old_policy.allow_device_paths);
    assert!(!old_policy.allow_executables);
    assert!(!old_policy.allow_unknown_file_types);
}

#[test]
fn default_app_blocks_executables_until_explicitly_allowed() {
    let local = OpenPolicy::with_local_paths();
    for extension in ["exe", "bat", "cmd", "ps1", "com", "scr", "msi"] {
        let target = format!("C:\\temp\\run.{extension}");
        assert_eq!(
            resolve_target(&target, Opener::DefaultApp, local),
            Err(TargetError::ExecutableDisallowed {
                extension: extension.to_owned(),
            })
        );
        assert_eq!(
            resolve_target(&target, Opener::FileManager, local),
            Ok(TargetKind::LocalPath)
        );
        assert_eq!(
            resolve_target(&target, Opener::DefaultApp, local.with_executables()),
            Ok(TargetKind::LocalPath)
        );
    }

    for target in [
        "C:\\temp\\RUN.EXE",
        "C:\\temp\\run.exe.",
        "C:\\temp\\run.exe:stream",
    ] {
        assert!(matches!(
            resolve_target(target, Opener::DefaultApp, local),
            Err(TargetError::ExecutableDisallowed { extension }) if extension == "exe"
        ));
    }
    assert_eq!(
        resolve_target(
            "https://example.test/setup.exe",
            Opener::DefaultApp,
            OpenPolicy::web_only()
        ),
        Ok(TargetKind::WebUrl)
    );
    assert_eq!(
        resolve_target(
            "\\\\server\\share\\run.exe",
            Opener::DefaultApp,
            local.with_network_paths()
        ),
        Err(TargetError::ExecutableDisallowed {
            extension: "exe".to_owned(),
        })
    );
    assert_eq!(
        local_file_extension("C:\\temp\\run.EXE"),
        Some("exe".to_owned())
    );
    assert_eq!(
        local_file_extension("C:\\temp\\report.txt"),
        Some("txt".to_owned())
    );
    assert!(is_safe_default_app_extension("pdf"));
    assert!(is_safe_default_app_extension("mp4"));
    assert!(is_known_executable_extension("py"));
}

#[test]
fn default_app_uses_a_safe_allowlist_and_denies_unknown_types() {
    let local = OpenPolicy::with_local_paths();
    let interpreter_extensions = [
        "py",
        "pyw",
        "jar",
        "hta",
        "pl",
        "rb",
        "sh",
        "application",
        "pif",
        "scf",
        "msc",
        "settingcontent-ms",
        "search-ms",
        "diagcab",
    ];
    for extension in interpreter_extensions {
        let target = format!("C:\\temp\\run.{extension}");
        let error = resolve_target(&target, Opener::DefaultApp, local).unwrap_err();
        assert!(
            matches!(
                error,
                TargetError::ExecutableDisallowed { .. }
                    | TargetError::UnknownFileTypeDisallowed { .. }
            ),
            "expected .{extension} to be denied, got {error:?}"
        );
        assert_eq!(
            resolve_target(&target, Opener::FileManager, local),
            Ok(TargetKind::LocalPath),
            "the file manager must still be able to reveal .{extension}"
        );
    }

    for extension in [
        "txt", "pdf", "docx", "xlsx", "csv", "png", "jpg", "svg", "html", "mp3", "mp4", "zip",
        "7z", "tar",
    ] {
        let target = format!("C:\\temp\\report.{extension}");
        assert_eq!(
            resolve_target(&target, Opener::DefaultApp, local),
            Ok(TargetKind::LocalPath),
            "safe .{extension} should open with the default application"
        );
    }

    for folder in ["C:\\Users\\me\\Documents", "C:\\Users\\me\\Documents\\"] {
        assert_eq!(
            resolve_target(folder, Opener::DefaultApp, local),
            Ok(TargetKind::LocalPath)
        );
    }

    let unknown = "C:\\temp\\data.xyz";
    assert_eq!(
        resolve_target(unknown, Opener::DefaultApp, local),
        Err(TargetError::UnknownFileTypeDisallowed {
            extension: "xyz".to_owned(),
        })
    );
    assert_eq!(
        resolve_target(unknown, Opener::DefaultApp, local.with_unknown_file_types()),
        Ok(TargetKind::LocalPath)
    );
    assert_eq!(
        resolve_target(
            "C:\\temp\\run.py",
            Opener::DefaultApp,
            local.with_unknown_file_types()
        ),
        Err(TargetError::ExecutableDisallowed {
            extension: "py".to_owned(),
        })
    );
    assert_eq!(
        resolve_target(
            "C:\\temp\\run.py",
            Opener::DefaultApp,
            local.with_executables()
        ),
        Ok(TargetKind::LocalPath)
    );
    assert_eq!(
        resolve_target(unknown, Opener::DefaultApp, local.with_executables()),
        Ok(TargetKind::LocalPath)
    );
}

#[test]
fn import_rejects_executable_and_unknown_default_app_targets() {
    let local_executable = Quicklink::new(
        QuicklinkId::from_slug("import-py").unwrap(),
        "Python",
        literal_template("C:\\temp\\run.py"),
    )
    .with_opener(Opener::DefaultApp);
    let mut library = Library::new();
    let report = library.import_json(
        &document_with(&[&local_executable]),
        ImportOptions::replace_confirmed(),
    );
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.applied, 0);
    assert!(library.is_empty());

    let unknown = Quicklink::new(
        QuicklinkId::from_slug("import-unknown").unwrap(),
        "Unknown",
        literal_template("C:\\temp\\data.xyz"),
    )
    .with_opener(Opener::DefaultApp);
    let mut library = Library::new();
    let report = library.import_json(
        &document_with(&[&unknown]),
        ImportOptions::replace_confirmed(),
    );
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.applied, 0);
    assert!(library.is_empty());

    let document = Quicklink::new(
        QuicklinkId::from_slug("import-doc").unwrap(),
        "Document",
        literal_template("C:\\temp\\report.pdf"),
    )
    .with_opener(Opener::DefaultApp);
    let mut library = Library::new();
    let report = library.import_json(
        &document_with(&[&document]),
        ImportOptions::replace_confirmed(),
    );
    assert_eq!(report.applied, 1);
    assert!(!report.has_errors());
}

#[test]
fn unicode_whitespace_and_soft_hyphen_are_rejected_in_targets() {
    for character in ['\u{00a0}', '\u{2000}', '\u{2028}', '\u{2029}', '\u{00ad}'] {
        let url = format!("https://example.test/a{character}b");
        assert!(matches!(
            resolve_target(&url, Opener::Browser, OpenPolicy::web_only()),
            Err(TargetError::UnsafeWhitespace { .. })
        ));
        let path = format!("C:\\temp\\a{character}b.txt");
        assert!(matches!(
            resolve_target(&path, Opener::FileManager, OpenPolicy::with_local_paths()),
            Err(TargetError::UnsafeWhitespace { .. })
        ));
    }
    assert_eq!(
        resolve_target(
            "C:\\temp\\report final.txt",
            Opener::FileManager,
            OpenPolicy::with_local_paths()
        ),
        Ok(TargetKind::LocalPath)
    );
}

#[test]
fn invalid_bracketed_ipv6_shapes_are_rejected() {
    for target in [
        "https://[:::]",
        "https://[1:2:3:4:5:6:7]",
        "https://[1::2::3]",
        "https://[1:2:3:4:5:6:7:8:9]",
        "https://[:1:2:3:4:5:6:7]",
    ] {
        assert!(
            resolve_target(target, Opener::Browser, OpenPolicy::web_only()).is_err(),
            "expected invalid IPv6 {target} to be rejected"
        );
    }
    for target in [
        "https://[::1]/",
        "https://[2001:db8::1]/",
        "https://[::ffff:192.0.2.1]/",
    ] {
        assert_eq!(
            resolve_target(target, Opener::Browser, OpenPolicy::web_only()),
            Ok(TargetKind::WebUrl),
            "expected valid IPv6 {target} to be accepted"
        );
    }
}

// ---------------------------------------------------------------------------
// LOW fixes: authorities, IRIs, bidi, bounds, path segments, aliases
// ---------------------------------------------------------------------------

#[test]
fn malformed_authorities_and_userinfo_are_rejected() {
    assert!(matches!(
        resolve_target("http://.", Opener::Browser, OpenPolicy::web_only()),
        Err(TargetError::InvalidHost { .. })
    ));
    assert!(matches!(
        resolve_target(
            "http://example.test:65536",
            Opener::Browser,
            OpenPolicy::web_only()
        ),
        Err(TargetError::InvalidPort { .. })
    ));
    assert!(matches!(
        resolve_target(
            "http://example.test:-1",
            Opener::Browser,
            OpenPolicy::web_only()
        ),
        Err(TargetError::InvalidPort { .. })
    ));
    assert_eq!(
        resolve_target(
            "https://user@evil.test@good.test/",
            Opener::Browser,
            OpenPolicy::web_only()
        ),
        Err(TargetError::UserInfoNotAllowed)
    );
    assert!(matches!(
        resolve_target(
            "http://example.test/%00",
            Opener::Browser,
            OpenPolicy::web_only()
        ),
        Err(TargetError::EncodedControlCharacter { byte: 0, .. })
    ));
    assert!(matches!(
        resolve_target(
            "http://example.test/%0d%0aSet-Cookie:x",
            Opener::Browser,
            OpenPolicy::web_only()
        ),
        Err(TargetError::EncodedControlCharacter { byte: 0x0d, .. })
    ));
    assert!(matches!(
        resolve_target(
            "https://example.test:80:90/",
            Opener::Browser,
            OpenPolicy::web_only()
        ),
        Err(TargetError::InvalidHost { .. })
    ));
    assert_eq!(
        resolve_target(
            "https://[::1]:8080/",
            Opener::Browser,
            OpenPolicy::web_only()
        ),
        Ok(TargetKind::WebUrl)
    );
    assert_eq!(
        resolve_target(
            "https://example.test:80/",
            Opener::Browser,
            OpenPolicy::web_only()
        ),
        Ok(TargetKind::WebUrl)
    );
}

#[test]
fn literal_unicode_urls_are_accepted_verbatim() {
    let target = "https://de.wikipedia.org/wiki/München";
    let link = Quicklink::new(
        QuicklinkId::from_slug("unicode-url").unwrap(),
        "Unicode URL",
        literal_template(target),
    );
    let preview = Library::new()
        .preview(&link, &BTreeMap::new(), OpenPolicy::web_only(), &probe())
        .unwrap();
    assert_eq!(preview.target(), target);
}

#[test]
fn invisible_format_controls_are_rejected() {
    let format_override = '\u{202e}';
    let id = QuicklinkId::from_slug("bidi").unwrap();
    let title = Quicklink::new(
        id.clone(),
        format!("safe{format_override}txt.exe"),
        literal_template("https://example.test/x"),
    );
    assert!(matches!(
        title.validate(),
        Err(ModelError::UnsafeFormatInTitle { .. })
    ));

    let alias = Quicklink::new(
        id.clone(),
        "Safe",
        literal_template("https://example.test/x"),
    )
    .with_alias(format!("bad{format_override}alias"));
    assert!(matches!(
        alias.validate(),
        Err(ModelError::UnsafeFormatInAlias { .. })
    ));

    let tag = Quicklink::new(id, "Safe", literal_template("https://example.test/x"))
        .with_tags([format!("bad{format_override}tag")]);
    assert!(matches!(
        tag.validate(),
        Err(ModelError::UnsafeFormatInTag { .. })
    ));

    assert!(matches!(
        resolve_target(
            &format!("C:\\temp\\safe{format_override}txt.exe"),
            Opener::FileManager,
            OpenPolicy::with_local_paths()
        ),
        Err(TargetError::UnsafeFormatCharacter { .. })
    ));
    assert!(matches!(
        Template::parse(
            &format!("https://example.test/{format_override}evil"),
            Vec::new()
        ),
        Err(TemplateError::UnsafeFormatCharacter { .. })
    ));
    assert!(matches!(
        encode_literal_segment(&format!("safe{format_override}txt.exe")),
        Err(EncodeError::UnsafeFormatCharacter { .. })
    ));
}

#[test]
fn resolved_target_is_bounded_before_large_allocation() {
    let template = Template::parse(
        "https://example.test/{q}",
        vec![ArgSpec::new("q", ArgKind::Text, Encoding::UrlQueryValue)],
    )
    .unwrap();
    let oversized = "a".repeat(MAX_RESOLVED_TARGET_BYTES + 1);
    assert!(matches!(
        template.resolve_text(&values(&[("q", oversized.as_str())])),
        Err(ResolveError::TargetTooLong { .. })
    ));

    let huge_default = "a".repeat(MAX_RESOLVED_TARGET_BYTES + 1);
    assert!(matches!(
        Template::parse(
            "{q}",
            vec![
                ArgSpec::new("q", ArgKind::Text, Encoding::UrlQueryValue)
                    .optional(ArgumentValue::text(huge_default))
            ]
        ),
        Err(TemplateError::DefaultTooLong { .. })
    ));
}

#[test]
fn ambiguous_dot_and_space_path_segments_are_rejected() {
    for target in [
        "C:\\temp\\...\\file.txt",
        "C:\\temp\\.. \\file.txt",
        "/srv/./file.txt",
    ] {
        assert!(
            matches!(
                resolve_target(target, Opener::FileManager, OpenPolicy::with_local_paths()),
                Err(TargetError::AmbiguousPathSegment { .. })
            ),
            "expected {target} to be rejected"
        );
    }
}

#[test]
fn alias_grammar_matches_settings_alias_prefixes() {
    let id = QuicklinkId::from_slug("alias-grammar").unwrap();
    let base = || {
        Quicklink::new(
            id.clone(),
            "Alias",
            literal_template("https://example.test/alias"),
        )
    };
    assert!(matches!(
        base().with_alias("my link").validate(),
        Err(ModelError::InvalidAliasCharacter { character: ' ', .. })
    ));
    assert!(matches!(
        base().with_alias("-bad").validate(),
        Err(ModelError::AliasMustStartAlphanumeric)
    ));
    assert!(matches!(
        base()
            .with_alias("a".repeat(MAX_ALIAS_CHARS + 1))
            .validate(),
        Err(ModelError::AliasTooLong { .. })
    ));
    base().with_alias("my.link_1").validate().unwrap();
}
