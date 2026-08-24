    #[test]
    fn negotiates_supported_position_encodings() {
        assert_eq!(negotiate_encoding(None), PositionEncoding::Utf16);
        assert_eq!(
            negotiate_encoding(Some(vec![lsp::PositionEncodingKind::UTF16])),
            PositionEncoding::Utf16
        );
        assert_eq!(
            negotiate_encoding(Some(vec![
                lsp::PositionEncodingKind::UTF32,
                lsp::PositionEncodingKind::UTF8,
            ])),
            PositionEncoding::Utf8
        );
    }

    #[test]
    fn initialize_uses_client_root_and_advertises_incremental_sync() {
        let (root, state) = fixture();
        let result = initialize_state(&root, &state);
        assert_eq!(state.borrow().lifecycle, Lifecycle::Running);
        assert_eq!(
            result.capabilities.position_encoding,
            Some(lsp::PositionEncodingKind::UTF16)
        );
        assert!(matches!(
            result.capabilities.text_document_sync,
            Some(lsp::TextDocumentSyncCapability::Options(
                lsp::TextDocumentSyncOptions {
                    open_close: Some(true),
                    change: Some(lsp::TextDocumentSyncKind::INCREMENTAL),
                    ..
                }
            ))
        ));
        let completion = result
            .capabilities
            .completion_provider
            .expect("completion capability");
        assert_eq!(completion.resolve_provider, Some(false));
        assert_eq!(completion.trigger_characters, Some(vec![".".to_owned()]));
    }

    #[test]
    fn applies_ordered_utf16_changes_transactionally() {
        let (root, state) = fixture();
        initialize_state(&root, &state);
        let path = root.join("main.telora");
        let uri = lsp::Url::from_file_path(&path).expect("document URI");
        {
            let mut state = state.borrow_mut();
            state.workspace = Some(Rc::new(
                Workspace::new(&path, Engine::new(config())).expect("create document workspace"),
            ));
            state
                .workspace
                .as_ref()
                .expect("workspace")
                .open(&path, DocumentVersion(1), "a😀c")
                .expect("open document");
            state.documents.insert(path.clone(), 1);
        }

        apply_changes(
            &state,
            lsp::DidChangeTextDocumentParams {
                text_document: lsp::VersionedTextDocumentIdentifier { uri, version: 2 },
                content_changes: vec![
                    lsp::TextDocumentContentChangeEvent {
                        range: Some(lsp::Range::new(
                            lsp::Position::new(0, 1),
                            lsp::Position::new(0, 3),
                        )),
                        range_length: Some(2),
                        text: "中".to_owned(),
                    },
                    lsp::TextDocumentContentChangeEvent {
                        range: Some(lsp::Range::new(
                            lsp::Position::new(0, 2),
                            lsp::Position::new(0, 3),
                        )),
                        range_length: Some(1),
                        text: "d".to_owned(),
                    },
                ],
            },
        )
        .expect("apply changes");

        let document = state
            .borrow()
            .workspace
            .as_ref()
            .expect("workspace")
            .document(&path)
            .expect("open document");
        assert_eq!(document.version(), DocumentVersion(2));
        assert_eq!(document.text().to_string(), "a中d");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn notifications_apply_full_changes_and_reject_bad_transactions() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let (root, state) = fixture();
                initialize_state(&root, &state);
                let path = root.join("new.telora");
                let uri = lsp::Url::from_file_path(&path).expect("document URI");
                let open: AnyNotification = serde_json::from_value(serde_json::json!({
                    "method": "textDocument/didOpen",
                    "params": {
                        "textDocument": {
                            "uri": uri,
                            "languageId": "telora",
                            "version": 1,
                            "text": "a😀c"
                        }
                    }
                }))
                .expect("open notification");
                assert_eq!(
                    dispatch_notification(Rc::clone(&state), open).expect("open document"),
                    NotificationOutcome::Continue
                );

                let full: AnyNotification = serde_json::from_value(serde_json::json!({
                    "method": "textDocument/didChange",
                    "params": {
                        "textDocument": { "uri": uri, "version": 2 },
                        "contentChanges": [{ "text": "valid" }]
                    }
                }))
                .expect("change notification");
                dispatch_notification(Rc::clone(&state), full).expect("full change");
                let workspace = Rc::clone(state.borrow().workspace.as_ref().expect("workspace"));
                let revision = workspace.revision();
                assert_eq!(
                    workspace
                        .document(&path)
                        .expect("document")
                        .text()
                        .to_string(),
                    "valid"
                );

                let out_of_order: AnyNotification = serde_json::from_value(serde_json::json!({
                    "method": "textDocument/didChange",
                    "params": {
                        "textDocument": { "uri": uri, "version": 2 },
                        "contentChanges": [{ "text": "wrong" }]
                    }
                }))
                .expect("change notification");
                assert!(dispatch_notification(Rc::clone(&state), out_of_order).is_err());
                assert_eq!(workspace.revision(), revision);

                let invalid_boundary: AnyNotification = serde_json::from_value(serde_json::json!({
                    "method": "textDocument/didChange",
                    "params": {
                        "textDocument": { "uri": uri, "version": 3 },
                        "contentChanges": [{
                            "range": {
                                "start": { "line": 0, "character": 99 },
                                "end": { "line": 0, "character": 99 }
                            },
                            "text": "wrong"
                        }]
                    }
                }))
                .expect("change notification");
                assert!(dispatch_notification(Rc::clone(&state), invalid_boundary).is_err());
                assert_eq!(workspace.revision(), revision);
                assert_eq!(
                    workspace
                        .document(&path)
                        .expect("document")
                        .text()
                        .to_string(),
                    "valid"
                );

                let close: AnyNotification = serde_json::from_value(serde_json::json!({
                    "method": "textDocument/didClose",
                    "params": { "textDocument": { "uri": uri } }
                }))
                .expect("close notification");
                dispatch_notification(Rc::clone(&state), close).expect("close document");
                assert!(workspace.document(path).is_err());
                assert!(state.borrow().documents.is_empty());
            })
            .await;
    }

    #[test]
    fn cancel_notification_sets_registered_telora_token() {
        let (_, state) = fixture();
        let token = CancellationToken::default();
        state
            .borrow()
            .pending
            .borrow_mut()
            .insert(RequestId::Number(7), token.clone());
        let notification: AnyNotification = serde_json::from_value(serde_json::json!({
            "method": "$/cancelRequest",
            "params": { "id": 7 }
        }))
        .expect("cancel notification");

        assert_eq!(
            dispatch_notification(state, notification).expect("dispatch cancellation"),
            NotificationOutcome::Continue
        );
        assert!(token.is_cancelled());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancellation_reaches_a_pending_query_checkpoint() {
        let (_, state, uri) = semantic_fixture("let value = 1;\nvalue").await;
        let mut future = Box::pin(dispatch_request(
            Rc::clone(&state),
            request(
                41,
                lsp::request::HoverRequest::METHOD,
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": 1, "character": 1 }
                }),
            ),
        ));
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));
        assert!(
            state
                .borrow()
                .pending
                .borrow()
                .contains_key(&RequestId::Number(41))
        );

        let cancel: AnyNotification = serde_json::from_value(serde_json::json!({
            "method": "$/cancelRequest",
            "params": { "id": 41 }
        }))
        .expect("cancel notification");
        assert_eq!(
            dispatch_notification(Rc::clone(&state), cancel).expect("cancel request"),
            NotificationOutcome::Continue
        );
        let error = future.await.expect_err("cancelled response");
        assert_eq!(error.code, ErrorCode::REQUEST_CANCELLED);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn edit_makes_a_pending_query_content_modified() {
        let (path, state, uri) = semantic_fixture("let value = 1;\nvalue").await;
        let mut future = Box::pin(dispatch_request(
            Rc::clone(&state),
            request(
                42,
                lsp::request::HoverRequest::METHOD,
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": 1, "character": 1 }
                }),
            ),
        ));
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));

        let workspace = Rc::clone(state.borrow().workspace.as_ref().expect("workspace"));
        workspace
            .change(
                &path,
                DocumentVersion(1),
                DocumentVersion(2),
                &[TextEdit::Full("let value = 2;\nvalue".to_owned())],
            )
            .expect("advance revision");
        let error = future.await.expect_err("stale response");
        assert_eq!(error.code, ErrorCode::CONTENT_MODIFIED);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn duplicate_ids_and_shutdown_with_pending_work_are_deterministic() {
        let (root, state) = fixture();
        initialize_state(&root, &state);
        let original = CancellationToken::default();
        state
            .borrow()
            .pending
            .borrow_mut()
            .insert(RequestId::Number(9), original.clone());
        let duplicate = dispatch_request(
            Rc::clone(&state),
            request(9, lsp::request::HoverRequest::METHOD, serde_json::json!({})),
        )
        .await
        .expect_err("duplicate request ID");
        assert_eq!(duplicate.code, ErrorCode::INVALID_REQUEST);
        assert!(!original.is_cancelled());

        dispatch_request(
            Rc::clone(&state),
            request(10, lsp::request::Shutdown::METHOD, serde_json::Value::Null),
        )
        .await
        .expect("shutdown response");
        assert!(original.is_cancelled());
        assert!(state.borrow().pending.borrow().is_empty());

        let unknown_cancel: AnyNotification = serde_json::from_value(serde_json::json!({
            "method": "$/cancelRequest",
            "params": { "id": "missing" }
        }))
        .expect("cancel notification");
        assert_eq!(
            dispatch_notification(state, unknown_cancel).expect("ignore unknown ID"),
            NotificationOutcome::Continue
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hover_definition_and_references_use_the_published_snapshot() {
        let (_, state, uri) = semantic_fixture("let value = 1;\nvalue").await;
        let position = serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": 1, "character": 1 }
        });
        let hover: Option<lsp::Hover> = serde_json::from_value(
            dispatch_request(
                Rc::clone(&state),
                request(50, lsp::request::HoverRequest::METHOD, position.clone()),
            )
            .await
            .expect("hover response"),
        )
        .expect("hover result");
        let hover = hover.expect("hover information");
        assert!(matches!(
            hover.contents,
            lsp::HoverContents::Scalar(lsp::MarkedString::String(ref text))
                if text.contains("value")
        ));
        assert_eq!(hover.range.expect("hover range").start.line, 1);

        let definition: Option<lsp::GotoDefinitionResponse> = serde_json::from_value(
            dispatch_request(
                Rc::clone(&state),
                request(51, lsp::request::GotoDefinition::METHOD, position.clone()),
            )
            .await
            .expect("definition response"),
        )
        .expect("definition result");
        let Some(lsp::GotoDefinitionResponse::Scalar(definition)) = definition else {
            panic!("expected scalar definition");
        };
        assert_eq!(definition.range.start.line, 0);

        let references: Vec<lsp::Location> = serde_json::from_value(
            dispatch_request(
                state,
                request(
                    52,
                    lsp::request::References::METHOD,
                    serde_json::json!({
                        "textDocument": position["textDocument"].clone(),
                        "position": position["position"].clone(),
                        "context": { "includeDeclaration": true }
                    }),
                ),
            )
            .await
            .expect("references response"),
        )
        .expect("references result");
        assert_eq!(references.len(), 2);
        assert_eq!(references[0].range.start.line, 0);
        assert_eq!(references[1].range.start.line, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hover_reports_an_ordinary_expression_type() {
        let (_, state, uri) =
            semantic_fixture("let count = 1 + 2;\nexport { count as output };").await;
        let hover: Option<lsp::Hover> = serde_json::from_value(
            dispatch_request(
                state,
                request(
                    53,
                    lsp::request::HoverRequest::METHOD,
                    serde_json::json!({
                        "textDocument": { "uri": uri },
                        "position": { "line": 0, "character": 12 }
                    }),
                ),
            )
            .await
            .expect("hover response"),
        )
        .expect("hover result");
        assert!(matches!(
            hover.expect("expression hover").contents,
            lsp::HoverContents::Scalar(lsp::MarkedString::String(ref text)) if text == "Int"
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hover_reports_inferred_local_type_schemes() {
        let (_, state, uri) =
            semantic_fixture(
                "let identity = fn(value) { value };\nlet result = identity(1); export { result as output };",
            )
            .await;
        let hover: Option<lsp::Hover> = serde_json::from_value(
            dispatch_request(
                state,
                request(
                    54,
                    lsp::request::HoverRequest::METHOD,
                    serde_json::json!({
                        "textDocument": { "uri": uri },
                        "position": { "line": 1, "character": 15 }
                    }),
                ),
            )
            .await
            .expect("hover response"),
        )
        .expect("hover result");
        assert!(matches!(
            hover.expect("scheme hover").contents,
            lsp::HoverContents::Scalar(lsp::MarkedString::String(ref text))
                if text == "identity: for(A) Fn(A) -> A"
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hover_preserves_static_trait_constraints() {
        let source = r#"trait Display { display: Fn(Self) -> String };
impl Display for Int { display: fn(value) { `int=\{value}` } };
export def render: for(T: Display) Fn(T) -> String = fn(value) {
    Display.display(value)
};
export def output = render(1);"#;
        let (_, state, uri) = semantic_fixture(source).await;
        let hover: Option<lsp::Hover> = serde_json::from_value(
            dispatch_request(
                state,
                request(
                    55,
                    lsp::request::HoverRequest::METHOD,
                    serde_json::json!({
                        "textDocument": { "uri": uri },
                        "position": { "line": 5, "character": 20 }
                    }),
                ),
            )
            .await
            .expect("hover response"),
        )
        .expect("hover result");
        assert!(matches!(
            hover.expect("bounded scheme hover").contents,
            lsp::HoverContents::Scalar(lsp::MarkedString::String(ref text))
                if text == "render: for(T: Display) Fn(T) -> String"
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn protocol_encodings_map_unicode_and_crlf_to_the_same_bytes() {
        let (_, state, uri) = semantic_fixture("\"😀\";\r\n1").await;
        let snapshot = state
            .borrow()
            .workspace
            .as_ref()
            .expect("workspace")
            .published()
            .expect("snapshot");
        let position = |line, character| lsp::TextDocumentPositionParams {
            text_document: lsp::TextDocumentIdentifier { uri: uri.clone() },
            position: lsp::Position::new(line, character),
        };
        assert_eq!(
            request_location(&snapshot, &position(0, 5), PositionEncoding::Utf8)
                .expect("UTF-8 position")
                .start,
            5
        );
        assert_eq!(
            request_location(&snapshot, &position(0, 3), PositionEncoding::Utf16)
                .expect("UTF-16 position")
                .start,
            5
        );
        assert_eq!(
            request_location(&snapshot, &position(0, 2), PositionEncoding::Utf32)
                .expect("UTF-32 position")
                .start,
            5
        );
        for encoding in [
            PositionEncoding::Utf8,
            PositionEncoding::Utf16,
            PositionEncoding::Utf32,
        ] {
            assert_eq!(
                request_location(&snapshot, &position(1, 0), encoding)
                    .expect("position after CRLF")
                    .start,
                9
            );
        }
        assert!(request_location(&snapshot, &position(0, 2), PositionEncoding::Utf16).is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn lsp_completion_maps_struct_fields_and_utf16_text_edits() {
        let source =
            "let face = \"😀\"; let value = {alpha: 1, beta: \"x\"}; let selected = value.alpha";
        let module_source = format!("{source}; export {{ selected as output }};");
        let (_, state, uri) = disk_semantic_fixture(&module_source).await;
        let document = state
            .borrow()
            .workspace
            .as_ref()
            .expect("workspace")
            .published()
            .expect("snapshot")
            .sources()
            .files()
            .find(|file| file.name.ends_with("main.telora"))
            .expect("main source")
            .text()
            .clone();
        let cursor = document
            .position(source.len() as u32, PositionEncoding::Utf16)
            .expect("UTF-16 cursor");
        let list = completion_response(
            state,
            80,
            uri,
            lsp::Position::new(cursor.line, cursor.character),
        )
        .await;
        assert!(!list.is_incomplete);
        assert_eq!(list.items.len(), 1);
        let item = &list.items[0];
        assert_eq!(item.label, "alpha");
        assert_eq!(item.kind, Some(lsp::CompletionItemKind::FIELD));
        assert_eq!(item.detail.as_deref(), Some("Int"));
        let Some(lsp::CompletionTextEdit::Edit(edit)) = &item.text_edit else {
            panic!("expected completion text edit");
        };
        assert_eq!(edit.new_text, "alpha");
        assert_eq!(
            edit.range.end,
            lsp::Position::new(cursor.line, cursor.character)
        );
        assert_eq!(edit.range.end.character - edit.range.start.character, 5);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn lsp_completion_maps_module_exports() {
        let (root, state) = fixture();
        let model = root.join("model.telora");
        let main = root.join("main.telora");
        std::fs::write(&model, "export def alpha = 1; export def beta = \"x\";")
            .expect("write model");
        let source = "import \"./model.telora\" as model; model.alpha";
        std::fs::write(&main, format!("{source}; export {{ model as output }};"))
            .expect("write main");
        initialize_state(&root, &state);
        let workspace = Rc::new(
            Workspace::new(&main, Engine::new(config())).expect("create document workspace"),
        );
        let context = workspace.context();
        workspace.rebuild(&context).await.expect("build snapshot");
        state.borrow_mut().workspace = Some(workspace);
        let uri = lsp::Url::from_file_path(main).expect("main URI");
        let list =
            completion_response(state, 81, uri, lsp::Position::new(0, source.len() as u32)).await;
        assert_eq!(list.items.len(), 1);
        assert_eq!(list.items[0].label, "alpha");
        assert_eq!(list.items[0].kind, Some(lsp::CompletionItemKind::MODULE));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn lsp_completion_supports_an_empty_prefix_in_recovered_source() {
        let (root, state) = fixture();
        let model = root.join("model.telora");
        let main = root.join("main.telora");
        std::fs::write(&model, "export def alpha = 1; export def beta = \"x\";")
            .expect("write model");
        let source = "import \"./model.telora\" as model; model.";
        std::fs::write(&main, format!("{source}\nexport {{ model as output }};"))
            .expect("write main");
        initialize_state(&root, &state);
        let workspace = Rc::new(
            Workspace::new(&main, Engine::new(config())).expect("create document workspace"),
        );
        let context = workspace.context();
        workspace.rebuild(&context).await.expect("build snapshot");
        state.borrow_mut().workspace = Some(workspace);
        let uri = lsp::Url::from_file_path(main).expect("main URI");
        let list =
            completion_response(state, 82, uri, lsp::Position::new(0, source.len() as u32)).await;
        assert_eq!(
            list.items
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );
        for item in list.items {
            let Some(lsp::CompletionTextEdit::Edit(edit)) = item.text_edit else {
                panic!("expected completion text edit");
            };
            assert_eq!(edit.range.start, edit.range.end);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn completion_observes_cancellation_and_stale_revisions() {
        let source = "let value = {alpha: 1, beta: 2}; value.alpha";
        let (path, state, uri) = disk_semantic_fixture(source).await;
        let completion_request = || {
            request(
                82,
                lsp::request::Completion::METHOD,
                serde_json::json!({
                    "textDocument": { "uri": uri.clone() },
                    "position": { "line": 0, "character": source.len() }
                }),
            )
        };
        let mut cancelled = Box::pin(dispatch_request(Rc::clone(&state), completion_request()));
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        assert!(matches!(
            cancelled.as_mut().poll(&mut context),
            Poll::Pending
        ));
        let cancel: AnyNotification = serde_json::from_value(serde_json::json!({
            "method": "$/cancelRequest",
            "params": { "id": 82 }
        }))
        .expect("cancel notification");
        dispatch_notification(Rc::clone(&state), cancel).expect("cancel completion");
        assert_eq!(
            cancelled.await.expect_err("cancelled completion").code,
            ErrorCode::REQUEST_CANCELLED
        );

        let mut stale = Box::pin(dispatch_request(Rc::clone(&state), completion_request()));
        assert!(matches!(stale.as_mut().poll(&mut context), Poll::Pending));
        state
            .borrow()
            .workspace
            .as_ref()
            .expect("workspace")
            .open(
                path,
                DocumentVersion(1),
                "let value = {alpha: 2}; value.alpha",
            )
            .expect("advance workspace revision");
        assert_eq!(
            stale.await.expect_err("stale completion").code,
            ErrorCode::CONTENT_MODIFIED
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn transport_eof_cancels_pending_tokens() {
        let (_, state, main_loop) = fixture_loop();
        let token = CancellationToken::default();
        state
            .borrow()
            .pending
            .borrow_mut()
            .insert(RequestId::Number(61), token.clone());
        let mut output = futures::io::Cursor::new(Vec::new());
        let error = main_loop
            .run_buffered(futures::io::Cursor::new(Vec::new()), &mut output)
            .await
            .expect_err("EOF terminates transport");
        assert!(matches!(error, async_lsp::Error::Eof));
        assert!(token.is_cancelled());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn exit_before_shutdown_is_a_protocol_error() {
        let (root, _, main_loop) = fixture_loop();
        let _ = root;
        let input = frame(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "exit"
        }));
        let mut output = futures::io::Cursor::new(Vec::new());
        let error = main_loop
            .run_buffered(futures::io::Cursor::new(input), &mut output)
            .await
            .expect_err("early exit is abnormal");
        assert!(matches!(error, async_lsp::Error::Protocol(_)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn diagnostics_publish_current_errors_and_an_empty_clear() {
        let (root, state, main_loop) = fixture_loop();
        initialize_state(&root, &state);
        let path = root.join("main.telora");
        let workspace = Rc::new(
            Workspace::new(&path, Engine::new(config())).expect("create document workspace"),
        );
        workspace
            .open(
                &path,
                DocumentVersion(1),
                "def first = 1 / 0; def second = 2 / 0; export def output = 0;",
            )
            .expect("open invalid source");
        {
            let mut state = state.borrow_mut();
            state.workspace = Some(Rc::clone(&workspace));
            state.documents.insert(path.clone(), 1);
        }
        let context = workspace.context();
        let invalid = workspace.rebuild(&context).await.expect("invalid snapshot");
        assert_eq!(
            invalid
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.message.contains("division by zero"))
                .count(),
            2
        );
        publish_diagnostics(&state, &invalid).await;

        workspace
            .change(
                &path,
                DocumentVersion(1),
                DocumentVersion(2),
                &[TextEdit::Full("export def output = 1;".to_owned())],
            )
            .expect("fix source");
        state.borrow_mut().documents.insert(path, 2);
        let context = workspace.context();
        let valid = workspace.rebuild(&context).await.expect("valid snapshot");
        assert!(valid.diagnostics().is_empty());
        publish_diagnostics(&state, &valid).await;

        let mut input = Vec::new();
        input.extend(frame(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 70,
            "method": "shutdown"
        })));
        input.extend(frame(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "exit"
        })));
        let mut output = futures::io::Cursor::new(Vec::new());
        main_loop
            .run_buffered(futures::io::Cursor::new(input), &mut output)
            .await
            .expect("flush diagnostics");
        let output = String::from_utf8(output.into_inner()).expect("UTF-8 output");
        assert_eq!(output.matches("textDocument/publishDiagnostics").count(), 2);
        assert!(output.contains("\"version\":1"));
        assert!(output.contains("\"version\":2"));
        assert!(output.contains("\"diagnostics\":[]"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn main_loop_completes_initialize_shutdown_and_exit() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("telora-lsp-loop-test-{unique}"));
        std::fs::create_dir_all(&root).expect("create fixture root");
        let uri = lsp::Url::from_directory_path(&root).expect("fixture URI");
        let mut input = Vec::new();
        input.extend(frame(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "capabilities": {}, "rootUri": uri }
        })));
        input.extend(frame(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {}
        })));
        input.extend(frame(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "shutdown"
        })));
        input.extend(frame(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "exit"
        })));

        let (main_loop, _) =
            async_lsp::MainLoop::new_server(move |client| Server::new(root, config(), client));
        let mut output = futures::io::Cursor::new(Vec::new());
        main_loop
            .run_buffered(futures::io::Cursor::new(input), &mut output)
            .await
            .expect("run protocol lifecycle");
        let output = String::from_utf8(output.into_inner()).expect("UTF-8 protocol output");
        assert!(output.contains("\"id\":1"));
        assert!(output.contains("\"positionEncoding\":\"utf-16\""));
        assert!(output.contains("\"id\":2"));
        assert!(output.contains("\"result\":null"));
    }
