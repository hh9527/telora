    #[test]
    fn distinguishes_explicit_cancellation_from_stale_revisions() {
        let clock = RevisionClock::default();
        let cancellation = CancellationToken::default();
        let context = QueryContext::new(clock.current(), clock.clone(), cancellation.clone());
        assert_eq!(block_on(context.checkpoint()), Ok(()));

        cancellation.cancel();
        assert_eq!(block_on(context.checkpoint()), Err(QueryError::Cancelled));

        let context = QueryContext::current(clock.clone());
        let requested = context.revision();
        let current = clock.advance();
        assert_eq!(
            block_on(context.checkpoint()),
            Err(QueryError::StaleRevision { requested, current })
        );
    }
