//! Feature composition and the frozen provider/handler contracts.
//!
//! [`FeatureRegistry`] is the single place where feature crates register
//! searchable providers and command handlers. It is deliberately independent
//! of GPUI and of storage so the same composition works for the desktop, CLI
//! diagnostics and headless tests.
//!
//! # Frozen interfaces
//!
//! * [`SearchProvider`] contributes commands to a query. It exposes a stable
//!   name, a priority, readiness and the actual query/results pair.
//! * [`CommandHandler`] previews, executes and cancels commands in the
//!   namespace it owns. A handler's [`CommandHandler::name`] is the namespace
//!   matched against `CommandId::namespace()`.
//! * [`ansa_tools::Tool`] remains the low-level, schema-described operation
//!   contract. The registry can hold tools but does not execute them.
//!
//! # Merge rules
//!
//! Providers are consulted in ascending priority order (lower runs first) and
//! then by name for determinism. Results keep each provider's ordering; the
//! registry only removes duplicate command identities and applies the caller's
//! result limit. This keeps priority meaningful instead of letting a large
//! provider drown a deliberate high-priority one.
//!
//! Handlers are keyed by namespace. Registering two handlers for the same
//! namespace replaces the earlier one; that makes composition idempotent
//! during startup and tests.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use ansa_domain::Command;
use ansa_tools::Tool;

/// An error raised by a feature provider or command handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureError {
    message: String,
}

impl FeatureError {
    /// Creates an error with a display-safe message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The display-safe failure message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for FeatureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for FeatureError {}

/// A bounded query handed to every provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchQuery<'a> {
    /// Raw query text. Providers must not mutate it or log it.
    pub text: &'a str,
    /// Maximum results the provider may return.
    pub limit: usize,
}

/// One result from a search provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    /// Searchable command.
    pub command: Command,
    /// Provider-local relevance score. Higher is better within one provider.
    pub score: u32,
}

impl SearchResult {
    /// Creates a result with an explicit provider-local score.
    pub fn new(command: Command, score: u32) -> Self {
        Self { command, score }
    }
}

/// Contributes commands to a query.
///
/// A provider must not perform blocking I/O inside [`SearchProvider::query`].
/// Expensive refreshes belong in a background task that updates the provider's
/// cached state; [`SearchProvider::ready`] reports whether that state is usable.
pub trait SearchProvider {
    /// Stable diagnostic name, for example `catalog` or `snippets`.
    fn name(&self) -> &str;

    /// Lower priorities are consulted first. Use negative values for pinned
    /// providers and positive values for optional expansions.
    fn priority(&self) -> i32;

    /// Whether the provider's cached state is usable for a query right now.
    fn ready(&self) -> bool;

    /// Returns at most `query.limit` results for `query.text`.
    fn query(&self, query: SearchQuery<'_>) -> Vec<SearchResult>;

    /// Convenience alias for [`SearchProvider::query`].
    ///
    /// Kept as a distinct frozen method so callers can express "give me the
    /// results" without depending on a particular implementation's caching.
    fn results(&self, query: SearchQuery<'_>) -> Vec<SearchResult> {
        self.query(query)
    }
}

/// A non-executing preview of a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandPreview {
    /// Preview title.
    pub title: String,
    /// Preview body. Must not contain secrets or clipboard bodies.
    pub body: String,
}

impl CommandPreview {
    /// Creates a preview.
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
        }
    }
}

/// The result of executing a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandlerOutcome {
    /// Display-safe status line.
    pub status: String,
    /// Whether the launcher should close after the action.
    pub dismiss: bool,
}

impl HandlerOutcome {
    /// Creates an outcome.
    pub fn new(status: impl Into<String>, dismiss: bool) -> Self {
        Self {
            status: status.into(),
            dismiss,
        }
    }
}

/// Previews, executes and cancels commands in one namespace.
///
/// [`CommandHandler::name`] is matched against the command identity namespace
/// (`app`, `quicklink`, `snippet`, `script`, `mcp`, `agent`, and so on).
/// Handlers own their side effects; the registry only routes.
pub trait CommandHandler {
    /// Namespace this handler owns.
    fn name(&self) -> &str;

    /// Produces a preview without executing anything.
    fn preview(&self, command: &Command) -> Result<CommandPreview, FeatureError>;

    /// Executes a command. Implementations must re-check any mutable target
    /// before acting; a stale preview is not permission to execute.
    fn execute(&mut self, command: &Command) -> Result<HandlerOutcome, FeatureError>;

    /// Cancels in-flight work for a command, if any. The default is a no-op.
    fn cancel(&mut self, _command: &Command) -> Result<(), FeatureError> {
        Ok(())
    }
}

