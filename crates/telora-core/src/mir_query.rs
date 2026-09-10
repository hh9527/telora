//! Read-only queries over the passes' authoritative graph, including invalid
//! programs. No alternate symbol/type graph, inference, sealing or VM access.
use crate::{mir::*, source::Location};

#[derive(Clone, Copy)]
pub struct MirQuery<'a> {
    mir: &'a Mir,
}

pub struct Reference<'a> {
    pub node: HirId,
    pub location: Location,
    pub resolution: &'a ResolveState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemberKind {
    ModuleExport,
    StructField,
}

pub struct Member<'a> {
    pub name: &'a str,
    pub kind: MemberKind,
    pub ty: TypeState,
    pub symbol: Option<SymbolId>,
}

pub struct Completion<'a> {
    pub replacement: crate::source::TextRange,
    pub candidates: Vec<Member<'a>>,
}

impl<'a> MirQuery<'a> {
    pub fn new(mir: &'a Mir) -> Self {
        Self { mir }
    }

    pub fn mir(self) -> &'a Mir {
        self.mir
    }

    pub fn symbols(self) -> impl Iterator<Item = (SymbolId, &'a Symbol)> {
        self.mir
            .symbols
            .iter()
            .enumerate()
            .map(|(index, symbol)| (SymbolId(index as u32), symbol))
    }

    pub fn definition_locations(self, symbol: SymbolId) -> impl Iterator<Item = Location> + 'a {
        self.mir.symbols[symbol.index()]
            .declarations
            .iter()
            .map(move |&node| {
                let declaration = &self.mir.hir[node.index()];
                declaration
                    .children
                    .iter()
                    .find(|edge| edge.role == Role::Name)
                    .map(|edge| self.mir.hir[edge.node.index()].location)
                    .unwrap_or(declaration.location)
            })
    }

    pub fn definition_at(self, location: Location) -> Option<SymbolId> {
        self.symbols()
            .filter(|(_, symbol)| {
                symbol.kind != SymbolKind::Export && !symbol.name.starts_with('\0')
            })
            .flat_map(|(symbol, _)| {
                self.definition_locations(symbol)
                    .map(move |span| (symbol, span))
            })
            .filter(|(_, span)| contains(*span, location))
            .min_by_key(|(_, span)| span.end - span.start)
            .map(|(symbol, _)| symbol)
    }

    pub fn references(self) -> impl Iterator<Item = Reference<'a>> {
        self.mir
            .hir
            .iter()
            .enumerate()
            .filter_map(move |(index, node)| {
                let resolution = &self.mir.resolve_slots[node.resolution?.index()];
                // A field reference covers the member name, not its receiver.
                let location = if matches!(node.kind, HirKind::Field) {
                    node.children
                        .iter()
                        .find(|edge| edge.role == Role::Name)
                        .map(|edge| self.mir.hir[edge.node.index()].location)
                        .unwrap_or(node.location)
                } else {
                    node.location
                };
                Some(Reference {
                    node: HirId(index as u32),
                    location,
                    resolution,
                })
            })
    }

    pub fn reference_at(self, location: Location) -> Option<Reference<'a>> {
        self.references()
            .filter(|reference| contains(reference.location, location))
            .min_by_key(|reference| reference.location.end - reference.location.start)
    }

    /// Unresolved/Conflicted are final pass outcomes, never a request to search
    /// names again or to pick one declaration from ambiguous candidates.
    pub fn target_at(self, location: Location) -> Option<SymbolId> {
        let state = if let Some(reference) = self.reference_at(location) {
            reference.resolution
        } else {
            &self.mir.symbols[self.definition_at(location)?.index()].resolution
        };
        match state {
            ResolveState::Bound(symbol) => Some(*symbol),
            _ => None,
        }
    }

    pub fn references_of(self, symbol: SymbolId) -> Vec<Reference<'a>> {
        let mut references = self.references().filter(|reference| {
            matches!(reference.resolution, ResolveState::Bound(target) if *target == symbol)
        }).collect::<Vec<_>>();
        references.sort_by_key(|reference| {
            (
                reference.location.source,
                reference.location.start,
                reference.location.end,
            )
        });
        // Lowering can use the same source reference in an export and its
        // generated module result. Both edges have the same authoritative ID.
        references.dedup_by_key(|reference| reference.location);
        references
    }

    pub fn expression_at(self, location: Location) -> Option<HirId> {
        self.mir
            .hir
            .iter()
            .enumerate()
            .filter(|(index, node)| {
                self.mir
                    .required_types
                    .get(*index)
                    .copied()
                    .unwrap_or(false)
                    && contains(node.location, location)
            })
            .min_by_key(|(_, node)| node.location.end - node.location.start)
            .map(|(index, _)| HirId(index as u32))
    }

    pub fn symbol_type(self, symbol: SymbolId) -> TypeState {
        let target = match self.mir.symbols[symbol.index()].resolution {
            ResolveState::Bound(target) => target,
            _ => symbol,
        };
        self.mir.ty_slots[self.mir.symbol_types[target.index()].index()]
    }

    pub fn type_at(self, location: Location) -> Option<TypeState> {
        if let Some(reference) = self.reference_at(location) {
            return Some(self.mir.ty_slots[reference.node.ty().index()]);
        }
        if let Some(symbol) = self.definition_at(location) {
            return Some(self.symbol_type(symbol));
        }
        self.expression_at(location)
            .map(|node| self.mir.ty_slots[node.ty().index()])
    }

    pub fn type_name(self, id: TypeId) -> String {
        self.display_type(id, 0)
    }

    pub fn symbol_signature(self, symbol: SymbolId) -> Option<String> {
        let symbol = match self.mir.symbols[symbol.index()].resolution {
            ResolveState::Bound(target) => target,
            _ => symbol,
        };
        let TypeState::Known(ty) = self.symbol_type(symbol) else {
            return None;
        };
        let parameters = &self.mir.symbol_generics[symbol.index()];
        let signature = self.type_name(ty);
        if parameters.is_empty() {
            return Some(signature);
        }
        let parameters = parameters
            .iter()
            .map(|&parameter| {
                let symbol = &self.mir.symbols[parameter.index()];
                let mut bounds = vec![];
                for &declaration in &symbol.declarations {
                    for edge in &self.mir.hir[declaration.index()].children {
                        if edge.role == Role::Bound {
                            let location = self.mir.hir[edge.node.index()].location;
                            if let Ok(text) = self.mir.sources.get(location.source).text().slice(
                                crate::source::TextRange::from_usize(location.range())
                                    .expect("HIR range"),
                            ) {
                                bounds.push(text.into_owned());
                            }
                        }
                    }
                }
                if bounds.is_empty() {
                    symbol.name.clone()
                } else {
                    format!("{}: {}", symbol.name, bounds.join(" + "))
                }
            })
            .collect::<Vec<_>>();
        Some(format!("for({}) {signature}", parameters.join(", ")))
    }

    pub fn completion_at(self, location: Location) -> Option<Completion<'a>> {
        use crate::syntax::telora::lexer::Token;
        if location.start != location.end {
            return None;
        }
        let file = self.mir.sources.get(location.source);
        let cursor = location.start as usize;
        if cursor > file.text().byte_len() {
            return None;
        }
        let (tokens, spans) =
            crate::syntax::telora::lexer::tokenize_document(file.text(), &mut vec![]);
        let significant = tokens
            .iter()
            .zip(&spans)
            .filter(|(token, _)| !matches!(token, Token::Whitespace | Token::Comment))
            .take_while(|(_, span)| span.end <= cursor)
            .collect::<Vec<_>>();
        let (dot, replacement, prefix) = match significant.as_slice() {
            [.., (Token::Dot, dot)] if dot.end == cursor => (
                *dot,
                crate::source::TextRange::at(location.start),
                String::new(),
            ),
            [.., (Token::Dot, dot), (Token::Identifier, prefix)]
                if dot.end == prefix.start && prefix.end == cursor =>
            {
                let replacement = crate::source::TextRange::from_usize((*prefix).clone()).ok()?;
                (
                    *dot,
                    replacement,
                    file.text().slice(replacement).ok()?.into_owned(),
                )
            }
            _ => return None,
        };
        let receiver = u32::try_from(dot.start).ok()?.checked_sub(1)?;
        let ty = self.type_at(Location {
            source: location.source,
            start: receiver,
            end: receiver,
        });
        let mut candidates = match ty {
            Some(TypeState::Known(id)) => self.members(id),
            _ => vec![],
        };
        candidates.retain(|member| member.name.starts_with(&prefix));
        candidates.sort_by_key(|member| member.name);
        candidates.dedup_by_key(|member| member.name);
        Some(Completion {
            replacement,
            candidates,
        })
    }

    /// Completion reads the solved skeleton, including concrete generic member
    /// layouts. It never instantiates a type or evaluates property metadata.
    pub fn members(self, mut id: TypeId) -> Vec<Member<'a>> {
        let mut ty = &self.mir.types[id.index()];
        if ty.constructor == TypeConstructor::Unchecked {
            id = ty.arguments[0];
            ty = &self.mir.types[id.index()];
        }
        match &ty.constructor {
            TypeConstructor::Namespace(module) => self.mir.exports[module.index()]
                .iter()
                .map(|&symbol| Member {
                    name: &self.mir.symbols[symbol.index()].name,
                    kind: MemberKind::ModuleExport,
                    ty: self.symbol_type(symbol),
                    symbol: match self.mir.symbols[symbol.index()].resolution {
                        ResolveState::Bound(target) => Some(target),
                        _ => None,
                    },
                })
                .collect(),
            TypeConstructor::Record(names) => names
                .iter()
                .zip(&ty.arguments)
                .map(|(name, &ty)| Member {
                    name,
                    kind: MemberKind::StructField,
                    ty: TypeState::Known(ty),
                    symbol: None,
                })
                .collect(),
            TypeConstructor::Nominal(symbol) => {
                let Some(definition) = self.mir.type_definitions.iter().find(|definition| {
                    definition.symbol == *symbol && definition.operation == TypeOperation::Struct
                }) else {
                    return vec![];
                };
                let layout = self
                    .mir
                    .type_layouts
                    .get(id.index())
                    .and_then(Option::as_ref);
                definition
                    .members
                    .iter()
                    .enumerate()
                    .map(|(index, member)| Member {
                        name: &member.name,
                        kind: MemberKind::StructField,
                        symbol: None,
                        ty: layout
                            .and_then(|layout| layout.members[index])
                            .map(TypeState::Known)
                            .unwrap_or_else(|| {
                                if definition.parameters.is_empty() {
                                    member
                                        .payload
                                        .map(|slot| self.mir.ty_slots[slot.index()])
                                        .unwrap_or(TypeState::Unknown)
                                } else {
                                    TypeState::Unknown
                                }
                            }),
                    })
                    .collect()
            }
            _ => vec![],
        }
    }

    fn display_type(self, id: TypeId, depth: usize) -> String {
        if depth == 256 {
            return "…".into();
        }
        let ty = &self.mir.types[id.index()];
        let args = ty
            .arguments
            .iter()
            .map(|&id| self.display_type(id, depth + 1))
            .collect::<Vec<_>>();
        match &ty.constructor {
            TypeConstructor::Tuple => format!(
                "({}{})",
                args.join(", "),
                if args.len() == 1 { "," } else { "" }
            ),
            TypeConstructor::Function => {
                let (result, params) = args.split_last().expect("function result");
                format!("Fn({}) -> {result}", params.join(", "))
            }
            TypeConstructor::Record(names) => format!(
                "{{ {} }}",
                names
                    .iter()
                    .zip(&args)
                    .map(|(name, ty)| format!("{name}: {ty}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            TypeConstructor::Namespace(module) => {
                format!("module {}", self.mir.modules[module.index()].name)
            }
            TypeConstructor::Parameter(symbol) => self.mir.symbols[symbol.index()].name.clone(),
            TypeConstructor::Native(native) => {
                let declaration = self.mir.symbols.iter().find(|symbol| symbol.native_type == Some(*native));
                if let Some(symbol) = declaration
                    && let Some(module) = symbol.module {
                    format!("opaque({}#{})", self.mir.modules[module.index()].name, symbol.name)
                } else {
                    // Even incomplete diagnostic graphs retain the admitted
                    // numeric identity; do not expose Rust's debug encoding.
                    format!("opaque(native:{}#{})", native.module, native.slot)
                }
            }
            TypeConstructor::Nominal(symbol) => {
                let name = &self.mir.symbols[symbol.index()].name;
                if args.is_empty() {
                    name.clone()
                } else {
                    format!("{name}({})", args.join(", "))
                }
            }
            TypeConstructor::Meta => format!("TypeOf({})", args.join(", ")),
            cons if args.is_empty() => format!("{cons:?}"),
            cons => format!("{cons:?}({})", args.join(", ")),
        }
    }
}

fn contains(span: Location, location: Location) -> bool {
    span.source == location.source
        && span.start <= location.start
        && location.start < span.end
        && location.end <= span.end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(mir: &Mir, module: &str, needle: &str) -> Location {
        let ModuleState::Source { source, .. } =
            mir.modules.iter().find(|m| m.name == module).unwrap().state
        else {
            panic!("source");
        };
        let file = mir.sources.get(source);
        let start = file.text().to_string().rfind(needle).unwrap() as u32;
        Location {
            source,
            start,
            end: start,
        }
    }

    #[test]
    fn opaque_names_and_generic_arguments_come_from_the_solved_graph() {
        let mir = crate::codegen::tests::graph(
            "import \"std/test\" as test; type Box(T) = struct {value: T}; export def deferred = test.should_ok(fn() { 42 }); export def pair: Box((Int, String)) = {value: (1, \"x\")};",
            "",
        );
        assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
        let query = MirQuery::new(&mir);
        let signature = |name| {
            let symbol = query.symbols().find(|(_, symbol)| symbol.name == name && matches!(symbol.kind, SymbolKind::Declaration(_))).unwrap().0;
            query.symbol_signature(symbol).unwrap()
        };
        assert_eq!(signature("deferred"), "opaque(std/test#Test)");
        assert_eq!(signature("pair"), "Box((Int, String))");
        assert_eq!(signature("Box"), "for(T) TypeOf(Box(T))");
    }

    #[test]
    fn references_and_hover_use_the_resolved_cross_module_identity() {
        let mir = crate::codegen::tests::graph(
            "import \"./math\" {value as renamed}; export def answer = renamed + renamed;",
            "export def value = 42;",
        );
        assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
        let query = MirQuery::new(&mir);
        let usage = position(&mir, "@src/main", "renamed;");
        let target = query.target_at(usage).unwrap();
        assert_eq!(mir.symbols[target.index()].name, "value");
        let declaration = query.definition_locations(target).next().unwrap();
        assert_eq!(
            mir.sources.get(declaration.source).name.as_ref(),
            "@src/math"
        );
        let TypeState::Known(ty) = query.type_at(usage).unwrap() else {
            panic!("known");
        };
        assert_eq!(query.type_name(ty), "Int");
        let references = query.references_of(target);
        assert!(references.iter().any(|r| contains(r.location, usage)));
        assert!(references.len() >= 2);
        assert!(std::ptr::eq(query.mir(), &mir));
    }

    #[test]
    fn shadowed_locals_keep_distinct_symbol_ids() {
        let mir = crate::codegen::tests::graph(
            "def x = 1; export def answer = do { let x = \"inner\"; x }; export def outer = x;",
            "",
        );
        assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
        let query = MirQuery::new(&mir);
        let inner = query.target_at(position(&mir, "@src/main", "x }")).unwrap();
        let outer = query.target_at(position(&mir, "@src/main", "x;")).unwrap();
        assert_ne!(inner, outer);
        let TypeState::Known(ty) = query.symbol_type(inner) else {
            panic!("known");
        };
        assert_eq!(query.type_name(ty), "String");
    }

    #[test]
    fn member_queries_use_export_ids_and_precomputed_generic_layouts() {
        let mir = crate::codegen::tests::graph(
            "import \"./math\" as math; type Box(T) = struct { item: T }; def box: Box(Int) = {item: 42}; export def answer = (math.value, box.item);",
            "export def value = 7; def private_value = 9;",
        );
        assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
        let query = MirQuery::new(&mir);
        let TypeState::Known(namespace) = query
            .type_at(position(&mir, "@src/main", "math.value"))
            .unwrap()
        else {
            panic!("namespace");
        };
        let exports = query.members(namespace);
        assert_eq!(exports.len(), 1);
        assert_eq!(exports[0].name, "value");
        let member = position(&mir, "@src/main", "value,");
        assert_eq!(exports[0].symbol, query.target_at(member));
        let TypeState::Known(owner) = query
            .type_at(position(&mir, "@src/main", "box.item"))
            .unwrap()
        else {
            panic!("owner");
        };
        let fields = query.members(owner);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name, "item");
        let TypeState::Known(field_type) = fields[0].ty else {
            panic!("concrete member");
        };
        assert_eq!(query.type_name(field_type), "Int");
        assert_eq!(
            query.type_at(position(&mir, "@src/main", "item);")),
            Some(TypeState::Known(field_type))
        );
    }

    #[test]
    fn invalid_program_queries_do_not_recover_or_guess_names() {
        let mir = crate::codegen::tests::graph(
            "def duplicated = 1; def duplicated = 2; export def first = duplicated; export def second = missing; export def bad: Int = \"wrong\";",
            "",
        );
        assert!(!mir.diagnostics.is_empty());
        let query = MirQuery::new(&mir);
        let duplicate = position(&mir, "@src/main", "duplicated;");
        assert!(matches!(
            query.reference_at(duplicate).unwrap().resolution,
            ResolveState::Conflicted(_)
        ));
        assert!(query.target_at(duplicate).is_none());
        assert!(matches!(
            query.type_at(duplicate),
            Some(TypeState::Conflicted(_))
        ));
        let missing = position(&mir, "@src/main", "missing;");
        assert!(matches!(
            query.reference_at(missing).unwrap().resolution,
            ResolveState::Unresolved
        ));
        assert!(query.target_at(missing).is_none());
        assert!(matches!(query.type_at(missing), Some(TypeState::Unknown)));
    }
}
