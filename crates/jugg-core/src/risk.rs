use serde::{Deserialize, Serialize};

/// How dangerous a tool call is. The agent loop uses this to decide whether
/// a call runs immediately or waits for the owner's approval in the chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// Reads state only.
    Read,
    /// Creates drafts / files inside the job workspace.
    Create,
    /// Changes existing settings or content (still unpublished).
    Modify,
    /// Makes something public: publish site, send email, post to social.
    Publish,
    /// Deletes or overwrites data irreversibly.
    Destructive,
}

impl Risk {
    /// Default policy: anything that goes public or destroys data needs a human.
    pub fn needs_approval(self) -> bool {
        matches!(self, Risk::Publish | Risk::Destructive)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_policy() {
        assert!(!Risk::Create.needs_approval());
        assert!(!Risk::Modify.needs_approval());
        assert!(Risk::Publish.needs_approval());
        assert!(Risk::Destructive.needs_approval());
    }
}
