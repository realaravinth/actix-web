mod dispatcher;
mod service;

pub use self::service::H3Service;

use std::rc::Rc;

use futures_intrusive::sync::LocalMutex;
use h3::server::RequestStream;

/// HTTP/2 peer stream.
pub struct Payload {
    stream: Rc<LocalMutex<RequestStream>>,
}

impl Payload {
    pub(crate) fn new(stream: RecvStream) -> Self {
        Self { stream }
    }
}

impl Stream for Payload {
    type Item = Result<Bytes, PayloadError>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        match ready!(Pin::new(&mut this.stream).poll_data(cx)) {
            Some(Ok(chunk)) => {
                let len = chunk.len();

                match this.stream.flow_control().release_capacity(len) {
                    Ok(()) => Poll::Ready(Some(Ok(chunk))),
                    Err(err) => Poll::Ready(Some(Err(err.into()))),
                }
            }
            Some(Err(err)) => Poll::Ready(Some(Err(err.into()))),
            None => Poll::Ready(None),
        }
    }
}
