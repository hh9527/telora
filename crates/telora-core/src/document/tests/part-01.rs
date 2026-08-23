    #[test]
    fn converts_negotiated_positions_without_conflating_encodings() {
        let text = DocumentText::new("中😀\r\ncombining: e\u{301}\n");
        let end = "中😀".len() as u32;
        assert_eq!(
            text.position(end, PositionEncoding::Utf8).unwrap(),
            TextPosition::new(0, 7)
        );
        assert_eq!(
            text.position(end, PositionEncoding::Utf16).unwrap(),
            TextPosition::new(0, 3)
        );
        assert_eq!(
            text.position(end, PositionEncoding::Utf32).unwrap(),
            TextPosition::new(0, 2)
        );
        for encoding in [
            PositionEncoding::Utf8,
            PositionEncoding::Utf16,
            PositionEncoding::Utf32,
        ] {
            for offset in [0, 3, 7, 9, 10, 21, 23] {
                let position = text.position(offset, encoding).unwrap();
                assert_eq!(text.offset(position, encoding).unwrap(), offset);
            }
        }
        assert!(
            text.offset(TextPosition::new(0, 2), PositionEncoding::Utf16)
                .is_err()
        );
        assert!(text.position(8, PositionEncoding::Utf8).is_err());
        assert_eq!(
            text.offset(TextPosition::new(2, 0), PositionEncoding::Utf16)
                .unwrap(),
            text.byte_len() as u32
        );
        assert_eq!(text.line_content_offsets(0).unwrap(), (0, 7));
        assert_eq!(text.line_content_offsets(1).unwrap(), (9, 23));
        assert_eq!(text.line_content_offsets(2).unwrap(), (24, 24));
    }

    #[test]
    fn applies_ordered_edits_to_a_cow_snapshot_atomically() {
        let original = DocumentSnapshot::new(DocumentVersion(1), "hello 世界");
        let changed = original
            .changed(
                DocumentVersion(1),
                DocumentVersion(2),
                &[
                    TextEdit::Replace {
                        range: TextRange::new(0, 5).unwrap(),
                        replacement: "goodbye".into(),
                    },
                    TextEdit::Replace {
                        range: TextRange::new(7, 7).unwrap(),
                        replacement: ",".into(),
                    },
                ],
            )
            .unwrap();
        assert_eq!(original.text().to_string(), "hello 世界");
        assert_eq!(changed.text().to_string(), "goodbye, 世界");
        assert_eq!(changed.version(), DocumentVersion(2));

        let invalid = changed.changed(
            DocumentVersion(2),
            DocumentVersion(3),
            &[
                TextEdit::Replace {
                    range: TextRange::new(0, 7).unwrap(),
                    replacement: "hello".into(),
                },
                TextEdit::Replace {
                    range: TextRange::new(7, 100).unwrap(),
                    replacement: String::new(),
                },
            ],
        );
        assert!(invalid.is_err());
        assert_eq!(changed.text().to_string(), "goodbye, 世界");
    }
