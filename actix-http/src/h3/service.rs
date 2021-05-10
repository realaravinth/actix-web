use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::{net, rc::Rc};

use actix_server_alt::net::{UdpConnecting, UdpStream};
use actix_service::{
    fn_factory, fn_service, IntoServiceFactory, Service, ServiceFactory,
    ServiceFactoryExt as _,
};
use actix_utils::future::ready;
use futures_core::{future::LocalBoxFuture, ready};
use h3::server;
use log::error;

use crate::body::MessageBody;
use crate::config::ServiceConfig;
use crate::error::{DispatchError, Error};
use crate::request::Request;
use crate::response::Response;
use crate::service::HttpFlow;
use crate::{ConnectCallback, OnConnectData};

use super::dispatcher::{Connection, Dispatcher};

/// `ServiceFactory` implementation for HTTP/3 transport
pub struct H3Service<S, B> {
    srv: S,
    cfg: ServiceConfig,
    on_connect_ext: Option<Rc<ConnectCallback<UdpStream>>>,
    _phantom: PhantomData<B>,
}

impl<S, B> H3Service<S, B>
where
    S: ServiceFactory<Request, Config = ()>,
    S::Error: Into<Error> + 'static,
    S::Response: Into<Response<B>> + 'static,
    <S::Service as Service<Request>>::Future: 'static,

    B: MessageBody + 'static,
    B::Error: Into<Error>,
{
    /// Create new `H2Service` instance with config.
    pub(crate) fn with_config<F: IntoServiceFactory<S, Request>>(
        cfg: ServiceConfig,
        service: F,
    ) -> Self {
        H3Service {
            cfg,
            on_connect_ext: None,
            srv: service.into_factory(),
            _phantom: PhantomData,
        }
    }

    /// Set on connect callback.
    pub(crate) fn on_connect_ext(
        mut self,
        f: Option<Rc<ConnectCallback<UdpStream>>>,
    ) -> Self {
        self.on_connect_ext = f;
        self
    }
}

impl<S, B> H3Service<S, B>
where
    S: ServiceFactory<Request, Config = ()>,
    S::Future: 'static,
    S::Error: Into<Error> + 'static,
    S::Response: Into<Response<B>> + 'static,
    <S::Service as Service<Request>>::Future: 'static,

    B: MessageBody + 'static,
    B::Error: Into<Error>,
{
    pub fn rustls(
        self,
    ) -> impl ServiceFactory<
        UdpStream,
        Config = (),
        Response = (),
        Error = DispatchError,
        InitError = S::InitError,
    > {
        fn_factory(|| {
            ready(Ok::<_, S::InitError>(fn_service(|io: UdpStream| async {
                let peer_addr = Some(io.peer_addr());
                Ok::<_, DispatchError>((io, peer_addr))
            })))
        })
        .and_then(self)
    }
}

impl<S, B> ServiceFactory<(UdpStream, Option<net::SocketAddr>)> for H3Service<S, B>
where
    S: ServiceFactory<Request, Config = ()>,
    S::Future: 'static,
    S::Error: Into<Error> + 'static,
    S::Response: Into<Response<B>> + 'static,
    <S::Service as Service<Request>>::Future: 'static,

    B: MessageBody + 'static,
    B::Error: Into<Error>,
{
    type Response = ();
    type Error = DispatchError;
    type Config = ();
    type Service = H3ServiceHandler<S::Service, B>;
    type InitError = S::InitError;
    type Future = LocalBoxFuture<'static, Result<Self::Service, Self::InitError>>;

    fn new_service(&self, _: ()) -> Self::Future {
        let service = self.srv.new_service(());
        let cfg = self.cfg.clone();
        let on_connect_ext = self.on_connect_ext.clone();

        Box::pin(async move {
            let service = service.await?;
            Ok(H3ServiceHandler::new(cfg, on_connect_ext, service))
        })
    }
}

/// `Service` implementation for HTTP/3 transport
pub struct H3ServiceHandler<S, B>
where
    S: Service<Request>,
{
    flow: Rc<HttpFlow<S, (), ()>>,
    cfg: ServiceConfig,
    on_connect_ext: Option<Rc<ConnectCallback<UdpStream>>>,
    _phantom: PhantomData<B>,
}

