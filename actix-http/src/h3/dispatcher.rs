use std::task::{Context, Poll};
use std::{cmp, future::Future, marker::PhantomData, net, pin::Pin, rc::Rc};

use actix_service::Service;
use bytes::{Bytes, BytesMut};
use futures_core::future::LocalBoxFuture;
use futures_core::ready;
use h3::server;
use h3_quinn::quinn::{self, crypto::rustls::TlsSession};
use http::header::{HeaderValue, CONNECTION, CONTENT_LENGTH, DATE, TRANSFER_ENCODING};
use log::{error, trace};
use pin_project_lite::pin_project;

use crate::body::{Body, BodySize, MessageBody};
use crate::config::ServiceConfig;
use crate::error::{DispatchError, Error};
use crate::message::ResponseHead;
use crate::payload::Payload;
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
    S: Service<Request>,
    S::Error: Into<Error>,
    S::Response: Into<Response<B>>,
    B: MessageBody,
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

                actix_rt::spawn(async move {
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

                    let res = this.flow.call(req).await.unwrap();

                    let res = http::Response::builder()
                        .status(http::StatusCode::OK)
                        .body(())
                        .unwrap();

                    stream.send_response(res).await.unwrap();

                    stream.finish().await.unwrap();
                });
            }

            Ok(())
        })
    }
}