/// The composition root for feature providers, handlers and tools.
#[derive(Default)]
pub struct FeatureRegistry {
    providers: Vec<Box<dyn SearchProvider>>,
    handlers: BTreeMap<String, Box<dyn CommandHandler>>,
    tools: Vec<Box<dyn Tool>>,
}

impl FeatureRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a search provider.
    pub fn register_provider(&mut self, provider: impl SearchProvider + 'static) -> &mut Self {
        self.providers.push(Box::new(provider));
        self
    }

    /// Registers a command handler, replacing any handler with the same name.
    pub fn register_handler(&mut self, handler: impl CommandHandler + 'static) -> &mut Self {
        self.handlers
            .insert(handler.name().to_owned(), Box::new(handler));
        self
    }

    /// Registers a tool, replacing any tool with the same name.
    pub fn register_tool(&mut self, tool: impl Tool + 'static) -> &mut Self {
        let name = tool.name().to_owned();
        self.tools.retain(|existing| existing.name() != name);
        self.tools.push(Box::new(tool));
        self
    }

    /// Provider names in registration order.
    pub fn provider_names(&self) -> Vec<&str> {
        self.providers
            .iter()
            .map(|provider| provider.name())
            .collect()
    }

    /// Handler namespaces in deterministic order.
    pub fn handler_names(&self) -> Vec<&str> {
        self.handlers.keys().map(String::as_str).collect()
    }

    /// Registered tools in deterministic name order.
    pub fn tools(&self) -> impl Iterator<Item = &dyn Tool> {
        let mut tools: Vec<&dyn Tool> = self.tools.iter().map(|tool| tool.as_ref()).collect();
        tools.sort_by_key(|tool| tool.name());
        tools.into_iter()
    }

    /// Looks up a tool by exact name.
    pub fn tool(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|tool| tool.name() == name)
            .map(|tool| tool.as_ref())
    }

    /// Runs every ready provider and returns a bounded, de-duplicated result
    /// set in provider-priority order.
    pub fn search(&self, text: &str, limit: usize) -> Vec<SearchResult> {
        if limit == 0 {
            return Vec::new();
        }
        let mut ordered: Vec<&dyn SearchProvider> = self
            .providers
            .iter()
            .map(|provider| provider.as_ref())
            .collect();
        ordered.sort_by(|left, right| {
            left.priority()
                .cmp(&right.priority())
                .then_with(|| left.name().cmp(right.name()))
        });

        let query = SearchQuery { text, limit };
        let mut seen = BTreeSet::new();
        let mut merged = Vec::new();
        for provider in ordered {
            if !provider.ready() {
                continue;
            }
            for result in provider.results(query) {
                if !seen.insert(result.command.id.clone()) {
                    continue;
                }
                merged.push(result);
                if merged.len() == limit {
                    return merged;
                }
            }
        }
        merged
    }

    /// Looks up the handler that owns a command namespace.
    pub fn handler_for(&self, command: &Command) -> Option<&dyn CommandHandler> {
        self.handlers
            .get(command.id.namespace())
            .map(|handler| handler.as_ref())
    }

    /// Mutable handler lookup used by the dispatch path.
    pub fn handler_for_mut(&mut self, command: &Command) -> Option<&mut Box<dyn CommandHandler>> {
        self.handlers.get_mut(command.id.namespace())
    }

    /// Previews a command through its namespace handler.
    pub fn preview(&self, command: &Command) -> Result<CommandPreview, FeatureError> {
        self.handler_for(command)
            .ok_or_else(|| missing_handler(command.id.namespace()))?
            .preview(command)
    }

    /// Executes a command through its namespace handler.
    pub fn execute(&mut self, command: &Command) -> Result<HandlerOutcome, FeatureError> {
        self.handler_for_mut(command)
            .ok_or_else(|| missing_handler(command.id.namespace()))?
            .execute(command)
    }

    /// Cancels a command through its namespace handler.
    pub fn cancel(&mut self, command: &Command) -> Result<(), FeatureError> {
        self.handler_for_mut(command)
            .ok_or_else(|| missing_handler(command.id.namespace()))?
            .cancel(command)
    }
}

