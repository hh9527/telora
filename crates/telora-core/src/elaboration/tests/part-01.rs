    #[test]
    fn propagation_elaborates_to_hygienic_core_forms() {
        let location = location();
        let operand = located(
            ExprKind::Variable(located("input".into(), location)),
            location,
        );
        let mut elaborator = Elaborator {
            families: &HashMap::new(),
            not_families: &HashMap::new(),
            trait_member_evidence: &HashMap::new(),
            generic_call_evidence: &HashMap::new(),
            generic_evidence_parameters: &HashMap::new(),
            generic_dictionary_factories: &HashMap::new(),
            next: 0,
        };
        let ExprKind::Block(block) =
            elaborator.propagation(operand, PropagationFamily::Result, location)
        else {
            panic!("propagation must elaborate to a block")
        };
        let subject = &block.value.bindings[0].value.name.value;
        assert!(subject.starts_with("$propagate:"));
        let ExprKind::Match { arms, .. } = &block.value.result.value else {
            panic!("elaborated block must end in match")
        };
        assert!(
            matches!(arms[0].value.pattern.value, PatternKind::Tagged { ref tag, .. } if tag == "Ok")
        );
        assert!(matches!(arms[1].value.value.value, ExprKind::Return { .. }));
        assert_eq!(arms[1].value.value.location, location);
    }
