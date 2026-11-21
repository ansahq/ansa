//! Composes the searchable alpha catalog and the calculator/emoji side channels.
//!
//! Calculator routing is deliberately delegated to the public
//! [`ansa_core::query::route_query`] entry point. The trigger is therefore the
//! same one the core and GPUI already use: an explicit `=` prefix, or a
//! digit-containing string made only of whitespace and `+ - * / ( ) .`.
//! Application names such as `7-Zip` and `1Password` are never swallowed.

use std::collections::BTreeMap;
use std::fmt;

use ansa_core::{Engine, query::QueryRoute, query::route_query};
use ansa_domain::{BuiltinCommand, Command, CommandId, CommandKind};
use ansa_platform_windows::AppEntry;
use ansa_quicklinks::{
    Library, OpenPolicy, Opener, OpenerAvailability, OpenerProbe, StaticOpenerProbe, TargetKind,
};

use crate::error::AlphaError;

/// One bounded emoji result with its licensed dataset metadata available from
/// [`crate::emoji_info`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmojiHit {
    /// The exact base grapheme to copy.
    pub grapheme: String,
    /// Human-readable English name.
    pub name: String,
    /// Curated group.
    pub group: String,
    /// Accessible base name.
    pub accessible_name: String,
    /// Skin-tone and other variant graphemes with their accessible names.
    pub variants: Vec<(String, String)>,
}

/// Whether a query was handled as a catalog search or local arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryKind {
    /// Ranked catalog search, with optional emoji hits.
    Search,
    /// Local calculator evaluation.
    Calculator,
}

/// The combined result of one alpha query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchOutcome {
    /// Which route handled the query.
    pub kind: QueryKind,
    /// Ranked commands for a search query; empty for calculator queries.
    pub commands: Vec<Command>,
    /// Formatted value or human-readable calculator error.
    pub calculator: Option<Result<String, String>>,
    /// Bounded emoji hits for a non-empty search query.
    pub emoji: Vec<EmojiHit>,
}

/// A quicklink that was intentionally left out of the launchable catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedQuicklink {
    /// Stable quicklink identity.
    pub id: String,
    /// Display title at composition time.
    pub title: String,
    /// Why it cannot be represented as a directly launchable command.
    pub reason: QuicklinkSkipReason,
}

/// Why a persisted quicklink was not composed into the launchable catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuicklinkSkipReason {
    /// The template declares required arguments, so there is no single target.
    RequiredArguments,
    /// The opener is not a web-compatible opener used by this catalog.
    UnsupportedOpener {
        /// User-facing opener label.
        opener: &'static str,
    },
    /// Preview/target resolution failed.
    Unresolved {
        /// User-facing resolution error.
        reason: String,
    },
    /// The resolved target is a local path, not an HTTP(S) URL.
    NotWebTarget {
        /// Stable target-kind label.
        target_kind: &'static str,
    },
    /// The static opener probe reports the opener as unavailable.
    OpenerUnavailable {
        /// User-facing reason.
        reason: String,
    },
    /// A resolved target still contained an unresolved placeholder.
    PlaceholderTarget,
}

impl fmt::Display for QuicklinkSkipReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RequiredArguments => formatter.write_str("required arguments are not supplied"),
            Self::UnsupportedOpener { opener } => write!(
                formatter,
                "opener {opener} is not supported by the web launch path"
            ),
            Self::Unresolved { reason } => write!(formatter, "preview failed: {reason}"),
            Self::NotWebTarget { target_kind } => write!(
                formatter,
                "resolved target kind {target_kind} is not an HTTP(S) URL"
            ),
            Self::OpenerUnavailable { reason } => {
                write!(formatter, "opener is unavailable: {reason}")
            }
            Self::PlaceholderTarget => {
                formatter.write_str("resolved target still contains an unresolved placeholder")
            }
        }
    }
}

