    #[test]
    fn recursive_interning_returns_the_pending_identity_and_never_reuses_aborted_ids() {
        let mut store = TypeStore::default();
        let InternType::Reserved(first) = store.begin(constructor(1024), []) else {
            panic!("first application reserves an ID")
        };
        assert_eq!(first.raw(), 1024);
        assert!(store.is_pending(first));
        assert_eq!(
            store.begin(constructor(1024), []),
            InternType::Existing(first)
        );
        store.abort(first).unwrap();

        let InternType::Reserved(second) = store.begin(constructor(1024), []) else {
            panic!("aborted application can be retried")
        };
        assert_eq!(second.raw(), 1025);
        store
            .seal(
                second,
                TypeData {
                    name: "Node".into(),
                    shape: TypeShape::Struct(Box::new([("next".into(), second)])),
                },
            )
            .unwrap();
        assert_eq!(store.get(second).unwrap().name, "Node");
    }

    #[test]
    fn constructor_identity_is_part_of_the_intern_key() {
        let mut store = TypeStore::default();
        let InternType::Reserved(left) = store.begin(constructor(1024), []) else {
            unreachable!()
        };
        let InternType::Reserved(right) = store.begin(constructor(1025), []) else {
            unreachable!()
        };
        assert_ne!(left, right);
    }

    #[test]
    fn nominal_recursion_is_a_finite_canonical_graph() {
        let mut store = TypeStore::default();
        let constructor = constructor(1024);
        let InternType::Reserved(node) = store.begin(constructor, []) else {
            unreachable!()
        };
        let names = HashMap::from([("Node".into(), node)]);
        store
            .seal_descriptor(
                node,
                "Node",
                &TypeDescriptor::Struct(std::collections::BTreeMap::from([(
                    "next".into(),
                    TypeDescriptor::Named("Node".into()),
                )])),
                &names,
            )
            .unwrap();

        let TypeShape::Struct(fields) = &store.get(node).unwrap().shape else {
            panic!("Node must retain its nominal struct body")
        };
        assert_eq!(fields.as_ref(), &[("next".into(), node)]);
    }

    #[test]
    fn parameterized_nominal_applications_are_memoized_by_type_ids() {
        let mut store = TypeStore::default();
        let constructor = constructor(1024);
        let InternType::Reserved(option_int) = store.begin(constructor, [TypeId::INT]) else {
            unreachable!()
        };
        assert_eq!(
            store.begin(constructor, [TypeId::INT]),
            InternType::Existing(option_int)
        );
        let InternType::Reserved(option_string) = store.begin(constructor, [TypeId::STRING]) else {
            unreachable!()
        };
        assert_ne!(option_int, option_string);
    }

    #[test]
    fn failed_nominal_definition_removes_the_pending_memo_entry() {
        let mut store = TypeStore::default();
        let constructor = constructor(1024);
        let InternType::Reserved(failed) = store.begin(constructor, []) else {
            unreachable!()
        };
        assert!(
            store
                .seal_descriptor(
                    failed,
                    "NotNominal",
                    &TypeDescriptor::Array(Box::new(TypeDescriptor::Int)),
                    &HashMap::new(),
                )
                .is_err()
        );
        assert!(!store.is_pending(failed));
        assert!(store.get(failed).is_none());

        let InternType::Reserved(retry) = store.begin(constructor, []) else {
            panic!("failed nominal construction must be retryable")
        };
        assert_ne!(retry, failed);
    }
