//! Catalog composition must only emit directly launchable, resolved quicklink
//! targets. These tests exercise the real quicklink preview/target policy.

use std::collections::BTreeMap;

use ansa_alpha::{QuicklinkSkipReason, compose_catalog};
use ansa_domain::CommandKind;
use ansa_quicklinks::{
    ArgKind, ArgSpec, ArgumentValue, Encoding, ImportOptions, Library, OpenPolicy, Opener,
    Quicklink, QuicklinkId, StaticOpenerProbe, Template,
};

fn probe() -> StaticOpenerProbe {
    StaticOpenerProbe::all_available()
}

fn policy() -> OpenPolicy {
    OpenPolicy::with_local_paths()
}

fn literal_template(source: &str) -> Template {
    Template::parse(source, Vec::new()).expect("valid literal template")
}

fn insert(library: &mut Library, link: &Quicklink, values: &BTreeMap<String, String>) {
    let preview = library
        .preview(link, values, policy(), &probe())
        .expect("preview");
    library.commit(&preview).expect("commit");
}

fn empty_values() -> BTreeMap<String, String> {
    BTreeMap::new()
}

#[test]
fn static_web_quicklink_is_composed_with_the_resolved_target() {
    let mut library = Library::new();
    let link = Quicklink::new(
        QuicklinkId::from_slug("docs").expect("id"),
        "Docs",
        literal_template("https://example.test/docs"),
    )
    .with_alias("docs");
    insert(&mut library, &link, &empty_values());

    let composition = compose_catalog(&library, &[]).expect("compose");
    let command = composition
        .commands
        .iter()
        .find(|command| command.id.as_str() == link.id().as_str())
        .expect("static quicklink is launchable");
    match &command.kind {
        CommandKind::Quicklink { url } => {
            assert_eq!(url, "https://example.test/docs");
            assert_eq!(command.alias.as_deref(), Some("docs"));
        }
        other => panic!("expected quicklink command, got {other:?}"),
    }
    assert!(composition.skipped_quicklinks.is_empty());
    assert_eq!(composition.launchable_quicklinks(), 1);
}

#[test]
fn optional_default_arguments_resolve_into_the_composed_target() {
    let mut library = Library::new();
    let template = Template::parse(
        "https://example.test/search?lang={lang}",
        vec![
            ArgSpec::new("lang", ArgKind::Text, Encoding::UrlQueryValue)
                .optional(ArgumentValue::text("en")),
        ],
    )
    .expect("template");
    let link = Quicklink::new(
        QuicklinkId::from_slug("search-default").expect("id"),
        "Search with default",
        template,
    );
    insert(&mut library, &link, &empty_values());

    let composition = compose_catalog(&library, &[]).expect("compose");
    let command = composition
        .commands
        .iter()
        .find(|command| command.id.as_str() == link.id().as_str())
        .expect("defaulted quicklink is launchable");
    match &command.kind {
        CommandKind::Quicklink { url } => {
            assert_eq!(url, "https://example.test/search?lang=en");
        }
        other => panic!("expected quicklink command, got {other:?}"),
    }
}

#[test]
fn required_argument_template_is_never_launchable() {
    let mut library = Library::new();
    let template = Template::parse(
        "https://example.test/search?q={query}",
        vec![ArgSpec::new(
            "query",
            ArgKind::Text,
            Encoding::UrlQueryValue,
        )],
    )
    .expect("template");
    let link = Quicklink::new(
        QuicklinkId::from_slug("search").expect("id"),
        "Search",
        template,
    );
    let mut seeded = BTreeMap::new();
    seeded.insert("query".to_owned(), "seed".to_owned());
    insert(&mut library, &link, &seeded);

    let composition = compose_catalog(&library, &[]).expect("compose");
    assert!(
        composition
            .commands
            .iter()
            .all(|command| !matches!(command.kind, CommandKind::Quicklink { .. })),
        "a required-argument quicklink must not be a launch command"
    );
    let skipped = composition
        .skipped_quicklinks
        .iter()
        .find(|skipped| skipped.id.as_str() == link.id().as_str())
        .expect("required-argument quicklink is reported");
    assert_eq!(skipped.reason, QuicklinkSkipReason::RequiredArguments);
}