/// Result of composing the alpha catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogComposition {
    /// Commands that can be searched and acted on directly.
    pub commands: Vec<Command>,
    /// Enabled quicklinks intentionally omitted from `commands`, with reasons.
    pub skipped_quicklinks: Vec<SkippedQuicklink>,
}

impl CatalogComposition {
    /// Number of directly launchable quicklink commands.
    pub fn launchable_quicklinks(&self) -> usize {
        self.commands
            .iter()
            .filter(|command| matches!(command.kind, CommandKind::Quicklink { .. }))
            .count()
    }
}

/// Builds the alpha catalog: built-ins, discovered applications (including
/// packaged and broken entries), and every enabled quicklink that resolves to a
/// directly launchable HTTP(S) target.
///
/// Quicklinks are resolved through the public preview/target policy with their
/// declared defaults. A quicklink is emitted only when:
///
/// * it declares no required arguments;
/// * its opener is [`Opener::Browser`] or [`Opener::DefaultApp`];
/// * preview succeeds and the resolved target kind is [`TargetKind::WebUrl`];
/// * the opener is available to a static [`StaticOpenerProbe`];
/// * the resolved target still contains no `{`/`}` placeholder.
///
/// Required-argument, local-path, FileManager, unsafe and unresolvable
/// quicklinks are omitted and reported in [`CatalogComposition::skipped_quicklinks`].
/// The raw `template().source()` is never used as a launch target.
///
/// The static probe cannot know whether the operating system currently has a
/// browser registered; the platform `open_url` path performs its own launch-time
/// validation.
pub fn compose_catalog(
    quicklinks: &Library,
    apps: &[AppEntry],
) -> Result<CatalogComposition, AlphaError> {
    let mut commands: Vec<Command> = BuiltinCommand::ALL
        .into_iter()
        .map(|builtin| {
            let subtitle = match builtin {
                BuiltinCommand::Settings => "Local settings and capability status",
                BuiltinCommand::RefreshApplications => "Rescan the Windows application catalog",
                BuiltinCommand::About => "About this alpha build",
            };
            builtin
                .command()
                .with_subtitle(subtitle)
                .with_keywords(["ansa", "builtin"])
        })
        .collect();

    for app in apps {
        let id = CommandId::new(&app.id).map_err(|error| {
            AlphaError::Catalog(format!(
                "application entry {:?} has an invalid identity: {error}",
                app.title
            ))
        })?;
        let subtitle = if let Some(reason) = &app.broken {
            format!("Broken shortcut · {reason} · refresh required")
        } else if app.packaged {
            "Packaged application · Windows".to_owned()
        } else {
            "Application · Windows Start Menu".to_owned()
        };
        let mut keywords = vec!["application".to_owned(), "app".to_owned()];
        if app.packaged {
            keywords.push("packaged".to_owned());
        }
        commands.push(
            Command::new(
                id,
                app.title.clone(),
                CommandKind::Application {
                    path: app.path.clone(),
                },
            )
            .with_subtitle(subtitle)
            .with_keywords(keywords),
        );
    }

    let probe = StaticOpenerProbe::all_available();
    let defaults = BTreeMap::new();
    let mut skipped_quicklinks = Vec::new();

    for link in quicklinks.list() {
        if !link.enabled() {
            continue;
        }
        let id = link.id().to_string();
        let title = link.title().to_owned();

        if link
            .template()
            .arguments()
            .iter()
            .any(|argument| argument.required)
        {
            skipped_quicklinks.push(SkippedQuicklink {
                id,
                title,
                reason: QuicklinkSkipReason::RequiredArguments,
            });
            continue;
        }

        if !matches!(link.opener(), Opener::Browser | Opener::DefaultApp) {
            skipped_quicklinks.push(SkippedQuicklink {
                id,
                title,
                reason: QuicklinkSkipReason::UnsupportedOpener {
                    opener: link.opener().label(),
                },
            });
            continue;
        }

        let preview =
            match quicklinks.preview(link, &defaults, OpenPolicy::with_local_paths(), &probe) {
                Ok(preview) => preview,
                Err(error) => {
                    skipped_quicklinks.push(SkippedQuicklink {
                        id,
                        title,
                        reason: QuicklinkSkipReason::Unresolved {
                            reason: error.to_string(),
                        },
                    });
                    continue;
                }
            };

        if preview.target_kind() != TargetKind::WebUrl {
            skipped_quicklinks.push(SkippedQuicklink {
                id,
                title,
                reason: QuicklinkSkipReason::NotWebTarget {
                    target_kind: preview.target_kind().label(),
                },
            });
            continue;
        }

        let availability = probe.probe(preview.opener());
        if !availability.is_available() {
            let reason = match availability {
                OpenerAvailability::Unavailable { reason } => reason,
                OpenerAvailability::Available => String::new(),
            };
            skipped_quicklinks.push(SkippedQuicklink {
                id,
                title,
                reason: QuicklinkSkipReason::OpenerUnavailable { reason },
            });
            continue;
        }

        let target = preview.target();
        if target.trim().is_empty() || target.contains('{') || target.contains('}') {
            skipped_quicklinks.push(SkippedQuicklink {
                id,
                title,
                reason: QuicklinkSkipReason::PlaceholderTarget,
            });
            continue;
        }

        let command_id = CommandId::new(&id).map_err(|error| {
            AlphaError::Catalog(format!(
                "quicklink {:?} has an invalid identity: {error}",
                link.title()
            ))
        })?;
        let mut keywords = vec!["quicklink".to_owned(), "link".to_owned()];
        keywords.extend(link.tags().iter().cloned());
        let mut command = Command::new(
            command_id,
            link.title(),
            CommandKind::Quicklink {
                url: target.to_owned(),
            },
        )
        .with_subtitle(if link.alias().is_some() {
            "Quicklink · alias available"
        } else {
            "Quicklink"
        })
        .with_keywords(keywords);
        if let Some(alias) = link.alias() {
            command = command.with_alias(alias);
        }
        commands.push(command);
    }

    Ok(CatalogComposition {
        commands,
        skipped_quicklinks,
    })
}

