use crate::mir::{Mir, ModuleKind};

pub(crate) fn graph(main: &str, math: &str) -> Mir {
    graph_order(main, math, 0)
}

fn graph_order(main: &str, math: &str, order: usize) -> Mir {
    let mut inventory = crate::static_sources::BUILTINS
        .iter()
        .map(|(name, _)| crate::module_resolve::ModuleSpec {
            name: (*name).into(),
            kind: ModuleKind::Source,
            native: crate::static_sources::native_module(name),
            implicit_imports: if *name == "std/prelude" {
                vec![]
            } else {
                vec!["std/prelude".into()]
            },
        })
        .collect::<Vec<_>>();
    for name in ["@src/main", "@src/main/math"] {
        inventory.push(crate::module_resolve::ModuleSpec {
            name: name.into(),
            kind: ModuleKind::Source,
            native: None,
            implicit_imports: vec!["std/prelude".into()],
        });
    }
    let main = if math.is_empty() {
        main.to_owned()
    } else {
        format!("mod math; {main}")
    };
    if order > 0 {
        inventory.reverse();
        let length = inventory.len();
        inventory.rotate_left(order % length);
    }
    let mut mir = crate::module_resolve::resolve(inventory, &["@src/main".into()], |_, name| {
        Ok(if name == "@src/main" {
            main.clone()
        } else if name == "@src/main/math" {
            math.into()
        } else {
            crate::static_sources::BUILTINS
                .iter()
                .find(|(n, _)| *n == name)
                .unwrap()
                .1
                .into()
        })
    });
    crate::symbol_resolve::resolve(&mut mir);
    crate::type_resolve::resolve(&mut mir);
    mir
}

#[test]
fn sealed_full_build_is_independent_of_inventory_enumeration_order() {
    let main = "use self::math::{ identity }; \
                use self::math::{ choose }; \
                use std::array::{ map, fold }; \
                def selected: for(B) Fn(Int, B) -> Int = choose@[Int, _]; \
                def seed: Int = selected(0, \"seed\"); \
                def other: Int = selected(0, True); \
                @property(PropertyTarget::Type) type Mark = struct { value: Int }; \
                def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) { {value: seed} }; \
                @mark \
                type Tree = enum { Leaf(Int), Branch((Tree, Tree)) }; \
                pub def answer: Int = fold(map([1, 2, 3], fn(x) { identity(x * 7) }), seed + other, fn(a, b) { a + b });";
    let math = "pub def identity: for(T) Fn(T) -> T = fn(x) { x }; \
                pub def choose: for(A, B) Fn(A, B) -> A = fn(a, b) { a };";
    let baseline = graph_order(main, math, 0);
    let sealed = baseline.seal().unwrap();
    let expected_image = format!("{:?}", sealed.types());
    for order in [1, 7, 19] {
        let rebuilt = graph_order(main, math, order);
        let sealed = rebuilt.seal().unwrap();
        assert_eq!(sealed.mir().dump(), baseline.dump());
        assert_eq!(format!("{:?}", sealed.types()), expected_image);
    }
}
