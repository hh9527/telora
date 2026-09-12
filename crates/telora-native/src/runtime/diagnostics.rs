use super::*;
use crate::abi::NativeDiagnostic;

impl Runtime {
    pub fn register_source_names(&mut self, sources: &telora_core::source::SourceDatabase) {
        self.source_names.extend(sources.files().map(|file| (file.id().get(), file.name.to_string())));
    }
    pub(super) fn diagnostic_field_type(&self, ty: TypeId, name: &str) -> Result<TypeId> {
        let layout = self.layout(ty)?;
        let index = layout.field_names.iter().position(|field| field == name).ok_or_else(|| format!("diagnostic contract lacks {name}"))?;
        Ok(layout.fields[index].0)
    }
    fn diagnostic_record(&mut self, ty: TypeId, fields: Vec<(&str, Value)>) -> Result<Value> {
        let values = self.layout(ty)?.field_names.iter().map(|name| fields.iter().find(|(field, _)| *field == name).map(|(_, value)| value.clone()).ok_or("diagnostic field value missing".into())).collect::<Result<Vec<_>>>()?;
        self.aggregate(ty, [0; 3], &values)
    }
    pub(super) fn diagnostic_snapshot(&mut self, ty: TypeId, diagnostic: &NativeDiagnostic) -> Result<Value> {
        let severity_ty = self.diagnostic_field_type(ty, "severity")?;
        let message_ty = self.diagnostic_field_type(ty, "message")?;
        let labels_ty = self.diagnostic_field_type(ty, "labels")?;
        let notes_ty = self.diagnostic_field_type(ty, "notes")?;
        let label_ty = self.layout(labels_ty)?.arguments[0];
        let range_ty = self.diagnostic_field_type(label_ty, "location")?;
        let source_ty = self.diagnostic_field_type(range_ty, "source")?;
        let int_ty = self.diagnostic_field_type(range_ty, "start")?;
        let bool_ty = self.diagnostic_field_type(label_ty, "primary")?;
        let mut labels = vec![];
        let origins = std::iter::once((diagnostic.origin, diagnostic.message.clone(), true))
            .chain(diagnostic.subjects.iter().enumerate().filter(|(_, origin)| **origin != diagnostic.origin)
                .map(|(index, origin)| (*origin, format!("subject {} originated here", index + 1), false)));
        for (origin, message, primary) in origins {
            let [source, start, end] = origin.words();
            if source == 0 { continue; }
            let name = self.source_names.get(&source).cloned().unwrap_or_else(|| format!("source:{source}"));
            let source = self.string(source_ty, [0; 3], &name)?;
            let start = self.scalar(int_ty, [0; 3], u64::from(start))?;
            let end = self.scalar(int_ty, [0; 3], u64::from(end))?;
            let location = self.diagnostic_record(range_ty, vec![("source", source), ("start", start), ("end", end)])?;
            let message = self.string(message_ty, [0; 3], &message)?;
            let primary = self.scalar(bool_ty, [0; 3], u64::from(primary))?;
            labels.push(self.diagnostic_record(label_ty, vec![("location", location), ("message", message), ("primary", primary)])?);
        }
        let severity = match diagnostic.severity { telora_core::source::Severity::Error => "Error", telora_core::source::Severity::Warning => "Warning", telora_core::source::Severity::Info => "Info" };
        let severity = self.named_variant(severity_ty, [0; 3], severity, None)?;
        let message = self.string(message_ty, [0; 3], &diagnostic.message)?;
        let labels = self.array(labels_ty, [0; 3], &labels)?;
        let notes = self.array(notes_ty, [0; 3], &[])?;
        self.diagnostic_record(ty, vec![("severity", severity), ("message", message), ("labels", labels), ("notes", notes)])
    }
}
