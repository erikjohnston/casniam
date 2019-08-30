use futures::compat::Future01CompatExt;
use futures::FutureExt;
use http::Uri;
use std::collections::BTreeMap;
use std::future::Future;
use trust_dns_resolver;

use std::net::IpAddr;

pub struct Endpoint {
    pub host: String,
    pub port: u16,

    pub host_header: String,
    pub tls_name: String,
}

#[derive(Clone)]
pub struct MatrixResolver {
    resolver: trust_dns_resolver::AsyncResolver,
    http_client: awc::Client,
}

impl MatrixResolver {
    pub fn new(
        http_client: awc::Client,
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
        let mut authority = uri.authority_part().expect("URI has no authority");
        let mut host = uri.host().expect("URI has no host");
        let mut port = uri.port_u16();

        // If a literal IP or includes port then we shortcircuit.
        if host.parse::<IpAddr>().is_ok() || port.is_some() {
            return Ok(vec![Endpoint {
                host: host.to_string(),
                port: port.unwrap_or(8448),

                host_header: authority.to_string(),
                tls_name: host.to_string(),
            }]);
        }

        // TODO: Do lookup
        if let Some(server) = get_well_known(&self.http_client, host).await {}

        // If a literal IP or includes port then we shortcircuit.
        if host.parse::<IpAddr>().is_ok() || port.is_some() {
            return Ok(vec![Endpoint {
                host: host.to_string(),
                port: port.unwrap_or(8448),

                host_header: authority.to_string(),
                tls_name: host.to_string(),
            }]);
        }

        let records = self.resolver.lookup_srv(host).compat().await?;

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

async fn get_well_known(
    http_client: &awc::Client,
    host: &str,
) -> Option<WellKnownServer> {
    http_client
        .get(format!("https://{}/.well-known/matrix/server", host))
        .send()
        .compat()
        .await
        .ok()?
        .json()
        .compat()
        .await
        .ok()?
}

#[derive(Deserialize)]
struct WellKnownServer {
    #[serde(rename = "m.server")]
    server: String,
}
