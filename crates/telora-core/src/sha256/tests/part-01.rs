    #[test]
    fn standard_vectors() {
        assert_eq!(
            hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn streaming_matches_one_shot_across_block_boundaries() {
        let input = vec![b'x'; 137];
        let mut context = Context::default();
        context.update(&input[..3]);
        context.update(&input[3..64]);
        context.update(&input[64..]);
        let digest = context.finish();
        assert_eq!(
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
            hex(&input)
        );
    }