impl fmt::Debug for FeatureRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FeatureRegistry")
            .field("providers", &self.provider_names())
            .field("handlers", &self.handler_names())
            .field(
                "tools",
                &self
                    .tools
                    .iter()
                    .map(|tool| tool.name())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

fn missing_handler(namespace: &str) -> FeatureError {
    FeatureError::new(format!(
        "no command handler is registered for namespace {namespace:?}"
    ))
}

/// Builds the default feature registry.
///
/// Feature crates add their own providers and handlers during integration. The
/// scaffold intentionally starts empty so no unimplemented surface can claim
/// availability.
pub fn build_registry() -> FeatureRegistry {
    FeatureRegistry::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ansa_domain::{CommandId, CommandKind};
    use ansa_tools::{ApprovalRequirement, ToolError, ToolOutput, ToolSchema};
    use serde_json::json;

    fn command(id: &str, title: &str) -> Command {
        Command::new(
            CommandId::parse(id).expect("test command id"),
            title,
            CommandKind::Builtin(ansa_domain::BuiltinCommand::About),
        )
    }

    struct Provider {
        name: &'static str,
        priority: i32,
        ready: bool,
        results: Vec<SearchResult>,
    }

    impl SearchProvider for Provider {
        fn name(&self) -> &str {
            self.name
        }

        fn priority(&self) -> i32 {
            self.priority
        }

        fn ready(&self) -> bool {
            self.ready
        }

        fn query(&self, query: SearchQuery<'_>) -> Vec<SearchResult> {
            self.results.iter().take(query.limit).cloned().collect()
        }
    }

    #[test]
    fn search_orders_by_priority_skips_unready_and_deduplicates() {
        let mut registry = FeatureRegistry::new();
        registry.register_provider(Provider {
            name: "late",
            priority: 10,
            ready: true,
            results: vec![
                SearchResult::new(command("app:late", "Late"), 90),
                SearchResult::new(command("app:shared", "Late shared"), 80),
            ],
        });
        registry.register_provider(Provider {
            name: "early",
            priority: -5,
            ready: true,
            results: vec![
                SearchResult::new(command("app:early", "Early"), 1),
                SearchResult::new(command("app:shared", "Early shared"), 99),
            ],
        });
        registry.register_provider(Provider {
            name: "disabled",
            priority: -100,
            ready: false,
            results: vec![SearchResult::new(command("app:disabled", "Disabled"), 100)],
        });

        let results = registry.search("anything", 10);
        let ids: Vec<_> = results
            .iter()
            .map(|result| result.command.id.as_str())
            .collect();
        assert_eq!(ids, vec!["app:early", "app:shared", "app:late"]);
        assert_eq!(results[1].command.title, "Early shared");
        assert_eq!(registry.search("anything", 2).len(), 2);
        assert!(registry.search("anything", 0).is_empty());
    }

    struct Handler {
        namespace: &'static str,
        previews: usize,
        executions: usize,
        cancels: usize,
    }

    impl CommandHandler for Handler {
        fn name(&self) -> &str {
            self.namespace
        }

        fn preview(&self, command: &Command) -> Result<CommandPreview, FeatureError> {
            Ok(CommandPreview::new(
                command.title.clone(),
                format!("{} previews", self.previews),
            ))
        }

        fn execute(&mut self, _command: &Command) -> Result<HandlerOutcome, FeatureError> {
            self.executions += 1;
            Ok(HandlerOutcome::new("executed", true))
        }

        fn cancel(&mut self, _command: &Command) -> Result<(), FeatureError> {
            self.cancels += 1;
            Ok(())
        }
    }

    #[test]
    fn handlers_are_routed_by_command_namespace() {
        let mut registry = FeatureRegistry::new();
        registry.register_handler(Handler {
            namespace: "snippet",
            previews: 1,
            executions: 0,
            cancels: 0,
        });
        registry.register_handler(Handler {
            namespace: "mcp",
            previews: 1,
            executions: 0,
            cancels: 0,
        });

        let snippet = command("snippet:greeting", "Greeting");
        let preview = registry.preview(&snippet).expect("preview");
        assert_eq!(preview.title, "Greeting");
        assert!(preview.body.contains("previews"));

        let outcome = registry.execute(&snippet).expect("execute");
        assert_eq!(outcome, HandlerOutcome::new("executed", true));
        registry.cancel(&snippet).expect("cancel");

        let missing = command("script:cleanup", "Cleanup");
        assert!(registry.preview(&missing).is_err());
        assert_eq!(registry.handler_names(), vec!["mcp", "snippet"]);
    }

    struct EchoTool;

    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "test:echo"
        }

        fn schema(&self) -> &ToolSchema {
            static SCHEMA: std::sync::OnceLock<ToolSchema> = std::sync::OnceLock::new();
            SCHEMA.get_or_init(|| ToolSchema::input(json!({"type": "object"})))
        }

        fn approval(&self) -> ApprovalRequirement {
            ApprovalRequirement::NotRequired
        }

        fn execute(&self, input: &serde_json::Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::new(input.clone(), "echo"))
        }
    }

    #[test]
    fn registry_composes_tools_without_executing_them() {
        let mut registry = FeatureRegistry::new();
        registry.register_tool(EchoTool);
        assert!(registry.tool("test:echo").is_some());
        assert!(registry.tool("test:missing").is_none());
        assert_eq!(registry.tools().count(), 1);
        assert_eq!(
            registry.tool("test:echo").unwrap().schema().input["type"],
            "object"
        );
    }
}
