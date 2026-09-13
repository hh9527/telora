use crate::emit::Emitter;
use telora_core::mir::TypeId;
use wasm_encoder::ValType;

impl Emitter<'_> {
    pub(crate) fn schema_enum(
        &mut self,
        source: TypeId,
        target: TypeId,
        rename: bool,
        untagged: bool,
    ) -> Result<u32, String> {
        let variants: Vec<_> = self.plan.layouts[source.index()]
            .variants
            .iter()
            .map(|v| (v.name.clone(), v.type_id))
            .collect();
        if untagged && variants.iter().filter(|(_, ty)| ty.is_none()).count() > 1 {
            self.codec_error(1, "$: untagged Enum may contain at most one unit variant")?;
            return Ok(self.local(ValType::I32));
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut choices = Vec::new();
        for (name, ty) in variants {
            let name = crate::codec_names::external_names([name], rename)?.remove(0);
            if !untagged && !seen.insert(name.clone()) {
                self.codec_error(1, &format!("$.{name}: duplicate external variant name"))?;
                return Ok(self.local(ValType::I32));
            }
            let value = match (untagged, ty) {
                (true, Some(ty)) => self.schema_call(self.plan.layouts[ty].id(), target, 0, 1)?,
                (true, None) => self.schema_kind(target, "null")?,
                (false, None) => {
                    let name = self.schema_string(target, name.as_bytes(), 1)?;
                    self.schema_object(target, vec![("const".into(), name)], 1)?
                }
                (false, Some(ty)) => {
                    let kind = self.schema_string(target, b"object", 1)?;
                    let child = self.schema_call(self.plan.layouts[ty].id(), target, 0, 1)?;
                    let properties = self.schema_object(target, vec![(name.clone(), child)], 1)?;
                    let name = self.schema_string(target, name.as_bytes(), 1)?;
                    let required = self.schema_array(target, &[name], 1)?;
                    let no = self.codec_variant(target, "False", None, 1)?;
                    self.schema_object(
                        target,
                        vec![
                            ("type".into(), kind),
                            ("properties".into(), properties),
                            ("required".into(), required),
                            ("additionalProperties".into(), no),
                        ],
                        1,
                    )?
                }
            };
            choices.push(value);
        }
        let choices = self.schema_array(target, &choices, 1)?;
        self.schema_object(target, vec![("oneOf".into(), choices)], 1)
    }
}
