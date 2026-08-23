use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::{Context, Poll};

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Revision(pub u64);

#[derive(Clone, Debug, Default)]
pub struct RevisionClock {
    current: Arc<AtomicU64>,
}

impl RevisionClock {
    pub fn current(&self) -> Revision {
        Revision(self.current.load(Ordering::Acquire))
    }

    pub fn advance(&self) -> Revision {
        Revision(self.current.fetch_add(1, Ordering::AcqRel) + 1)
    }
}

#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Clone, Debug)]
pub struct QueryContext {
    revision: Revision,
    clock: RevisionClock,
    cancellation: CancellationToken,
}

impl QueryContext {
    pub fn new(revision: Revision, clock: RevisionClock, cancellation: CancellationToken) -> Self {
        Self {
            revision,
            clock,
            cancellation,
        }
    }

    pub fn current(clock: RevisionClock) -> Self {
        let revision = clock.current();
        Self::new(revision, clock, CancellationToken::default())
    }

    pub const fn revision(&self) -> Revision {
        self.revision
    }

    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    pub fn check(&self) -> Result<(), QueryError> {
        if self.cancellation.is_cancelled() {
            return Err(QueryError::Cancelled);
        }
        let current = self.clock.current();
        if current != self.revision {
            return Err(QueryError::StaleRevision {
                requested: self.revision,
                current,
            });
        }
        Ok(())
    }

    pub async fn checkpoint(&self) -> Result<(), QueryError> {
        self.check()?;
        YieldOnce::default().await;
        self.check()
    }

    pub fn ensure_snapshot(&self, revision: Revision) -> Result<(), QueryError> {
        self.check()?;
        if revision != self.revision {
            return Err(QueryError::SnapshotRevision {
                requested: self.revision,
                snapshot: revision,
            });
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueryError {
    Cancelled,
    StaleRevision {
        requested: Revision,
        current: Revision,
    },
    SnapshotRevision {
        requested: Revision,
        snapshot: Revision,
    },
}

impl fmt::Display for QueryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("query cancelled"),
            Self::StaleRevision { requested, current } => write!(
                formatter,
                "query revision {} is stale; current revision is {}",
                requested.0, current.0
            ),
            Self::SnapshotRevision {
                requested,
                snapshot,
            } => write!(
                formatter,
                "query revision {} does not match snapshot revision {}",
                requested.0, snapshot.0
            ),
        }
    }
}

impl std::error::Error for QueryError {}

#[derive(Default)]
struct YieldOnce {
    yielded: bool,
}

impl Future for YieldOnce {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if self.yielded {
            Poll::Ready(())
        } else {
            self.yielded = true;
            context.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

#[cfg(test)]
#[path = "query/tests/mod.rs"]
mod tests;
