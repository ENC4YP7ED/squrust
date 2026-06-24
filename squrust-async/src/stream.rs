//! A lazy stream of typed rows.

use std::pin::Pin;
use std::task::{Context, Poll};

use futures::Stream;

use crate::error::Result;

/// A `Stream` of typed rows, yielded lazily as the underlying executor advances.
pub struct RowStream<T> {
    inner: Pin<Box<dyn Stream<Item = Result<T>> + Send>>,
}

impl<T> RowStream<T> {
    pub(crate) fn new(stream: impl Stream<Item = Result<T>> + Send + 'static) -> Self {
        RowStream {
            inner: Box::pin(stream),
        }
    }
}

impl<T> Stream for RowStream<T> {
    type Item = Result<T>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}
