//! The frozen tool interface shared by commands, agents and MCP integrations.
//!
//! A [`Tool`] is a named, schema-described operation. The launcher does not
//! execute tools directly: a feature handler asks for approval when
//! [`Tool::approval`] requires it, validates input against [`Tool::schema`],
//! and then calls [`Tool::execute`]. Keeping this contract in a tiny crate
//! prevents feature crates from inventing incompatible tool vocabularies.
//!
//! This crate deliberately contains no transports, no approval UI and no
//! secrets. Input and output are JSON values; callers own redaction,
//! persistence and audit policy.
//!
//! # Compatibility
//!
//! The trait and supporting types are frozen for Phase 0. New capabilities
//! should be added through new enum variants or a new type, not by changing
//! the meaning of an existing method.

#![forbid(unsafe_code)]

use std::fmt;
use std::fmt::Display;

use serde_json::Value;

/// Whether executing a tool requires explicit user approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalRequirement {
    /// Read-only or otherwise safe; no approval prompt is required.
    NotRequired,
    /// The user must approve each invocation. `reason` is shown verbatim and
    /// must not contain secrets.
    Required {
        /// Short, non-sensitive explanation for the approval prompt.
        reason: String,
    },
}

impl ApprovalRequirement {
    /// Builds a required-approval marker with a human-readable reason.
    pub fn required(reason: impl Into<String>) -> Self {
        Self::Required {
            reason: reason.into(),
        }
    }

    /// Whether an approval prompt must be shown before execution.
    #[must_use]
    pub fn is_required(&self) -> bool {
        matches!(self, Self::Required { .. })
    }
}

/// A tool's JSON input schema and optional output schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSchema {
    /// JSON Schema for the `input` value accepted by [`Tool::execute`].
    pub input: Value,
    /// JSON Schema for the successful output content, when known.
    pub output: Option<Value>,
}

impl ToolSchema {
    /// Creates an input-only schema.
    pub fn input(input: Value) -> Self {
        Self {
            input,
            output: None,
        }
    }

    /// Creates a schema for both input and output.
    pub fn new(input: Value, output: Value) -> Self {
        Self {
            input,
            output: Some(output),
        }
    }
}

/// A successful tool result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    /// Structured result value.
    pub content: Value,
    /// Short, display-safe summary for status lines and logs.
    pub summary: String,
}

impl ToolOutput {
    /// Creates a result with a safe summary.
    pub fn new(content: Value, summary: impl Into<String>) -> Self {
        Self {
            content,
            summary: summary.into(),
        }
    }
}

/// A tool invocation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    message: String,
}

impl ToolError {
    /// Creates an error with a non-sensitive message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The display-safe failure message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Display for ToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ToolError {}

/// A named, schema-described operation.
///
/// Implementations must be independent of the launcher's renderer and must not
/// perform UI work on the calling thread. Approval is a precondition reported
/// by the tool; the caller is responsible for enforcing it.
pub trait Tool: Send + Sync {
    /// Stable tool name. Use a namespaced value such as `mcp:github.search`.
    fn name(&self) -> &str;

    /// Human-readable description of what the tool does.
    fn description(&self) -> &str {
        ""
    }

    /// JSON schemas for input and, when known, output.
    fn schema(&self) -> &ToolSchema;

    /// Whether an approval prompt is required before execution.
    fn approval(&self) -> ApprovalRequirement;

    /// Executes the tool with a JSON input value.
    fn execute(&self, input: &Value) -> Result<ToolOutput, ToolError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Echo {
        schema: ToolSchema,
    }

    impl Tool for Echo {
        fn name(&self) -> &str {
            "example:echo"
        }

        fn description(&self) -> &str {
            "Returns the input value"
        }

        fn schema(&self) -> &ToolSchema {
            &self.schema
        }

        fn approval(&self) -> ApprovalRequirement {
            ApprovalRequirement::NotRequired
        }

        fn execute(&self, input: &Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::new(input.clone(), "echoed input"))
        }
    }

    #[test]
    fn tool_contract_round_trips_schema_approval_and_output() {
        let tool = Echo {
            schema: ToolSchema::input(json!({
                "type": "object",
                "properties": { "value": { "type": "string" } }
            })),
        };
        assert_eq!(tool.name(), "example:echo");
        assert!(!tool.approval().is_required());
        assert_eq!(tool.schema().input["type"], "object");

        let output = tool.execute(&json!({ "value": "hello" })).unwrap();
        assert_eq!(output.content["value"], "hello");
        assert_eq!(output.summary, "echoed input");
    }

    #[test]
    fn approval_requirement_is_explicit() {
        assert!(!ApprovalRequirement::NotRequired.is_required());
        let required = ApprovalRequirement::required("Writes to a remote service");
        assert!(required.is_required());
        assert_eq!(
            required,
            ApprovalRequirement::Required {
                reason: "Writes to a remote service".into()
            }
        );
    }

    #[test]
    fn errors_are_display_safe() {
        let error = ToolError::new("remote service unavailable");
        assert_eq!(error.message(), "remote service unavailable");
        assert_eq!(error.to_string(), "remote service unavailable");
    }
}
