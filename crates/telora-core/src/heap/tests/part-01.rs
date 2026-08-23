    #[test]
    #[cfg(target_pointer_width = "64")]
    fn val_is_compact_and_copy() {
        fn assert_copy<T: Copy>() {}

        assert_copy::<Val>();
        assert_eq!(std::mem::size_of::<Val>(), 32);
        assert_eq!(std::mem::align_of::<Val>(), 8);
        assert_eq!(std::mem::size_of::<Meta>(), 4);
    }

    #[test]
    fn flat_meta_round_trips_exact_classification_and_traits() {
        for storage in [Storage::Main, Storage::Work] {
            let value = Val::unknown(DecodedValue::Array(Handle { storage, slot: 7 }));
            assert_eq!(
                value.value(),
                DecodedValue::Array(Handle { storage, slot: 7 })
            );
            assert_eq!(value.meta.sub_kind(), HeapKind::Array);
            assert_ne!(value.meta.traits() & TRAIT_REFERENCE, 0);
            assert_ne!(value.meta.traits() & TRAIT_HEAP, 0);
            assert_ne!(value.meta.traits() & TRAIT_TRACE, 0);
            assert_eq!(ScopedId::from_raw(value.raw).storage(), storage);
        }
    }

    #[test]
    fn inline_text_and_native_type_use_no_heap_or_text_slot() {
        let mut heap = Heap::work();
        let short_string = Val::unknown(heap.string(None, "1234567"));
        let short_atom = Val::unknown(heap.atom(None, "1234567"));
        let native = crate::NativeType::bind(
            crate::value::NativeTypeId {
                module: crate::value::NativeModuleId(7),
                local: 11,
            },
            "fixture#Native",
        );
        let native_value = Val::unknown(DecodedValue::NativeType(
            heap.intern_native_type(native.clone()),
        ));

        assert_eq!(heap.counts(), (0, 0, 0));
        let view = HeapView {
            current: &heap,
            background: None,
        };
        assert_eq!(view.string_text(short_string).unwrap().unwrap(), "1234567");
        assert_eq!(view.atom_text(short_atom).unwrap().unwrap(), "1234567");
        let DecodedValue::NativeType(id) = native_value.value() else {
            panic!("expected immediate NativeType")
        };
        assert_eq!(heap.native_type(id).unwrap(), &native);

        let long = Val::unknown(heap.string(None, "12345678"));
        assert!(matches!(long.value(), DecodedValue::ShortString(_)));
        assert_eq!(heap.counts(), (0, 1, 0));
    }

    #[test]
    fn canonical_type_id_is_independent_from_value_storage() {
        let raw = Val::unknown(DecodedValue::Int(1));
        let typed = raw.with_type_id(crate::TypeId::builtin(7));
        assert_eq!(typed.type_id(), Some(crate::TypeId::builtin(7)));
        assert_eq!(typed.value(), DecodedValue::Int(1));
    }

    #[test]
    fn equality_never_guesses_across_a_nominal_witness_boundary() {
        let heap = Heap::work();
        let view = HeapView {
            current: &heap,
            background: None,
        };
        let raw = Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::True));
        let typed = raw.with_type_id(crate::TypeId::builtin(7));
        assert!(!view.values_equal(typed, raw).unwrap());
        assert!(!view.values_equal(raw, typed).unwrap());
        assert!(view.values_equal(typed, typed).unwrap());
    }

    #[test]
    fn val_equality_ignores_location() {
        let left = Val::new(DecodedValue::Int(42), Some(location("left", 1..2)));
        let right = Val::new(DecodedValue::Int(42), Some(location("right", 3..4)));

        assert_eq!(left, right);
    }

    #[test]
    fn call_site_rebasing_preserves_original_values_only() {
        let original_loc = location("data", 1..2);
        let generated_loc = location("function", 3..4);
        let call_loc = location("caller", 5..6);

        let original = Val::original(DecodedValue::Int(1), Some(original_loc));
        let generated = Val::new(DecodedValue::Int(2), Some(generated_loc));

        let preserved = original.rebase_generated(Some(call_loc));
        assert!(preserved.is_original());
        assert_eq!(preserved.loc(), Some(original_loc));
        assert_eq!(
            generated.rebase_generated(Some(call_loc)).loc(),
            Some(call_loc)
        );
        assert_eq!(
            Val::unknown(DecodedValue::Int(3))
                .rebase_generated(Some(call_loc))
                .loc(),
            Some(call_loc)
        );
    }

    #[test]
    fn copy_preserves_root_and_collection_edge_locations() {
        let root_loc = location("root", 0..5);
        let item_loc = location("item", 6..7);
        let mut world = Heap::main();
        let mut current = Heap::work();
        let array = current.allocate(Object::Array(
            vec![Val::original(DecodedValue::Int(42), Some(item_loc))].into(),
        ));
        let root = Val::original(DecodedValue::Array(array), Some(root_loc));

        let copied = copy_roots(
            &mut world,
            HeapView {
                current: &current,
                background: None,
            },
            &[root],
        )
        .unwrap()[0];

        assert_eq!(copied.loc(), Some(root_loc));
        assert!(copied.is_original());
        let DecodedValue::Array(handle) = copied.value() else {
            panic!("expected copied Array")
        };
        let Object::Array(items) = world.object(handle).unwrap() else {
            panic!("expected copied Array object")
        };
        assert_eq!(items[0].loc(), Some(item_loc));
        assert!(items[0].is_original());
    }

    #[test]
    fn copy_is_reachable_reinterning_and_target_self_contained() {
        let mut world = Heap::main();
        let shared = world.allocate(Object::Bytes(vec![9].into()));
        let mut current = Heap::work();
        let atom = current.atom(Some(&world), "Custom");
        let string = current.string(Some(&world), "Custom");
        let root = current.allocate(Object::Tuple(
            vec![rv(atom), rv(string), rv(DecodedValue::Bytes(shared))].into(),
        ));
        current.allocate(Object::Bytes(vec![1, 2, 3].into()));

        let copied = copy_roots(
            &mut world,
            HeapView {
                current: &current,
                background: None,
            },
            &[rv(DecodedValue::Tuple(root))],
        )
        .unwrap();

        assert_eq!(world.counts(), (2, 0, 0));
        let DecodedValue::Tuple(root) = copied[0].value() else {
            panic!("expected tuple root")
        };
        let Object::Tuple(values) = world.object(root).unwrap() else {
            panic!("expected tuple object")
        };
        assert_eq!(values[2], rv(DecodedValue::Bytes(shared)));
        assert!(
            !values
                .iter()
                .any(|value| val_contains_foreign(*value, Storage::Main))
        );
    }

    #[test]
    fn copy_preserves_cycles_and_failure_is_atomic() {
        let mut world = Heap::main();
        let mut current = Heap::work();
        let cycle = current.reserve();
        current
            .initialize(
                cycle,
                Object::Array(vec![rv(DecodedValue::Array(cycle))].into()),
            )
            .unwrap();
        copy_roots(
            &mut world,
            HeapView {
                current: &current,
                background: None,
            },
            &[rv(DecodedValue::Array(cycle))],
        )
        .unwrap();
        assert_eq!(world.counts().0, 1);

        let before = world.counts();
        let invalid = DecodedValue::Array(Handle {
            storage: Storage::Work,
            slot: 99,
        });
        assert!(
            copy_roots(
                &mut world,
                HeapView {
                    current: &current,
                    background: None,
                },
                &[rv(invalid)],
            )
            .is_err()
        );
        assert_eq!(world.counts(), before);
    }

    #[test]
    fn multiple_roots_share_one_forwarding_context() {
        let mut target = Heap::main();
        let mut source = Heap::work();
        let shared = source.allocate(Object::Bytes(vec![1].into()));
        let roots = copy_roots(
            &mut target,
            HeapView {
                current: &source,
                background: None,
            },
            &[
                rv(DecodedValue::Bytes(shared)),
                rv(DecodedValue::Bytes(shared)),
            ],
        )
        .unwrap();
        assert_eq!(roots[0], roots[1]);
        assert_eq!(target.counts().0, 1);
    }

    #[test]
    fn work_relocation_copies_work_edges_and_retains_main_edges() {
        let mut main = Heap::main();
        let stable = main.allocate(Object::Bytes(vec![9].into()));
        let mut source = Heap::work();
        let shared = source.allocate(Object::Bytes(vec![1, 2, 3].into()));
        let cycle = source.reserve();
        source
            .initialize(
                cycle,
                Object::Array(vec![rv(DecodedValue::Array(cycle))].into()),
            )
            .unwrap();
        let root = source.allocate(Object::Tuple(
            vec![
                rv(DecodedValue::Bytes(shared)),
                rv(DecodedValue::Bytes(shared)),
                rv(DecodedValue::Bytes(stable)),
                rv(DecodedValue::Array(cycle)),
            ]
            .into(),
        ));
        source.allocate(Object::Bytes(vec![4, 5, 6].into()));
        let mut target = Heap::work();
        target.allocate(Object::Bytes(Box::new([])));

        let relocated = relocate_work_roots(
            &mut target,
            &main,
            &source,
            &[rv(DecodedValue::Tuple(root))],
        )
        .unwrap();

        assert_eq!(target.counts().0, 4);
        let DecodedValue::Tuple(root) = relocated[0].value() else {
            panic!("expected relocated tuple")
        };
        let Object::Tuple(values) = target.object(root).unwrap() else {
            panic!("expected relocated tuple object")
        };
        assert_eq!(values[0], values[1]);
        assert_eq!(values[2], rv(DecodedValue::Bytes(stable)));
        let DecodedValue::Array(cycle) = values[3].value() else {
            panic!("expected relocated cycle")
        };
        let Object::Array(cycle_values) = target.object(cycle).unwrap() else {
            panic!("expected relocated cycle object")
        };
        assert_eq!(cycle_values[0], rv(DecodedValue::Array(cycle)));
        assert_ne!(root.slot, 0);
    }

    #[test]
    fn failed_nodes_cross_module_publication_but_not_host_publication() {
        let main = Heap::main();
        let mut source = Heap::work();
        let root = Val::unknown(DecodedValue::Array(source.allocate(Object::Array(
            vec![Val::unknown(DecodedValue::Failed(7))].into(),
        ))));

        let mut target = Heap::work();
        let relocated = relocate_work_roots(&mut target, &main, &source, &[root]).unwrap();
        let DecodedValue::Array(handle) = relocated[0].value() else {
            panic!("expected relocated Array")
        };
        let Object::Array(items) = target.object(handle).unwrap() else {
            panic!("expected relocated Array object")
        };
        assert!(matches!(items[0].value(), DecodedValue::Failed(7)));
        let mut destination = Heap::main();
        assert!(publish_root(&mut destination, &source, root).is_err());
        let published = publish_module_root(&mut destination, &source, root).unwrap();
        assert_eq!(
            HeapView {
                current: &destination,
                background: None,
            }
            .first_data_failure(published.runtime())
            .unwrap(),
            Some(7)
        );
    }

    #[test]
    fn typed_property_batch_failure_publishes_no_partial_heads() {
        let mut main = Heap::main();
        let work = Heap::work_for(&main);
        let property_ty = crate::TypeId::builtin(2);
        let first = PropertyKey::Ty {
            ty: crate::TypeId::builtin(3),
            property_ty,
        };
        let second = PropertyKey::Field {
            ty: crate::TypeId::builtin(3),
            member_index: 0,
            property_ty,
        };
        let valid = work.int(1).with_type_id(property_ty);
        let failed = Val::unknown(DecodedValue::Failed(7)).with_type_id(property_ty);

        assert!(
            publish_type_properties(
                &mut main,
                &work,
                Some(property_ty),
                &[(first, valid), (second, failed)],
            )
            .is_err()
        );
        let view = HeapView {
            current: &main,
            background: None,
        };
        assert!(view.property(first).is_none());
        assert!(view.property(second).is_none());
        assert_eq!(main.property_attr_type(), None);
    }

    #[test]
    fn publication_preserves_main_edges_and_relocates_work_edges() {
        let mut main = Heap::main();
        let stable = rv(DecodedValue::Bytes(
            main.allocate(Object::Bytes(vec![1, 2, 3].into_boxed_slice())),
        ));
        let mut work = Heap::work();
        let work_root = work.allocate(Object::Array(vec![stable].into()));

        let published = publish_root(&mut main, &work, rv(DecodedValue::Array(work_root)))
            .unwrap()
            .runtime();
        let DecodedValue::Array(main_root) = published.value() else {
            panic!("expected published Array")
        };
        assert_eq!(main_root.storage, Storage::Main);
        let Object::Array(items) = main.object(main_root).unwrap() else {
            panic!("expected Main Array")
        };
        let DecodedValue::Bytes(stable_bytes) = items[0].value() else {
            panic!("expected Main Bytes")
        };
        assert_eq!(stable_bytes.storage, Storage::Main);
        assert_eq!(main.counts(), (2, 0, 0));
    }
    #[test]
    fn structural_equality_terminates_on_internal_cycles() {
        let mut local = Heap::work();
        let left = local.reserve();
        local
            .initialize(
                left,
                Object::Array(vec![rv(DecodedValue::Array(left))].into()),
            )
            .unwrap();
        let right = local.reserve();
        local
            .initialize(
                right,
                Object::Array(vec![rv(DecodedValue::Array(right))].into()),
            )
            .unwrap();
        let world = Heap::main();
        assert!(
            HeapView {
                current: &local,
                background: Some(&world),
            }
            .values_equal(
                rv(DecodedValue::Array(left)),
                rv(DecodedValue::Array(right))
            )
            .unwrap()
        );
    }

    #[test]
    fn promotion_copies_ready_type_slots_and_rejects_uninitialized_links() {
        let mut local = Heap::work();
        let link = local.allocate(Object::TypeSlot { value: None });
        let array = local.allocate(Object::Array(vec![rv(DecodedValue::TypeSlot(link))].into()));
        local
            .initialize_type_slot(link, rv(DecodedValue::Array(array)))
            .unwrap();
        let mut world = Heap::main();
        let DecodedValue::TypeSlot(persistent_link) =
            publish_root(&mut world, &local, rv(DecodedValue::TypeSlot(link)))
                .unwrap()
                .runtime()
                .value()
        else {
            panic!("expected persistent up-link")
        };
        let reader = Heap::work();
        let view = HeapView {
            current: &reader,
            background: Some(&world),
        };
        let DecodedValue::Array(array) = view
            .type_slot(persistent_link)
            .unwrap()
            .expect("published up-link is ready")
            .value()
        else {
            panic!("expected Array")
        };
        assert_eq!(
            view.sequence(array, false).unwrap(),
            &[rv(DecodedValue::TypeSlot(persistent_link))]
        );

        let mut uninitialized = Heap::work();
        let link = uninitialized.allocate(Object::TypeSlot { value: None });
        assert!(
            publish_root(&mut world, &uninitialized, rv(DecodedValue::TypeSlot(link))).is_err()
        );
    }