#[test]
fn local_path_quicklinks_are_not_emitted_as_http_quicklinks() {
    let mut library = Library::new();
    let file_manager = Quicklink::new(
        QuicklinkId::from_slug("notes").expect("id"),
        "Notes",
        literal_template("C:\\temp\\notes.txt"),
    )
    .with_opener(Opener::FileManager);
    insert(&mut library, &file_manager, &empty_values());

    let default_app = Quicklink::new(
        QuicklinkId::from_slug("calculator").expect("id"),
        "Calculator",
        literal_template("C:\\temp\\notes.txt"),
    )
    .with_opener(Opener::DefaultApp);
    insert(&mut library, &default_app, &empty_values());

    let composition = compose_catalog(&library, &[]).expect("compose");
    assert!(
        composition
            .commands
            .iter()
            .all(|command| !matches!(command.kind, CommandKind::Quicklink { .. })),
        "local paths must not be emitted on the HTTP quicklink path"
    );
    let file_manager_skip = composition
        .skipped_quicklinks
        .iter()
        .find(|skipped| skipped.id.as_str() == file_manager.id().as_str())
        .expect("FileManager quicklink is reported");
    assert_eq!(
        file_manager_skip.reason,
        QuicklinkSkipReason::UnsupportedOpener {
            opener: "File Manager"
        }
    );
    let default_app_skip = composition
        .skipped_quicklinks
        .iter()
        .find(|skipped| skipped.id.as_str() == default_app.id().as_str())
        .expect("local DefaultApp quicklink is reported");
    assert_eq!(
        default_app_skip.reason,
        QuicklinkSkipReason::NotWebTarget {
            target_kind: "local_path"
        }
    );
}

#[test]
fn unsafe_literal_rejected_by_import_is_never_composed() {
    let id = QuicklinkId::from_slug("unsafe").expect("id");
    let document = serde_json::json!({
        "schema_version": 1,
        "quicklinks": [{
            "id": id.as_str(),
            "title": "Unsafe",
            "alias": null,
            "tags": [],
            "template": {
                "source": "javascript:alert(1)",
                "arguments": []
            },
            "opener": "browser",
            "enabled": true
        }]
    });
    let mut library = Library::new();
    let report = library.import_json(&document.to_string(), ImportOptions::replace_confirmed());
    assert!(report.has_errors(), "unsafe literal must be rejected");
    assert!(library.is_empty(), "a rejected import must not be applied");
    assert!(
        report
            .errors
            .iter()
            .any(|error| error.to_string().contains("javascript")
                || error.to_string().contains("scheme")),
        "rejection must name the unsafe scheme: {:?}",
        report.errors
    );

    let composition = compose_catalog(&library, &[]).expect("compose");
    assert!(
        composition
            .commands
            .iter()
            .all(|command| !matches!(command.kind, CommandKind::Quicklink { .. }))
    );
    assert!(composition.skipped_quicklinks.is_empty());
}

#[test]
fn no_composed_quicklink_target_contains_an_unresolved_placeholder() {
    let mut library = Library::new();
    insert(
        &mut library,
        &Quicklink::new(
            QuicklinkId::from_slug("docs").expect("id"),
            "Docs",
            literal_template("https://example.test/docs"),
        ),
        &empty_values(),
    );
    let required = Quicklink::new(
        QuicklinkId::from_slug("search").expect("id"),
        "Search",
        Template::parse(
            "https://example.test/search?q={query}",
            vec![ArgSpec::new(
                "query",
                ArgKind::Text,
                Encoding::UrlQueryValue,
            )],
        )
        .expect("template"),
    );
    let mut seeded = BTreeMap::new();
    seeded.insert("query".to_owned(), "seed".to_owned());
    insert(&mut library, &required, &seeded);
    insert(
        &mut library,
        &Quicklink::new(
            QuicklinkId::from_slug("notes").expect("id"),
            "Notes",
            literal_template("C:\\temp\\notes.txt"),
        )
        .with_opener(Opener::FileManager),
        &empty_values(),
    );

    let composition = compose_catalog(&library, &[]).expect("compose");
    for command in &composition.commands {
        if let CommandKind::Quicklink { url } = &command.kind {
            assert!(
                url.starts_with("https://") || url.starts_with("http://"),
                "quicklink target must be an HTTP(S) URL: {url}"
            );
            assert!(
                !url.contains('{') && !url.contains('}'),
                "quicklink target must not contain a placeholder: {url}"
            );
        }
    }
}
