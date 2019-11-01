use failure::Error;
use futures::compat::Future01CompatExt;
use futures::FutureExt;
use http::Uri;
use hyper;
use hyper::client::connect::{Connect, Connected, Destination, HttpConnector};
use hyper::Client;
use hyper_tls::HttpsConnector;
use native_tls::TlsConnector;
use tokio::net::TcpStream;
use tokio_tls::{TlsConnector as AsyncTlsConnector, TlsStream};
use trust_dns_resolver;

use std::collections::BTreeMap;
use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::str::FromStr;

pub struct Endpoint {
    pub host: String,
    pub port: u16,

    pub host_header: String,
    pub tls_name: String,
}

#[derive(Clone)]
pub struct MatrixResolver {
    resolver: trust_dns_resolver::AsyncResolver,
    http_client: Client<HttpsConnector<HttpConnector>>,
}

impl MatrixResolver {
    pub fn new(
    ) -> Result<(MatrixResolver, impl Future<Output = ()>), failure::Error>
    {
        let http_client =
            hyper::Client::builder().build(HttpsConnector::new()?);

        MatrixResolver::with_client(http_client)
    }

    pub fn with_client(
        http_client: Client<HttpsConnector<HttpConnector>>,
    ) -> Result<(MatrixResolver, impl Future<Output = ()>), failure::Error>
    {
        let (resolver, background_future) =
            trust_dns_resolver::AsyncResolver::from_system_conf()?;

        let fut = background_future.compat().map(|_| ());

        Ok((
            MatrixResolver {
                resolver,
                http_client,
            },
            fut,
        ))
    }

    /// Does SRV lookup
    pub async fn resolve_server_name_from_uri(
        &self,
        uri: &Uri,
    ) -> Result<Vec<Endpoint>, failure::Error> {
        let host = uri.host().expect("URI has no host").to_string();
        let port = uri.port_u16();

        self.resolve_server_name_from_host_port(host, port).await
    }

    pub async fn resolve_server_name_from_host_port(
        &self,
        mut host: String,
        mut port: Option<u16>,
    ) -> Result<Vec<Endpoint>, failure::Error> {
        let mut authority = if let Some(p) = port {
            format!("{}:{}", host, p)
        } else {
            host.to_string()
        };

        // If a literal IP or includes port then we shortcircuit.
        if host.parse::<IpAddr>().is_ok() || port.is_some() {
            return Ok(vec![Endpoint {
                host: host.to_string(),
                port: port.unwrap_or(8448),

                host_header: authority.to_string(),
                tls_name: host.to_string(),
            }]);
        }

        // Do well-known delegation lookup.
        if let Some(server) = get_well_known(&self.http_client, &host).await {
            let a = http::uri::Authority::from_str(&server.server)?;
            host = a.host().to_string();
            port = a.port_u16();
            authority = a.to_string();
        }

        // If a literal IP or includes port then we shortcircuit.
        if host.parse::<IpAddr>().is_ok() || port.is_some() {
            return Ok(vec![Endpoint {
                host: host.clone(),
                port: port.unwrap_or(8448),

                host_header: authority.to_string(),
                tls_name: host.clone(),
            }]);
        }

        let records = self.resolver.lookup_srv(host.as_ref()).compat().await?;

        let mut priority_map: BTreeMap<u16, Vec<_>> = BTreeMap::new();

        let mut count = 0;
        for record in records {
            count += 1;
            let priority = record.priority();
            priority_map.entry(priority).or_default().push(record);
        }

        let mut results = Vec::with_capacity(count);

        for (_priority, records) in priority_map {
            // TODO: Correctly shuffle records
            results.extend(records.into_iter().map(|record| Endpoint {
                host: record.target().to_utf8(),
                port: record.port(),

                host_header: host.to_string(),
                tls_name: host.to_string(),
            }))
        }

        Ok(results)
    }
}

async fn get_well_known<C: Connect + 'static>(
    http_client: &Client<C>,
    host: &str,
) -> Option<WellKnownServer> {
    let uri = hyper::Uri::builder()
        .scheme("https")
        .authority(host)
        .path_and_query("/.well-known/matrix/server")
        .build()
        .ok()?;

    let mut body = http_client.get(uri).await.ok()?.into_body();

    let mut vec = Vec::new();
    while let Some(next) = body.next().await {
        let chunk = next.ok()?;
        vec.extend(chunk);
    }

    serde_json::from_slice(&vec).ok()?
}

#[derive(Deserialize)]
struct WellKnownServer {
    #[serde(rename = "m.server")]
    server: String,
}

pub struct MatrixConnector {
    resolver: MatrixResolver,
}

impl MatrixConnector {
    pub fn with_resolver(resolver: MatrixResolver) -> MatrixConnector {
        MatrixConnector { resolver }
    }
}

impl Connect for MatrixConnector {
    type Transport = TlsStream<TcpStream>;
    type Error = Error;
    type Future = Pin<
        Box<
            dyn Future<
                    Output = Result<(Self::Transport, Connected), Self::Error>,
                > + Send,
        >,
    >;

    fn connect(&self, dst: Destination) -> Self::Future {
        let resolver = self.resolver.clone();
        async move {
            let endpoints = resolver
                .resolve_server_name_from_host_port(
                    dst.host().to_string(),
                    dst.port(),
                )
                .await?;

            for endpoint in endpoints {
                let tcp =
                    TcpStream::connect((&endpoint.host as &str, endpoint.port))
                        .await?;

                let connector: AsyncTlsConnector =
                    if dst.host().contains("localhost") {
                        TlsConnector::builder()
                            .danger_accept_invalid_certs(true)
                            .build()?
                            .into()
                    } else {
                        TlsConnector::new().unwrap().into()
                    };

                let tls = connector.connect(&endpoint.tls_name, tcp).await?;

                let connected = Connected::new();

                return Ok((tls, connected));
            }

            Err(format_err!("help"))
        }
        .boxed()
    }
}
