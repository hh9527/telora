use serde_json::{Value, json};
use telora_core::{candidate_layout, mir::*};

pub fn emit(executable: &SealedExecutable<'_>) -> Result<String, String> {
    let sealed = executable.sealed_mir();
    let mir = sealed.mir();
    let layouts = candidate_layout::calculate(sealed)?;
    let image = sealed.types();
    let types = mir.types.iter().enumerate().map(|(i,ty)| {
        let layout = &layouts[i];
        let name = match ty.constructor {
            TypeConstructor::Nominal(symbol) => image.definition(symbol).map(|d| d.name.as_str()),
            TypeConstructor::Native(id) => image.native_name(id),
            _ => None,
        };
        let kind = match ty.constructor {
            TypeConstructor::Nominal(_) | TypeConstructor::Unchecked => "Ref",
            TypeConstructor::Bool | TypeConstructor::Enum(_) | TypeConstructor::Option | TypeConstructor::Result | TypeConstructor::FoldControl | TypeConstructor::PropertyTarget => "Enum",
            TypeConstructor::Record(_) => "Struct",
            TypeConstructor::Function => "Func",
            TypeConstructor::Native(_) => "Opaque",
            _ => layout.constructor,
        };
        let fields: Vec<_> = layout.object.as_ref().map(|o|o.members.iter().enumerate().map(|(index,m)|json!({"index":index,"name":m.name,"ty":m.type_id})).collect()).unwrap_or_default();
        let variants: Vec<_> = layout.variants.iter().enumerate().map(|(index,m)|json!({"index":index,"name":m.name,"ty":m.type_id})).collect();
        json!({"constructor":layout.constructor,"kind":kind,"args":layout.arguments,"body":mir.type_layouts[i].as_ref().map(|l|l.body.index()),"fields":fields,"variants":variants,"name":name})
    }).collect::<Vec<Value>>();
    Ok(format!(
        "const types = {};\n",
        serde_json::to_string(&types).map_err(|e| e.to_string())?
    ))
}
