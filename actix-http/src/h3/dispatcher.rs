use std::{marker::PhantomData, net, rc::Rc};

use actix_service::Service;
use actix_utils::future::poll_fn;
use bytes::{Bytes, BytesMut};
use futures_core::future::LocalBoxFuture;
use h3::quic::SendStream;
use h3::server::{self, RequestStream};
use h3_quinn::quinn::crypto::rustls::TlsSession;
use http::header::{HeaderValue, CONNECTION, CONTENT_LENGTH, DATE, TRANSFER_ENCODING};

use crate::body::{BodySize, MessageBody};
use crate::config::ServiceConfig;
use crate::error::{DispatchError, Error};
use crate::message::ResponseHead;
use crate::request::Request;
use crate::response::Response;
use crate::service::HttpFlow;
use crate::OnConnectData;

pub(super) type Connection = server::Connection<h3_quinn::Connection<TlsSession>>;

/// Dispatcher for HTTP/3 protocol.
pub struct Dispatcher<S, B>
where
    S: Service<Request>,
    B: MessageBody,
{
    _phantom: PhantomData<(S, B)>,
}

impl<S, B> Dispatcher<S, B>
where
    S: Service<Request> + 'static,
    S::Error: Into<Error>,
    S::Response: Into<Response<B>>,

    B: MessageBody,
    B::Error: Into<Error>,
{
    pub(super) fn new(
        flow: Rc<HttpFlow<S, (), ()>>,
        mut connection: Connection,
        mut on_connect_data: OnConnectData,
        config: ServiceConfig,
        peer_addr: Option<net::SocketAddr>,
    ) -> LocalBoxFuture<'static, Result<(), DispatchError>> {
        Box::pin(async move {
            while let Some(res) = connection.accept().await.transpose() {
                let (req, mut stream) = res?;

                // How to collect?
                // while let Some(bytes) = stream.recv_data().await? {
                //
                // }

                let (parts, _) = req.into_parts();
                let mut req = Request::new();

                let head = req.head_mut();
                head.uri = parts.uri;
                head.method = parts.method;
                head.version = parts.version;
                head.headers = parts.headers.into();
                head.peer_addr = peer_addr;

                // merge on_connect_ext data into request extensions
                on_connect_data.merge_into(&mut req);

                let fut = flow.service.call(req);
                let config = config.clone();

                actix_rt::spawn(async move {
                    let res = match fut.await {
                        Ok(res) => {
                            let (res, body) = res.into().replace_body(());
                            handle_response(res, body, &mut stream, config).await
                        }
                        Err(err) => {
                            let (res, body) =
                                Response::from_error(err.into()).replace_body(());
                            handle_response(res, body, &mut stream, config).await
                        }
                    };

                    if let Err(err) = res {
                        error!("{:?}", err);
                    }

                    // TODO: Should call stream finish after an DispatchError?
                    if let Err(err) = stream.finish().await {
                        error!("Error finishing RequestStream: {:?}", err);
                    }
                });
            }

            Ok(())
        })
    }
}

async fn handle_response<B, C>(
    res: Response<()>,
    body: B,
    stream: &mut RequestStream<C>,
    config: ServiceConfig,
) -> Result<(), DispatchError>
where
    B: MessageBody,
    B::Error: Into<Error>,

    C: SendStream<Bytes>,
{
    let mut size = body.size();
    let res = prepare_response(&config, res.head(), &mut size);

    stream.send_response(res).await?;

    actix_rt::pin!(body);

    while let Some(res) = poll_fn(|cx| body.as_mut().poll_next(cx)).await {
        let bytes = res.map_err(|err| err.into())?;
        stream.send_data(bytes).await?
    }

    Ok(())
}

fn prepare_response(
    config: &ServiceConfig,
    head: &ResponseHead,
    size: &mut BodySize,
) -> http::Response<()> {
    let mut has_date = false;
    let mut skip_len = size != &BodySize::Stream;

    let mut res = http::Response::new(());
    *res.status_mut() = head.status;
    *res.version_mut() = http::Version::HTTP_3;

    // Content length
    match head.status {
        http::StatusCode::NO_CONTENT
        | http::StatusCode::CONTINUE
        | http::StatusCode::PROCESSING => *size = BodySize::None,
        http::StatusCode::SWITCHING_PROTOCOLS => {
            skip_len = true;
            *size = BodySize::Stream;
        }
        _ => {}
    }

    let _ = match size {
        BodySize::None | BodySize::Stream => None,
        BodySize::Empty => res
            .headers_mut()
            .insert(CONTENT_LENGTH, HeaderValue::from_static("0")),
        BodySize::Sized(len) => {
            let mut buf = itoa::Buffer::new();

            res.headers_mut().insert(
                CONTENT_LENGTH,
                HeaderValue::from_str(buf.format(*len)).unwrap(),
            )
        }
    };

    // copy headers
    for (key, value) in head.headers.iter() {
        match *key {
            // TODO: consider skipping other headers according to:
            //       https://tools.ietf.org/html/rfc7540#section-8.1.2.2
            // omit HTTP/1.x only headers
            CONNECTION | TRANSFER_ENCODING => continue,
            CONTENT_LENGTH if skip_len => continue,
            DATE => has_date = true,
            _ => {}
        }

        res.headers_mut().append(key, value.clone());
    }

    // set date header
    if !has_date {
        let mut bytes = BytesMut::with_capacity(29);
        config.set_date_header(&mut bytes);
        res.headers_mut().insert(
            DATE,
            // SAFETY: serialized date-times are known ASCII strings
            unsafe { HeaderValue::from_maybe_shared_unchecked(bytes.freeze()) },
        );
    }

    res
}
