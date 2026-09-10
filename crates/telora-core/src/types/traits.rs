#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeConstraint {
    pub parameter: TypeParameterId,
    pub capability: TypeCapability,
    pub location: crate::Location,
}

#[derive(Clone, Debug)]
pub enum TypeCapability {
    /// 编译器内部传递的类型见证，不引入表面约束语法。
    #[doc(hidden)]
    RuntimeType,
    Trait { id: crate::TraitId, name: String },
    Property(TypeDescriptor),
}

impl PartialEq for TypeCapability {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::RuntimeType, Self::RuntimeType) => true,
            (Self::Trait { id: left, .. }, Self::Trait { id: right, .. }) => left == right,
            (Self::Property(left), Self::Property(right)) => left == right,
            _ => false,
        }
    }
}

impl Eq for TypeCapability {}

impl TypeCapability {
    fn display_name(&self) -> String {
        match self {
            Self::RuntimeType => "Type".into(),
            Self::Trait { name, .. } => name.clone(),
            Self::Property(property) => format!("Property({})", property.display_name()),
        }
    }
}
