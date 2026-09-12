use super::*;

pub(super) struct TypeInfo {
    kind: Option<&'static str>,
    children: Vec<TypeId>,
    body: Option<TypeId>,
    opaque_name: Option<String>,
    members: Option<Vec<(String, Option<TypeId>)>>,
}

pub(super) fn build(image: &telora_core::type_image::TypeImage) -> Result<Vec<TypeInfo>> {
    image
        .types
        .iter()
        .enumerate()
        .map(|(index, ty)| {
            let kind = match &ty.constructor {
                T::Never => "Never",
                T::Type | T::Meta => "Type",
                T::TypeOf => "TypeOf",
                T::Int => "Int",
                T::Float => "Float",
                T::String => "String",
                T::Bytes => "Bytes",
                T::Array => "Array",
                T::Dict => "Dict",
                T::Tuple => "Tuple",
                T::Record(_) => "Struct",
                T::Newtype => "Newtype",
                T::Enum(_)
                | T::Bool
                | T::Option
                | T::Result
                | T::FoldControl
                | T::PropertyTarget => "Enum",
                T::Function => "Func",
                T::Native(_) => "Opaque",
                T::Parameter(_) | T::PropertyBound => "Bound",
                T::Dyn => "Dyn",
                T::Nominal(_) | T::Unchecked => "Ref",
                _ => "",
            };
            let children = if matches!(
                ty.constructor,
                T::TypeOf
                    | T::Array
                    | T::Dict
                    | T::Tuple
                    | T::Record(_)
                    | T::Newtype
                    | T::Enum(_)
                    | T::Option
                    | T::Result
                    | T::FoldControl
            ) {
                ty.arguments
                    .iter()
                    .copied()
                    .map(TypeId::try_from)
                    .collect::<Result<_>>()?
            } else {
                vec![]
            };
            let members = match &ty.constructor {
                T::Record(names) => Some(
                    names
                        .iter()
                        .cloned()
                        .zip(ty.arguments.iter().copied().map(Some))
                        .collect::<Vec<_>>(),
                ),
                T::Enum(names) => {
                    let mut args = ty.arguments.iter().copied();
                    Some(
                        names
                            .iter()
                            .map(|(name, payload)| {
                                (name.clone(), if *payload { args.next() } else { None })
                            })
                            .collect(),
                    )
                }
                T::Bool => Some(vec![("False".into(), None), ("True".into(), None)]),
                T::PropertyTarget => Some(
                    [
                        "EnumType",
                        "Field",
                        "Member",
                        "StructType",
                        "Type",
                        "Variant",
                    ]
                    .into_iter()
                    .map(|name| (name.into(), None))
                    .collect(),
                ),
                T::Option => Some(vec![
                    ("None".into(), None),
                    ("Some".into(), Some(ty.arguments[0])),
                ]),
                T::Result => Some(vec![
                    ("Err".into(), Some(ty.arguments[1])),
                    ("Ok".into(), Some(ty.arguments[0])),
                ]),
                T::FoldControl => Some(vec![
                    ("Break".into(), Some(ty.arguments[1])),
                    ("Continue".into(), Some(ty.arguments[0])),
                ]),
                _ => None,
            }
            .map(|members| {
                members
                    .into_iter()
                    .map(|(name, payload)| Ok((name, payload.map(TypeId::try_from).transpose()?)))
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?;
            Ok(TypeInfo {
                members,
                kind: (!kind.is_empty()).then_some(kind),
                children,
                body: image.layouts[if ty.constructor == T::Unchecked {
                    ty.arguments[0].index()
                } else {
                    index
                }]
                .as_ref()
                .map(|layout| TypeId::try_from(layout.body))
                .transpose()?,
                opaque_name: if let T::Native(id) = ty.constructor {
                    image.native_name(id).map(str::to_owned)
                } else {
                    None
                },
            })
        })
        .collect()
}

impl Runtime {
    pub(crate) fn reflect(
        &mut self,
        result: TypeId,
        operation: usize,
        input: &Value,
    ) -> Result<Value> {
        let owner = self.represented_type(input.as_ref())?;
        let loc = input.origin().words();
        let info = &self.type_info[owner.index()];
        match operation {
            0 => {
                let kind = info.kind.ok_or("unsupported type descriptor kind")?;
                self.named_variant(result, loc, kind, None)
            }
            1 => {
                let element = self.layout(result)?.arguments[0];
                let values = info
                    .children
                    .iter()
                    .map(|&ty| self.metadata(element, loc, ty))
                    .collect::<Result<Vec<_>>>()?;
                self.array(result, loc, &values)
            }
            2 => {
                let name = info.opaque_name.clone();
                let payload = name
                    .map(|name| self.string(self.layout(result)?.arguments[0], loc, &name))
                    .transpose()?;
                self.named_variant(
                    result,
                    loc,
                    if payload.is_some() { "Some" } else { "None" },
                    payload.as_ref(),
                )
            }
            3 => {
                let args = self.layout(result)?.arguments.clone();
                let (tag, payload) = if let Some(body) = info.body {
                    ("Ok", self.metadata(args[0], loc, body)?)
                } else {
                    (
                        "Err",
                        self.string(args[1], loc, "type descriptor is not a recursive reference")?,
                    )
                };
                self.named_variant(result, loc, tag, Some(&payload))
            }
            4 | 5 => {
                let info = &self.type_info[info.body.unwrap_or(owner).index()];
                let expected = if operation == 4 { "Struct" } else { "Enum" };
                if info.kind != Some(expected) {
                    return Err(format!(
                        "std/type-desc.{} expects {expected}",
                        if operation == 4 { "fields" } else { "variants" }
                    ));
                }
                let members = info.members.clone().ok_or("missing sealed members")?;
                let element = self.layout(result)?.arguments[0];
                let fields = self.layout(element)?.fields.clone();
                let mut values = Vec::with_capacity(members.len());
                for (index, (name, payload)) in members.into_iter().enumerate() {
                    let index = self.scalar(fields[0].0, loc, index as u64)?;
                    let name = self.string(fields[1].0, loc, &name)?;
                    let payload = if operation == 4 {
                        self.metadata(fields[2].0, loc, payload.ok_or("missing field type")?)?
                    } else {
                        let option = fields[2].0;
                        let inner = self.layout(option)?.arguments[0];
                        let value = payload
                            .map(|ty| self.metadata(inner, loc, ty))
                            .transpose()?;
                        self.named_variant(
                            option,
                            loc,
                            if value.is_some() { "Some" } else { "None" },
                            value.as_ref(),
                        )?
                    };
                    values.push(self.aggregate(element, loc, &[index, name, payload])?);
                }
                self.array(result, loc, &values)
            }
            _ => Err("unknown type reflection operation".into()),
        }
    }
}
