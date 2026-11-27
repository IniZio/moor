//! Why moor refuses a hand-in. Each names the phrase by its quote or the effect by its label,
//! so the reader can fix that one and hand in again.

use crate::gate::GateError;

/// Why a hand-in is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// The reading is not waiting for a hand-in.
    #[error("this reading is not waiting for a hand-in")]
    NotReading,
    /// A quote with no words.
    #[error("quote `{quote}` has no words")]
    EmptyQuote {
        /// The quote.
        quote: String,
    },
    /// A quote found nowhere, even after folding white space, normal form and curly quotes.
    #[error("quote `{quote}` is not in the prompt; copy the prompt's own words")]
    NotFound {
        /// The quote.
        quote: String,
    },
    /// A quote found in more than one place.
    #[error(
        "quote `{quote}` is in the prompt {found} times; add a prefix or a suffix that picks one"
    )]
    Ambiguous {
        /// The quote.
        quote: String,
        /// How many places.
        found: usize,
    },
    /// Two phrases share words.
    #[error("phrases `{first}` and `{second}` overlap; make them one phrase or cut them apart")]
    Overlap {
        /// The phrase that starts first.
        first: String,
        /// The one after.
        second: String,
    },
    /// Two effects share a label.
    #[error("effect label `{label}` is used twice")]
    DuplicateLabel {
        /// The label.
        label: String,
    },
    /// Two effects hold the same thing.
    #[error("effect `{label}` holds what an earlier effect holds")]
    DuplicateKey {
        /// The later effect's label.
        label: String,
    },
    /// A held phrase names a label no effect has.
    #[error("phrase `{quote}` names effect `{label}`, which is not handed in")]
    UnknownEffect {
        /// The phrase's quote.
        quote: String,
        /// The label.
        label: String,
    },
    /// A held phrase names one effect twice.
    #[error("phrase `{quote}` names effect `{label}` twice")]
    RepeatedEffect {
        /// The phrase's quote.
        quote: String,
        /// The label.
        label: String,
    },
    /// An effect no phrase holds.
    #[error("effect `{label}` is held by no phrase")]
    Unheld {
        /// The label.
        label: String,
    },
    /// An outcome with no gate to end.
    #[error("outcome `{label}` needs at least one gate")]
    OutcomeWithoutGate {
        /// The label.
        label: String,
    },
    /// The gates and outcomes do not parse.
    #[error(transparent)]
    Gates(GateError),
    /// An intent name that is not lower case words joined by `-`.
    #[error("phrase `{quote}` names intent `{intent}`; write it as lower case words joined by -")]
    IntentName {
        /// The phrase's quote.
        quote: String,
        /// The name.
        intent: String,
    },
    /// A repo the Box does not have.
    #[error("phrase `{quote}` names repo `{repo}`, which is not one of this automation's repos")]
    UnknownRepo {
        /// The phrase's quote.
        quote: String,
        /// The repo.
        repo: String,
    },
    /// `as: app` on a row whose lines may not go out as the App.
    #[error("phrase `{quote}`: intent `{intent}` cannot go out as the App; drop `as`")]
    NoAppRow {
        /// The phrase's quote.
        quote: String,
        /// The intent.
        intent: String,
    },
}
