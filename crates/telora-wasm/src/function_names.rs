//! Debug names only: identities and code generation never depend on these labels.
use crate::plan::{Key, Special};
use telora_core::{
    mir::{HirKind, Mir, Role},
    mir_query::MirQuery,
};

pub(crate) struct FunctionNames<'a> {
    mir: &'a Mir,
    parents: Vec<Option<usize>>,
}

impl<'a> FunctionNames<'a> {
    pub fn new(mir: &'a Mir) -> Self {
        let mut parents = vec![None; mir.hir.len()];
        for (index, node) in mir.hir.iter().enumerate() {
            for edge in &node.children {
                parents[edge.node.index()] = Some(index);
            }
        }
        Self { mir, parents }
    }

    pub fn name(&self, key: Key, index: u32) -> String {
        let mir = self.mir;
        let node = &mir.hir[key.node.index()];
        let query = MirQuery::new(mir);
        let mut owner = None;
        let mut cursor = Some(key.node.index());
        while let Some(id) = cursor {
            let hir = &mir.hir[id];
            if matches!(hir.kind, HirKind::Binding { .. }) {
                owner = hir.children.iter().find_map(|edge| {
                    if edge.role == Role::Name {
                        if let HirKind::Name(name) = &mir.hir[edge.node.index()].kind {
                            return Some(name.as_str());
                        }
                    }
                    None
                });
                if owner.is_some() {
                    break;
                }
            }
            cursor = self.parents[id];
        }
        let role = match key.special {
            Special::Normal if key.callable => "call".to_owned(),
            Special::Normal => "demand".to_owned(),
            Special::Configured => "configured".to_owned(),
            Special::Property(id) => format!("property[{id}]"),
            Special::Equal(ty) => format!("equal[{}#{}]", query.type_name(ty), ty.index()),
            Special::Parse(ty) => format!("parse[{}#{}]", query.type_name(ty), ty.index()),
            Special::Json(ty) => format!("json[{}#{}]", query.type_name(ty), ty.index()),
            Special::Encode(a, b)
            | Special::Decode(a, b)
            | Special::DecodeVariant(a, b, _) => {
                let kind = match key.special {
                    Special::Encode(..) => "encode".to_owned(),
                    Special::Decode(..) => "decode".to_owned(),
                    Special::DecodeVariant(_, _, variant) => format!("decode-variant-{variant}"),
                    _ => unreachable!(),
                };
                format!(
                    "{kind}[{}#{},{}#{}]",
                    query.type_name(a),
                    a.index(),
                    query.type_name(b),
                    b.index()
                )
            }
        };
        let instance = key
            .instance
            .map(|id| {
                let instance = &mir.generic_instances[id.index()];
                let args = instance
                    .arguments
                    .iter()
                    .map(|(symbol, ty)| {
                        format!(
                            "{}={}#{}",
                            mir.symbols[symbol.index()].name,
                            query.type_name(*ty),
                            ty.index()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                format!(
                    "|instance={}::{}<{args}>",
                    id.index(),
                    mir.symbols[instance.symbol.index()].name
                )
            })
            .unwrap_or_default();
        format!(
            "telora_fn_{index}|module={}|owner={}|role={role}|hir={}|loc={}:{}..{}{instance}",
            mir.modules[node.module.index()].name,
            owner.unwrap_or("<anonymous>"),
            key.node.index(),
            node.location.source.get(),
            mir.sources.get(node.location.source).coordinates(node.location).start(),
            mir.sources.get(node.location.source).coordinates(node.location).end()
        )
    }
}
