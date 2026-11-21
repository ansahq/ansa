//! Launch testing against real catalog command identities.
//!
//! The onboarding crate does not discover or launch applications. A host
//! adapter implements [`LaunchProbe`] and reports whether a launch actually
//! succeeded. Onboarding records only what the adapter verified.

use std::fmt;

use ansa_domain::CommandId;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The result of attempting to launch a catalog command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchOutcome {
    /// The host confirmed that the command launched.
    Launched,
    /// The host attempted the launch and it failed.
    Failed { reason: String },
    /// The host issued the launch but could not confirm the result.
    Unverified,
}

impl LaunchOutcome {
    /// True only for [`LaunchOutcome::Launched`].
    pub fn is_launched(&self) -> bool {
        matches!(self, Self::Launched)
    }

    /// A short label suitable for status rows.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Launched => "Launched",
            Self::Failed { .. } => "Failed",
            Self::Unverified => "Unverified",
        }
    }

    /// Plain-language explanation for non-success outcomes.
    pub fn explanation(&self) -> Option<String> {
        match self {
            Self::Launched => None,
            Self::Failed { reason } => {
                Some(format!("The test application did not launch: {reason}"))
            }
            Self::Unverified => Some(
                "The launch was requested, but the system did not confirm that it started"
                    .to_owned(),
            ),
        }
    }
}

impl fmt::Display for LaunchOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// The catalog command whose launch onboarding actually verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchVerification {
    command_id: CommandId,
}

impl LaunchVerification {
    /// Records a verified launch. Callers must only construct this after a
    /// [`LaunchOutcome::Launched`] result.
    pub fn new(command_id: CommandId) -> Self {
        Self { command_id }
    }

    /// The identity that was launched.
    pub fn command_id(&self) -> &CommandId {
        &self.command_id
    }
}

impl Serialize for LaunchVerification {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.command_id.as_str())
    }
}

impl<'de> Deserialize<'de> for LaunchVerification {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct VerificationVisitor;

        impl Visitor<'_> for VerificationVisitor {
            type Value = LaunchVerification;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a canonical command identity such as app:demo")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let command_id = CommandId::parse(value).map_err(de::Error::custom)?;
                Ok(LaunchVerification::new(command_id))
            }
        }

        deserializer.deserialize_str(VerificationVisitor)
    }
}

/// Host adapter that attempts to launch a catalog command and reports the
/// outcome truthfully.
pub trait LaunchProbe {
    /// Attempts the launch. Returning [`LaunchOutcome::Launched`] is a claim
    /// that the host observed a successful start, not merely a submitted
    /// request.
    fn launch(&self, command_id: &CommandId) -> LaunchOutcome;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verification_serde_round_trips_a_command_identity() {
        let verification = LaunchVerification::new(CommandId::parse("app:demo").unwrap());
        let json = serde_json::to_string(&verification).unwrap();
        assert_eq!(json, "\"app:demo\"");
        let parsed: LaunchVerification = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, verification);
        assert_eq!(parsed.command_id().as_str(), "app:demo");
    }

    #[test]
    fn invalid_verification_identity_is_rejected() {
        assert!(serde_json::from_str::<LaunchVerification>("\"demo\"").is_err());
    }

    #[test]
    fn outcomes_are_honest_about_success() {
        assert!(LaunchOutcome::Launched.is_launched());
        assert!(!LaunchOutcome::Unverified.is_launched());
        assert!(
            !LaunchOutcome::Failed {
                reason: "missing".to_owned()
            }
            .is_launched()
        );
        assert!(
            LaunchOutcome::Failed {
                reason: "missing".to_owned()
            }
            .explanation()
            .unwrap()
            .contains("missing")
        );
    }
}