/// Searches the embedded emoji dataset with the crate's own result bound.
pub fn search_emoji(query: &str, limit: usize) -> Result<Vec<EmojiHit>, AlphaError> {
    let hits =
        ansa_emoji::search(query, limit).map_err(|error| AlphaError::Emoji(error.to_string()))?;
    Ok(hits
        .into_iter()
        .map(|hit| EmojiHit {
            grapheme: hit.entry.base().to_owned(),
            name: hit.entry.name().to_owned(),
            group: hit.entry.group().to_owned(),
            accessible_name: hit.entry.accessible_name().to_owned(),
            variants: hit
                .entry
                .variants()
                .iter()
                .map(|variant| {
                    (
                        variant.grapheme().to_owned(),
                        variant.accessible_name().to_owned(),
                    )
                })
                .collect(),
        })
        .collect())
}

/// Runs one alpha query against an already-composed catalog.
///
/// Calculator queries return no commands and no emoji so a numeric-looking
/// application name can never be launched by accident when the user explicitly
/// asked for arithmetic.
pub fn search_catalog(
    catalog: &[Command],
    query: &str,
    emoji_limit: usize,
) -> Result<SearchOutcome, AlphaError> {
    match route_query(query) {
        QueryRoute::Calculator(result) => Ok(SearchOutcome {
            kind: QueryKind::Calculator,
            commands: Vec::new(),
            calculator: Some(match result {
                Ok(value) => Ok(value.format()),
                Err(error) => Err(error.to_string()),
            }),
            emoji: Vec::new(),
        }),
        QueryRoute::Search => {
            let mut engine = Engine::new(catalog.to_vec());
            engine.set_query(query);
            let emoji = if query.trim().is_empty() {
                Vec::new()
            } else {
                search_emoji(query, emoji_limit)?
            };
            Ok(SearchOutcome {
                kind: QueryKind::Search,
                commands: engine.results().to_vec(),
                calculator: None,
                emoji,
            })
        }
    }
}
