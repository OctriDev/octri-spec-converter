//! A bounded diagnostic sink keeps warning floods from hiding errors or growing
//! without limit. Exhaustion is itself an error, never a successful partial run.

use crate::{Address, Diagnostic, ParserError, Severity};

pub(crate) struct Diagnostics {
    items: Vec<Diagnostic>,
    limit: usize,
}
impl Diagnostics {
    pub fn new(limit: usize) -> Self {
        Self {
            items: Vec::new(),
            limit,
        }
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn full(&self) -> bool {
        self.items.len() >= self.limit
    }
    pub fn push(&mut self, diagnostic: Diagnostic) {
        append(&mut self.items, self.limit, diagnostic);
    }
    pub fn extend(&mut self, diagnostics: impl IntoIterator<Item = Diagnostic>) {
        for diagnostic in diagnostics {
            self.push(diagnostic);
        }
    }
    pub fn finish(mut self) -> Vec<Diagnostic> {
        if self.full()
            && let Some(last) = self.items.last_mut()
        {
            *last = limit_diagnostic(last.clone());
        }
        self.items
    }
}

pub(crate) fn append(items: &mut Vec<Diagnostic>, limit: usize, diagnostic: Diagnostic) {
    if items.len() < limit {
        items.push(diagnostic);
    } else if let Some(last) = items.last_mut() {
        *last = limit_diagnostic(diagnostic);
    }
}

pub(crate) fn exhausted(at: Address) -> ParserError {
    ParserError::new(
        "LIMIT_DIAGNOSTICS",
        "Diagnostic budget exhausted; validation is incomplete",
        at,
    )
}

fn limit_diagnostic(diagnostic: Diagnostic) -> Diagnostic {
    Diagnostic {
        code: "LIMIT_DIAGNOSTICS".into(),
        severity: Severity::Error,
        message: "Diagnostic budget exhausted; validation is incomplete".into(),
        location: diagnostic.location,
        related: Vec::new(),
    }
}
