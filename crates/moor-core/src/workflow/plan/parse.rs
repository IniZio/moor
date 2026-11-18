//! The checks that turn the wire form of a plan into a [`Plan`].
//!
//! The wire form is lenient on purpose: every field is plain text or a number, so a plan with
//! five mistakes reports five errors at once and a model fixes them in one pass.

use std::collections::{HashMap, HashSet};

use super::doc::{doc_of, PlanDoc, PlanError, StepDoc};
use super::{Evidence, EvidenceName, Next, Plan, PromptHash, Step, StepId, StepKind, Tries};

/// Most steps a plan has.
const MAX_STEPS: usize = 20;
/// Most bytes of a step's title.
const MAX_TITLE: usize = 120;
/// Most bytes of a step's instructions, done-when or command.
const MAX_TEXT: usize = 4096;

impl Plan {
    /// Parses the JSON of a [`PlanDoc`] made for `prompt`, checking every rule and returning
    /// every error found at once. A plan that does not say what prompt it was made from is made
    /// from `prompt`.
    ///
    /// # Errors
    /// Every [`PlanError`] the plan has; one [`PlanError::Json`] when the text is not a plan.
    pub fn parse(text: &str, prompt: &str) -> Result<Self, Vec<PlanError>> {
        let json = |e: String| vec![PlanError::Json(e)];
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| json(e.to_string()))?;
        if !value.is_object() {
            return Err(json(
                "a plan is an object with a `steps` list, like {\"steps\": [...]}".to_owned(),
            ));
        }
        let doc: PlanDoc = serde_json::from_value(value).map_err(|e| json(e.to_string()))?;
        Self::from_doc(&doc, prompt)
    }

    /// The plan `doc` describes, for a plan made for `prompt`.
    ///
    /// # Errors
    /// Every [`PlanError`] the plan has.
    pub fn from_doc(doc: &PlanDoc, prompt: &str) -> Result<Self, Vec<PlanError>> {
        let mut errors = Vec::new();
        let made_from = doc.made_from.as_deref().map_or_else(
            || PromptHash::of(prompt),
            |hex| {
                PromptHash::from_hex(hex).unwrap_or_else(|| {
                    errors.push(PlanError::MadeFrom);
                    PromptHash::of(prompt)
                })
            },
        );
        let ids: HashSet<&str> = doc.steps.iter().map(|s| s.id.as_str()).collect();
        match doc.steps.len() {
            0 => errors.push(PlanError::NoSteps),
            n if n > MAX_STEPS => errors.push(PlanError::TooManySteps(n)),
            _ => {}
        }
        let mut seen = HashSet::new();
        let mut steps = Vec::new();
        for (i, s) in doc.steps.iter().enumerate() {
            if !seen.insert(s.id.as_str()) {
                errors.push(PlanError::DuplicateId(s.id.clone()));
            }
            steps.extend(step_of(s, i, &ids, &mut errors));
        }
        if !doc.steps.is_empty() && doc.steps.iter().all(|s| s.kind != "deliver") {
            errors.push(PlanError::NoDeliver);
        }
        if errors.is_empty() {
            errors.extend(trapped(&steps));
        }
        let mut steps = steps.into_iter();
        match (errors.is_empty(), steps.next()) {
            (true, Some(first)) => Ok(Self {
                first,
                rest: steps.collect(),
                made_from,
                edited: doc.edited,
            }),
            _ => Err(errors),
        }
    }

    /// The plan as written, with the hash it was made from.
    #[must_use]
    pub fn to_doc(&self) -> PlanDoc {
        PlanDoc {
            steps: self.steps().map(doc_of).collect(),
            made_from: Some(self.made_from.hex()),
            edited: self.edited,
        }
    }

    /// The plan as the JSON the store keeps.
    ///
    /// # Errors
    /// A serialization failure, which a plan cannot cause.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(&self.to_doc())
    }
}

/// `value` unless it is missing or blank.
fn need(value: Option<&String>) -> Option<&String> {
    value.filter(|v| !v.trim().is_empty())
}

