use std::{
    pin::Pin,
    task::{Context, Poll},
};

use actix_service::{Service, Transform};
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::Error;
use futures::{
    future::{ok, FutureExt, Ready, TryFutureExt},
    Future,
};
use tracing::field;
use tracing::Span;
use tracing_futures::Instrument;

#[derive(Clone, Debug)]
pub struct TracingService<S> {
    inner: S,
}

impl<S> TracingService<S> {
    pub fn new(inner: S) -> Self {
        TracingService { inner }
    }
}

impl<S, B> Service for TracingService<S>
where
    S: Service<
        Request = ServiceRequest,
        Response = ServiceResponse<B>,
        Error = Error,
    >,
    S::Future: 'static,
{
    type Request = ServiceRequest;
    type Response = ServiceResponse<B>;
    type Error = S::Error;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>>>>;

    fn poll_ready(
        &mut self,
        ctx: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(ctx)
    }

    fn call(&mut self, req: ServiceRequest) -> Self::Future {
        let method = req.method().to_string();
        let uri = req.uri().to_string();
        let peer = req
            .peer_addr()
            .map(|p| p.ip().to_string())
            .unwrap_or_else(|| "-".to_string());

        let fut = self
            .inner
            .call(req)
            .inspect_ok(|resp: &ServiceResponse<B>| {
                Span::current().record("status", &resp.status().as_u16());
            })
            .inspect_err(|err: &Error| {
                Span::current().record("error", &&err.to_string()[..]);
            })
            .instrument(tracing::info_span!(
                "inbound_request",
                method = &method as &str,
                uri = &uri as &str,
                peer = &peer as &str,
                status = field::Empty,
                error = field::Empty,
                span.kind = "server",
            ));

        fut.boxed_local()
    }
}

pub struct TracingTransform;

impl<S, B> Transform<S> for TracingTransform
where
    S: Service<
        Request = ServiceRequest,
        Response = ServiceResponse<B>,
        Error = Error,
    >,
    S::Future: 'static,
{
    type Request = ServiceRequest;
    type Response = ServiceResponse<B>;
    type Error = Error;
    type InitError = ();
    type Transform = TracingService<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ok(TracingService::new(service))
    }
}
