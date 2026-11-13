//! Who made a change: an analyst or an agent.
//!
//! The distinction is load-bearing, not cosmetic: some writes are refused to
//! an agent, and every history row names its actor by `label()`. The text
//! form is outside input (a CLI flag), so `parse` is total.

use crate::error::{EngineError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    Analyst(String),
    Agent(String),
}

/// The longest name an actor may carry.
const MAX_NAME: usize = 64;

impl Actor {
    /// `analyst:<name>` or `agent:<name>`, where the name is 1 to 64
    /// characters of `[A-Za-z0-9._-]`. Anything else is `Invalid`.
    pub fn parse(text: &str) -> Result<Actor> {
        let invalid = || {
            EngineError::Invalid(
                "an actor is analyst:<name> or agent:<name>, the name 1 to 64 \
                 characters of A-Z a-z 0-9 . _ -"
                    .to_string(),
            )
        };
        let (kind, name) = text.split_once(':').ok_or_else(invalid)?;
        let valid_name = (1..=MAX_NAME).contains(&name.len())
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        if !valid_name {
            return Err(invalid());
        }
        match kind {
            "analyst" => Ok(Actor::Analyst(name.to_string())),
            "agent" => Ok(Actor::Agent(name.to_string())),
            _ => Err(invalid()),
        }
    }

    /// The analyst an operating system user name stands for: its characters
    /// outside `[A-Za-z0-9._-]` dropped, the rest cut to 64, and `analyst`
    /// when nothing is left. Pure, and total: the result always passes
    /// `parse` as `label()`.
    pub fn from_os_user(raw: &str) -> Actor {
        let mut name: String = raw
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            .collect();
        // Only ASCII is left, so cutting at a byte count is cutting at a
        // character.
        name.truncate(MAX_NAME);
        if name.is_empty() {
            name.push_str("analyst");
        }
        Actor::Analyst(name)
    }

    /// The text `parse` accepts for this actor.
    pub fn label(&self) -> String {
        match self {
            Actor::Analyst(name) => format!("analyst:{name}"),
            Actor::Agent(name) => format!("agent:{name}"),
        }
    }

    pub fn is_agent(&self) -> bool {
        matches!(self, Actor::Agent(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_user_sanitiser() {
        for (raw, label) in [
            ("", "analyst:analyst"),
            ("!!!", "analyst:analyst"),
            ("..", "analyst:.."),
            ("a:b", "analyst:ab"),
            ("Jürgen", "analyst:Jrgen"),
            ("jane.doe", "analyst:jane.doe"),
        ] {
            let actor = Actor::from_os_user(raw);
            assert_eq!(actor.label(), label, "{raw:?}");
            assert_eq!(Actor::parse(&actor.label()).unwrap(), actor, "{raw:?}");
        }
        let actor = Actor::from_os_user(&"x".repeat(200));
        assert_eq!(actor.label().len(), "analyst:".len() + 64);
        assert_eq!(Actor::parse(&actor.label()).unwrap(), actor);
    }
}