/// The step `s` at place `i`, or the errors it has.
fn step_of(
    s: &StepDoc,
    i: usize,
    ids: &HashSet<&str>,
    errors: &mut Vec<PlanError>,
) -> Option<Step> {
    let before = errors.len();
    let place = if StepId::parse(&s.id).is_ok() {
        format!("step `{}`", s.id)
    } else {
        errors.push(PlanError::BadId {
            step: format!("step {} (`{}`)", i + 1, s.id),
        });
        format!("step {}", i + 1)
    };
    if s.title.trim().is_empty() || s.title.len() > MAX_TITLE {
        errors.push(PlanError::Title {
            step: place.clone(),
            len: s.title.len(),
        });
    }
    let tries = s.max_tries.map_or(Some(Tries::ONE), |n| {
        let tries = u8::try_from(n).ok().and_then(Tries::new);
        if tries.is_none() {
            errors.push(PlanError::MaxTries {
                step: place.clone(),
                got: n,
            });
        }
        tries
    });
    let ctx = Ctx { place: &place, ids };
    let kind = match s.kind.as_str() {
        "work" => work(s, &ctx, errors),
        "check" => check(s, &ctx, errors),
        "approve" => approve(s, &ctx, errors),
        "deliver" => deliver(s, &ctx, errors),
        other => {
            errors.push(PlanError::UnknownKind {
                step: place.clone(),
                kind: other.to_owned(),
            });
            None
        }
    };
    if errors.len() > before {
        return None;
    }
    Some(Step {
        id: StepId::parse(&s.id).ok()?,
        title: s.title.clone(),
        max_tries: tries?,
        kind: kind?,
    })
}

/// What a kind's check needs to say where an error is.
struct Ctx<'a> {
    /// The step's place, for messages.
    place: &'a str,
    /// Every id the plan writes.
    ids: &'a HashSet<&'a str>,
}

impl Ctx<'_> {
    /// An error for a field `kind` needs.
    fn missing(&self, kind: &'static str, field: &'static str) -> PlanError {
        PlanError::Missing {
            step: self.place.to_owned(),
            kind,
            field,
        }
    }

    /// Reports each of `fields` that is set though `kind` takes none of it.
    fn unexpected(
        &self,
        kind: &'static str,
        fields: &[(&'static str, bool)],
        errors: &mut Vec<PlanError>,
    ) {
        for (field, set) in fields {
            if *set {
                errors.push(PlanError::Unexpected {
                    step: self.place.to_owned(),
                    kind,
                    field,
                });
            }
        }
    }

    /// A required text of at most 4 KiB.
    fn text(
        &self,
        (kind, field): (&'static str, &'static str),
        value: Option<&String>,
        errors: &mut Vec<PlanError>,
    ) -> Option<String> {
        let Some(v) = need(value) else {
            errors.push(self.missing(kind, field));
            return None;
        };
        if v.len() > MAX_TEXT {
            errors.push(PlanError::TooLong {
                step: self.place.to_owned(),
                field,
                len: v.len(),
            });
            return None;
        }
        Some(v.clone())
    }

    /// The `ok` and `fail` edges.
    fn next(&self, kind: &'static str, s: &StepDoc, errors: &mut Vec<PlanError>) -> Option<Next> {
        let edge = |edge: &'static str, target: &str, errors: &mut Vec<PlanError>| {
            let id = StepId::parse(target)
                .ok()
                .filter(|_| self.ids.contains(target));
            if id.is_none() {
                errors.push(PlanError::UnknownTarget {
                    step: self.place.to_owned(),
                    edge,
                    target: target.to_owned(),
                });
            }
            id
        };
        let ok = if let Some(t) = &s.ok {
            edge("ok", t, errors)
        } else {
            errors.push(self.missing(kind, "ok"));
            None
        };
        let fail = s.fail.as_ref().map(|t| edge("fail", t, errors));
        Some(Next {
            ok: ok?,
            fail: match fail {
                None => None,
                Some(id) => Some(id?),
            },
        })
    }
}

/// A `work` step.
fn work(s: &StepDoc, c: &Ctx<'_>, errors: &mut Vec<PlanError>) -> Option<StepKind> {
    c.unexpected(
        "work",
        &[
            ("command", s.command.is_some()),
            ("question", s.question.is_some()),
        ],
        errors,
    );
    let instructions = c.text(("work", "instructions"), s.instructions.as_ref(), errors);
    let done_when = c.text(("work", "done_when"), s.done_when.as_ref(), errors);
    let evidence = evidence_of(s, c, errors);
    let next = c.next("work", s, errors);
    Some(StepKind::Work {
        instructions: instructions?,
        done_when: done_when?,
        evidence: evidence?,
        next: next?,
    })
}