impl<S, B> H3ServiceHandler<S, B>
where
    S: Service<Request>,
    S::Error: Into<Error> + 'static,
    S::Future: 'static,
    S::Response: Into<Response<B>> + 'static,
    B: MessageBody + 'static,
{
    fn new(
        cfg: ServiceConfig,
        on_connect_ext: Option<Rc<ConnectCallback<UdpStream>>>,
        service: S,
    ) -> H3ServiceHandler<S, B> {
        H3ServiceHandler {
            flow: HttpFlow::new(service, (), None),
            cfg,
            on_connect_ext,
            _phantom: PhantomData,
        }
    }
}

impl<S, B> Service<(UdpStream, Option<net::SocketAddr>)> for H3ServiceHandler<S, B>
where
    S: Service<Request>,
    S::Error: Into<Error> + 'static,
    S::Future: 'static,
    S::Response: Into<Response<B>> + 'static,
    B: MessageBody + 'static,
    B::Error: Into<Error>,
{
    type Response = ();
    type Error = DispatchError;
    type Future = H3ServiceHandlerResponse<S, B>;

    fn poll_ready(&self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.flow.service.poll_ready(cx).map_err(|e| {
            let e = e.into();
            error!("Service readiness error: {:?}", e);
            DispatchError::Service(e)
        })
    }

    fn call(&self, (io, addr): (UdpStream, Option<net::SocketAddr>)) -> Self::Future {
        let on_connect_data =
            OnConnectData::from_io(&io, self.on_connect_ext.as_deref());

        let connecting = io.connecting();

        H3ServiceHandlerResponse {
            state: State::Connecting(
                Some(self.flow.clone()),
                Some(self.cfg.clone()),
                addr,
                on_connect_data,
                connecting,
            ),
        }
    }
}

use pin_project_lite::pin_project;

pin_project! {
    pub struct H3ServiceHandlerResponse<S, B>
    where
        S: Service<Request>,
        S::Error: Into<Error>,
        S::Error: 'static,
        S::Future: 'static,
        S::Response: Into<Response<B>>,
        S::Response: 'static,
        B: MessageBody,
        B: 'static,
    {
        state: State<S, B>,
    }
}

enum State<S, B>
where
    S: Service<Request>,
    S::Future: 'static,

    B: MessageBody,
{
    Connected(
        LocalBoxFuture<'static, Result<(), DispatchError>>,
        PhantomData<B>,
    ),
    Connecting(
        Option<Rc<HttpFlow<S, (), ()>>>,
        Option<ServiceConfig>,
        Option<net::SocketAddr>,
        OnConnectData,
        UdpConnecting,
    ),
    Connecting2(
        Option<Rc<HttpFlow<S, (), ()>>>,
        Option<ServiceConfig>,
        Option<net::SocketAddr>,
        OnConnectData,
        LocalBoxFuture<'static, Result<Connection, h3::Error>>,
    ),
}

impl<S, B> Future for H3ServiceHandlerResponse<S, B>
where
    S: Service<Request>,
    S::Error: Into<Error> + 'static,
    S::Future: 'static,
    S::Response: Into<Response<B>> + 'static,
    B: MessageBody,
    B::Error: Into<Error>,
{
    type Output = Result<(), DispatchError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.as_mut().project();
        match this.state {
            State::Connected(ref mut disp, ..) => disp.as_mut().poll(cx),
            State::Connecting(
                ref mut flow,
                ref mut config,
                ref mut peer_addr,
                ref mut on_connect_data,
                ref mut connecting,
            ) => match ready!(Pin::new(connecting).poll(cx)) {
                Ok(conn) => {
                    let conn = h3_quinn::Connection::new(conn);
                    let connecting = Box::pin(server::Connection::new(conn));

                    *this.state = State::Connecting2(
                        flow.take(),
                        config.take(),
                        peer_addr.take(),
                        std::mem::take(on_connect_data),
                        connecting,
                    );
                    self.poll(cx)
                }
                Err(err) => {
                    trace!("Http/3 connecting error: {}", err);
                    Poll::Ready(Err(DispatchError::H3Connection(err)))
                }
            },
            State::Connecting2(
                ref mut flow,
                ref mut config,
                ref peer_addr,
                ref mut on_connect_data,
                ref mut connecting,
            ) => match ready!(Pin::new(connecting).poll(cx)) {
                Ok(connection) => {
                    let dispatcher = Dispatcher::new(
                        flow.take().unwrap(),
                        connection,
                        std::mem::take(on_connect_data),
                        config.take().unwrap(),
                        *peer_addr,
                    );

                    *this.state = State::Connected(dispatcher, PhantomData);
                    self.poll(cx)
                }
                Err(err) => {
                    trace!("Http/3 connecting error: {}", err);
                    Poll::Ready(Err(DispatchError::H3(err)))
                }
            },
        }
    }
}
