#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeErrorKind {
    Cancelled,
    FuelExhausted,
    AllocationQuotaExceeded,
    CallDepthExceeded,
    DivisionByZero,
    IntegerOverflow,
    InvalidBytecode,
    MissingField,
    NoPatternMatched,
    Panic,
    ReportedDiagnostic,
    RaisedBlame,
    StackLimitExceeded,
    TypeMismatch,
    UninitializedDefinition,
    DuplicateDefinition,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeError {
    pub kind: RuntimeErrorKind,
    pub message: String,
    pub function: String,
    pub instruction: usize,
    pub trace: Vec<RuntimeFrame>,
    locations: Option<Box<RuntimeLocations>>,
    rendered: Option<Box<str>>,
    trace_includes_active_frame: bool,
    propagated_failure: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RuntimeLocations {
    data_sources: Vec<crate::Loc>,
    rule: Option<crate::Loc>,
    implementation_rule: Option<crate::Loc>,
    rule_primary: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeFrame {
    pub function: String,
    pub instruction: usize,
    pub origin: Option<Origin>,
}

impl RuntimeError {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) const fn failure_class(&self) -> crate::evaluation::FailureClass {
        use crate::evaluation::FailureClass;
        match self.kind {
            RuntimeErrorKind::DivisionByZero
            | RuntimeErrorKind::IntegerOverflow
            | RuntimeErrorKind::MissingField
            | RuntimeErrorKind::NoPatternMatched
            | RuntimeErrorKind::Panic
            | RuntimeErrorKind::ReportedDiagnostic
            | RuntimeErrorKind::RaisedBlame
            | RuntimeErrorKind::TypeMismatch
            | RuntimeErrorKind::UninitializedDefinition
            | RuntimeErrorKind::DuplicateDefinition => FailureClass::Recoverable,
            RuntimeErrorKind::Cancelled
            | RuntimeErrorKind::FuelExhausted
            | RuntimeErrorKind::AllocationQuotaExceeded
            | RuntimeErrorKind::CallDepthExceeded
            | RuntimeErrorKind::InvalidBytecode
            | RuntimeErrorKind::StackLimitExceeded => FailureClass::Terminal,
        }
    }

    pub(crate) fn from_heap_error(
        function: &BytecodeFunction,
        heap_error: crate::heap::HeapError,
    ) -> Self {
        error(
            RuntimeErrorKind::InvalidBytecode,
            heap_error.to_string(),
            function,
            0,
        )
    }

    pub fn origin(&self) -> Option<Origin> {
        self.trace.first().and_then(|frame| frame.origin)
    }

    pub fn data_location(&self) -> Option<crate::Loc> {
        self.data_sources().first().copied()
    }

    pub fn data_sources(&self) -> &[crate::Loc] {
        self.locations
            .as_deref()
            .map_or(&[], |locations| locations.data_sources.as_slice())
    }

    pub fn rule_location(&self) -> Option<crate::Loc> {
        self.locations
            .as_deref()
            .and_then(|locations| locations.rule)
    }

    pub fn implementation_rule_location(&self) -> Option<crate::Loc> {
        self.locations
            .as_deref()
            .and_then(|locations| locations.implementation_rule)
    }

    pub(crate) const fn propagated_failure(&self) -> Option<u32> {
        self.propagated_failure
    }

    fn set_locations(&mut self, data: Option<crate::Loc>, rule: Option<crate::Loc>) {
        self.set_data_sources(data, rule);
    }

    fn set_data_sources(
        &mut self,
        data_sources: impl IntoIterator<Item = crate::Loc>,
        rule: Option<crate::Loc>,
    ) {
        let mut unique = Vec::new();
        for location in data_sources {
            if !unique.contains(&location) {
                unique.push(location);
            }
        }
        self.locations = (!unique.is_empty() || rule.is_some()).then(|| {
            Box::new(RuntimeLocations {
                data_sources: unique,
                rule,
                implementation_rule: None,
                rule_primary: false,
            })
        });
    }

    fn set_contextual_locations(
        &mut self,
        data_sources: impl IntoIterator<Item = crate::Loc>,
        rule: Option<crate::Loc>,
        implementation_rule: Option<crate::Loc>,
    ) {
        self.set_data_sources(data_sources, rule);
        let locations = self.locations.get_or_insert_with(|| {
            Box::new(RuntimeLocations {
                data_sources: Vec::new(),
                rule: None,
                implementation_rule: None,
                rule_primary: true,
            })
        });
        locations.implementation_rule = implementation_rule.filter(|origin| Some(*origin) != rule);
        locations.rule_primary = true;
    }

    fn set_data_location(&mut self, data: Option<crate::Loc>) {
        self.set_data_sources(data, self.rule_location());
    }

    pub(crate) fn diagnostic(&self) -> Option<Diagnostic> {
        if self.propagated_failure.is_some() {
            return None;
        }
        let operation_location = self.origin().and_then(|origin| match origin {
            Origin::Source(location) => Some(location),
            Origin::Synthetic { derived_from } => derived_from,
        });
        let rule_location = self.rule_location().or(operation_location);
        let locations = self.locations.as_deref();
        if locations.is_some_and(|locations| locations.rule_primary) {
            let primary = rule_location.or_else(|| self.data_location())?;
            let mut diagnostic = Diagnostic::error(self.message.clone(), primary);
            for (index, source) in self.data_sources().iter().copied().enumerate() {
                if source != primary {
                    diagnostic = diagnostic
                        .with_secondary(format!("subject {} originated here", index + 1), source);
                }
            }
            if let Some(implementation) = self.implementation_rule_location()
                && implementation != primary
                && !self.data_sources().contains(&implementation)
            {
                diagnostic = diagnostic.with_secondary("failure raised here", implementation);
            }
            Some(diagnostic)
        } else {
            let secondary_message = if self.rule_location().is_some() {
                "contract rule declared here"
            } else {
                "operation originated here"
            };
            match (self.data_location(), rule_location) {
                (Some(data), Some(rule)) if data != rule => Some(
                    Diagnostic::error(self.message.clone(), data)
                        .with_secondary(secondary_message, rule),
                ),
                (Some(data), _) => Some(Diagnostic::error(self.message.clone(), data)),
                (None, Some(rule)) => Some(Diagnostic::error(self.message.clone(), rule)),
                (None, None) => None,
            }
        }
    }

    pub fn with_sources(mut self, sources: &SourceDatabase) -> Self {
        if let Some(diagnostic) = self.diagnostic() {
            self.rendered = Some(sources.render(&diagnostic).into_boxed_str());
        }
        self
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(rendered) = &self.rendered {
            return formatter.write_str(rendered);
        }
        write!(
            formatter,
            "{} at {}:{}",
            self.message, self.function, self.instruction
        )
    }
}

impl std::error::Error for RuntimeError {}

fn fail_on_reported_error(
    account: &QuotaAccount,
    start: usize,
    function: &BytecodeFunction,
) -> Result<(), RuntimeError> {
    let Some(diagnostic) = account.diagnostics[start..]
        .iter()
        .find(|diagnostic| diagnostic.severity == crate::source::Severity::Error)
    else {
        return Ok(());
    };
    let mut runtime = error(
        RuntimeErrorKind::ReportedDiagnostic,
        diagnostic.message.clone(),
        function,
        0,
    );
    let primary = diagnostic
        .labels
        .iter()
        .find(|label| label.primary)
        .map(|label| label.location);
    let rule = diagnostic
        .labels
        .iter()
        .rev()
        .find(|label| !label.primary)
        .map(|label| label.location);
    runtime.set_locations(primary, rule);
    Err(runtime)
}

