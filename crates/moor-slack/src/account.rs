//! What a person can link with `/moor login`. Each account is one secret in the controller's
//! vault, under the name the controller's Proxy rules inject it by.

use moor_core::config::proxy::GITHUB_SECRET;
use moor_core::user::SecretName;

/// An account whose secret a person can link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Account {
    /// Claude Code, with the token `claude setup-token` prints.
    Claude,
    /// GitHub, with the token `gh auth token` prints.
    GitHub,
}

impl Account {
    /// The account a command means when it names none.
    pub(crate) const DEFAULT: Self = Self::Claude;

    /// Every account a person can link.
    const ALL: [Self; 2] = [Self::Claude, Self::GitHub];

    /// The account's command word, which is also the name of its secret.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::GitHub => GITHUB_SECRET,
        }
    }

    /// The name the vault keeps the account's secret under.
    pub(crate) fn secret(self) -> Result<SecretName, String> {
        SecretName::parse(self.name())
    }

    /// The account's name in a sentence.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::GitHub => "GitHub",
        }
    }

    /// What the login modal asks this account's user to paste.
    pub(crate) const fn login_prompt(self) -> &'static str {
        match self {
            Self::Claude => crate::coordinator::claude::LOGIN_PROMPT,
            Self::GitHub => "Paste the output of `gh auth token` from your machine.",
        }
    }

    /// The account called `word`.
    pub(crate) fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|account| account.name() == word)
    }
}

#[cfg(test)]
mod tests {
    use super::Account;

    #[test]
    fn each_account_is_one_word_and_names_its_secret() {
        for (word, account) in [("claude", Account::Claude), ("github", Account::GitHub)] {
            assert_eq!(Account::parse(word), Some(account));
            assert_eq!(account.secret().unwrap().as_str(), word);
        }
        assert_eq!(Account::parse("codex"), None);
        assert_eq!(Account::parse("GitHub"), None);
        assert_eq!(Account::DEFAULT, Account::Claude);
    }
}