/// A `check` step.
fn check(s: &StepDoc, c: &Ctx<'_>, errors: &mut Vec<PlanError>) -> Option<StepKind> {
    c.unexpected(
        "check",
        &[
            ("instructions", s.instructions.is_some()),
            ("done_when", s.done_when.is_some()),
            ("evidence", s.evidence.is_some()),
            ("question", s.question.is_some()),
        ],
        errors,
    );
    let command = c.text(("check", "command"), s.command.as_ref(), errors);
    let next = c.next("check", s, errors);
    Some(StepKind::Check {
        command: command?,
        next: next?,
    })
}

/// An `approve` step.
fn approve(s: &StepDoc, c: &Ctx<'_>, errors: &mut Vec<PlanError>) -> Option<StepKind> {
    c.unexpected(
        "approve",
        &[
            ("instructions", s.instructions.is_some()),
            ("done_when", s.done_when.is_some()),
            ("evidence", s.evidence.is_some()),
            ("command", s.command.is_some()),
        ],
        errors,
    );
    let question = c.text(("approve", "question"), s.question.as_ref(), errors);
    let next = c.next("approve", s, errors);
    Some(StepKind::Approve {
        question: question?,
        next: next?,
    })
}

/// A `deliver` step. It has no onward edge, which is why it ends the run.
fn deliver(s: &StepDoc, c: &Ctx<'_>, errors: &mut Vec<PlanError>) -> Option<StepKind> {
    c.unexpected(
        "deliver",
        &[
            ("evidence", s.evidence.is_some()),
            ("command", s.command.is_some()),
            ("question", s.question.is_some()),
            ("ok", s.ok.is_some()),
            ("fail", s.fail.is_some()),
        ],
        errors,
    );
    let instructions = c.text(("deliver", "instructions"), s.instructions.as_ref(), errors);
    let done_when = c.text(("deliver", "done_when"), s.done_when.as_ref(), errors);
    Some(StepKind::Deliver {
        instructions: instructions?,
        done_when: done_when?,
    })
}

/// A work step's evidence names, at least one.
fn evidence_of(s: &StepDoc, c: &Ctx<'_>, errors: &mut Vec<PlanError>) -> Option<Evidence> {
    let Some(names) = s.evidence.as_ref().filter(|n| !n.is_empty()) else {
        errors.push(c.missing("work", "evidence"));
        return None;
    };
    let mut parsed = Vec::new();
    for n in names {
        match EvidenceName::parse(n) {
            Some(name) => parsed.push(name),
            None => errors.push(PlanError::BadEvidence {
                step: c.place.to_owned(),
                name: n.clone(),
            }),
        }
    }
    if parsed.len() == names.len() {
        Evidence::new(parsed)
    } else {
        None
    }
}

/// The steps from which no deliver step can be reached.
fn trapped(steps: &[Step]) -> Vec<PlanError> {
    let delivers: Vec<&Step> = steps
        .iter()
        .filter(|s| matches!(s.kind, StepKind::Deliver { .. }))
        .collect();
    if delivers.is_empty() {
        return Vec::new();
    }
    let mut back: HashMap<&StepId, Vec<&StepId>> = HashMap::new();
    for s in steps {
        for to in s
            .kind
            .next()
            .into_iter()
            .flat_map(|n| [Some(&n.ok), n.fail.as_ref()])
            .flatten()
        {
            back.entry(to).or_default().push(&s.id);
        }
    }
    let mut reached: HashSet<&StepId> = delivers.iter().map(|s| &s.id).collect();
    let mut todo: Vec<&StepId> = reached.iter().copied().collect();
    while let Some(id) = todo.pop() {
        for from in back.get(id).into_iter().flatten() {
            if reached.insert(from) {
                todo.push(from);
            }
        }
    }
    steps
        .iter()
        .filter(|s| !reached.contains(&s.id))
        .map(|s| PlanError::Trapped {
            step: format!("step `{}`", s.id),
        })
        .collect()
}
