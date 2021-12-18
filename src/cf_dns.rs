use std::collections::HashSet;
use std::env;
use std::fmt::{self, Debug, Display, Formatter};
use std::iter::FromIterator;
use std::net::IpAddr;
use std::sync::Arc;

use anyhow::Result;
use cloudflare::endpoints::dns::{
    CreateDnsRecord, CreateDnsRecordParams, DeleteDnsRecord, DnsContent, DnsRecord, ListDnsRecords,
    ListDnsRecordsParams,
};
use cloudflare::endpoints::zone::{ListZones, ListZonesParams, Zone};
use cloudflare::framework::async_api::{ApiClient, Client};
use cloudflare::framework::auth::Credentials;
use cloudflare::framework::response::ApiFailure;
use cloudflare::framework::{Environment, HttpApiClientConfig};
use futures_util::TryFutureExt;
use http::StatusCode;
use tracing::{error, info, info_span, instrument, Instrument};

const DEFAULT_TTL: Option<u32> = Some(120);

#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum RecordKind {
    A,
    AAAA,
}

impl Display for RecordKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Debug::fmt(self, f)
    }
}

#[derive(Clone)]
pub struct CfDns {
    client: Arc<Client>,
}

impl Debug for CfDns {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("CfDns")
            .field("client", &"Client".to_string())
            .finish()
    }
}

impl CfDns {
    pub async fn new() -> Result<Self> {
        let cred = create_credentials();

        let client = Client::new(
            cred,
            HttpApiClientConfig::default(),
            Environment::Production,
        )?;

        Ok(Self {
            client: Arc::new(client),
        })
    }

    #[instrument(err)]
    pub async fn get_dns_record(
        &self,
        name: &str,
        zone: &str,
        kind: RecordKind,
    ) -> Result<Vec<IpAddr>> {
        let zone_id = self.get_zone_id(zone).await?;

        let ip_list = self
            .get_dns_record_with_zone_id(name, &zone_id, kind)
            .await?;

        info!(name, zone, %zone_id, %kind, ?ip_list, "get dns records success");

        Ok(ip_list)
    }

    #[instrument(err)]
    pub async fn set_dns_record(
        &self,
        name: &str,
        zone: &str,
        kind: RecordKind,
        ip_list: &[IpAddr],
    ) -> Result<()> {
        let zone_id = self.get_zone_id(zone).await?;

        let exist_dns_records: HashSet<_> = HashSet::from_iter(
            self.get_dns_record_with_zone_id(name, &zone_id, kind)
                .await?,
        );

        if !ip_list.iter().any(|ip| !exist_dns_records.contains(ip)) {
            info!(name, zone, %zone_id, %kind, ?ip_list, "no need update");

            return Ok(());
        }

        self.remove_dns_record_with_zone_id(name, &zone_id, kind)
            .await?;

        info!(name, zone, %zone_id, %kind, ?ip_list, "remove old dns record success");

        for ip in ip_list {
            let create_dns_req = match ip {
                IpAddr::V4(ip) => CreateDnsRecord {
                    zone_identifier: &zone_id,
                    params: CreateDnsRecordParams {
                        ttl: DEFAULT_TTL,
                        priority: None,
                        proxied: None,
                        name,
                        content: DnsContent::A { content: *ip },
                    },
                },

                IpAddr::V6(ip) => CreateDnsRecord {
                    zone_identifier: &zone_id,
                    params: CreateDnsRecordParams {
                        ttl: DEFAULT_TTL,
                        priority: None,
                        proxied: None,
                        name,
                        content: DnsContent::AAAA { content: *ip },
                    },
                },
            };

            let create_dns_resp = self
                .client
                .request(&create_dns_req)
                .instrument(info_span!("create_dns_record"))
                .await
                .map_err(|err| {
                    error!(name, zone, %zone_id, %kind, %ip, %err, "create dns record failed");

                    err
                })?;
            if let Some(api_err) = create_dns_resp.errors.first() {
                return Err(anyhow::anyhow!("{}", api_err));
            }

            info!(?create_dns_req, "create dns record success");
        }

        info!(name, zone, %zone_id, %kind, ?ip_list, "set dns record success");

        Ok(())
    }

    #[instrument(err)]
    pub async fn remove_dns_records(&self, name: &str, zone: &str, kind: RecordKind) -> Result<()> {
        let zone_id = self.get_zone_id(zone).await?;

        info!(name, zone, %zone_id, "get zone id done");

        self.remove_dns_record_with_zone_id(name, &zone_id, kind)
            .await?;

        info!(name, zone, %zone_id, %kind, "remove dns record success");

        Ok(())
    }

    #[instrument(err)]
    async fn remove_dns_record_with_zone_id(
        &self,
        name: &str,
        zone_id: &str,
        _kind: RecordKind,
    ) -> Result<()> {
        let list_dns_req = ListDnsRecords {
            zone_identifier: zone_id,
            params: ListDnsRecordsParams {
                record_type: None,
                name: Some(name.to_string()),
                page: None,
                per_page: None,
                order: None,
                direction: None,
                search_match: None,
            },
        };

        info!(?list_dns_req, "create list dns request");

        let list_dns_resp = self
            .client
            .request(&list_dns_req)
            .inspect_err(|err| {
                error!(?list_dns_req, %err, "list dns failed");
            })
            .await?;

        info!(?list_dns_resp, "get dns list response done");

        if let Some(api_err) = list_dns_resp.errors.first() {
            error!(%api_err, "list dns failed with response");

            return Err(anyhow::anyhow!("{}", api_err));
        }

        let dns_list: Vec<DnsRecord> = list_dns_resp.result;

        info!(?dns_list, "get dns list");

        for dns_record in dns_list
            .into_iter()
            .filter(|dns_record| dns_record.name == name)
        {
            let delete_dns_req = DeleteDnsRecord {
                zone_identifier: zone_id,
                identifier: &dns_record.id,
            };

            info!(?delete_dns_req, "create delete dns request");

            let delete_dns_resp = match self.client.request(&delete_dns_req).await {
                Err(ApiFailure::Error(status_code, _)) if status_code == StatusCode::NOT_FOUND => {
                    info!(name, zone_id, "dns record has been removed");

                    return Ok(());
                }

                Err(err) => {
                    error!(?delete_dns_req, %err, "delete dns failed");

                    return Err(err.into());
                }

                Ok(resp) => resp,
            };

            info!(?delete_dns_resp, "get delete dns response done");

            if let Some(api_err) = delete_dns_resp.errors.first() {
                error!(%api_err, "delete dns failed with response");

                return Err(anyhow::anyhow!("{}", api_err));
            }
        }

        info!(name, zone_id, "remove dns record success");

        Ok(())
    }

    #[instrument(err)]
    async fn get_zone_id(&self, zone: &str) -> Result<String> {
        let list_zones_req = ListZones {
            params: ListZonesParams {
                name: Some(zone.to_string()),
                status: None,
                page: None,
                per_page: None,
                order: None,
                direction: None,
                search_match: None,
            },
        };

        info!(?list_zones_req, "create list zones request");

        let list_zones_resp = self.client.request(&list_zones_req).await.map_err(|err| {
            error!(%err, get_zone_request = ?list_zones_req, "send get zone id request failed");

            err
        })?;

        info!(?list_zones_resp, "get list zones response done");

        if let Some(api_err) = list_zones_resp.errors.first() {
            error!(%api_err, "list zone failed with response");

            return Err(anyhow::anyhow!("api error {}", api_err));
        }

        let list_zones_resp: Vec<Zone> = list_zones_resp.result;

        info!(?list_zones_resp, "list zones done");

        list_zones_resp
            .into_iter()
            .find_map(|zone_info| (zone_info.name == zone).then(|| zone_info.id))
            .ok_or_else(|| {
                error!(?zone, "zone is not exist");

                anyhow::anyhow!("zone {} is not exist", zone)
            })
    }

    #[instrument(err)]
    async fn get_dns_record_with_zone_id(
        &self,
        name: &str,
        zone_id: &str,
        kind: RecordKind,
    ) -> Result<Vec<IpAddr>> {
        let list_dns_req = ListDnsRecords {
            zone_identifier: zone_id,
            params: ListDnsRecordsParams {
                record_type: None,
                name: Some(name.to_string()),
                page: None,
                per_page: None,
                order: None,
                direction: None,
                search_match: None,
            },
        };

        let list_dns_resp = self.client.request(&list_dns_req).await?;
        if let Some(api_err) = list_dns_resp.errors.first() {
            return Err(anyhow::anyhow!("{}", api_err));
        }

        let list_dns_resp: Vec<DnsRecord> = list_dns_resp.result;

        let ip_list = list_dns_resp
            .into_iter()
            .filter_map(|dns_record| {
                if dns_record.name == name {
                    match dns_record.content {
                        DnsContent::A { content } => {
                            if kind == RecordKind::A {
                                Some(IpAddr::from(content))
                            } else {
                                None
                            }
                        }
                        DnsContent::AAAA { content } => {
                            if kind == RecordKind::AAAA {
                                Some(IpAddr::from(content))
                            } else {
                                None
                            }
                        }

                        _ => None,
                    }
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        info!(name, zone_id, %kind, ?ip_list, "get dns records success");

        Ok(ip_list)
    }
}

fn create_credentials() -> Credentials {
    if let Some(cred) = create_credentials_from_email() {
        return cred;
    }

    create_credentials_from_token().expect("can't find cloudflare (email, key) or token")
}

fn create_credentials_from_email() -> Option<Credentials> {
    let email = env::var("CF_DNS_EMAIL").ok()?;
    let key = env::var("CF_DNS_KEY").ok()?;

    Some(Credentials::UserAuthKey { email, key })
}

fn create_credentials_from_token() -> Option<Credentials> {
    let token = env::var("CF_DNS_TOKEN").ok()?;

    Some(Credentials::UserAuthToken { token })
}

#[cfg(test)]
mod tests {
    use std::mem;
    use std::sync::Once;

    use super::*;

    fn init_tracing() {
        static TRACING_INIT: Once = Once::new();

        TRACING_INIT.call_once(|| {
            let tracing_stop = crate::trace::init_tracing().unwrap();

            mem::forget(tracing_stop);
        });
    }

    #[tokio::test]
    async fn get_dns_record() {
        init_tracing();

        let cf_dns = CfDns::new().await.unwrap();

        let zone = env::var("TEST_ZONE").unwrap();
        let domain = format!("test-get.{}", zone);

        let dns_records = cf_dns
            .get_dns_record(&domain, &zone, RecordKind::A)
            .await
            .unwrap();

        println!("{:?}", dns_records);
    }

    #[tokio::test]
    async fn set_dns_record() {
        init_tracing();

        let cf_dns = CfDns::new().await.unwrap();

        let zone = env::var("TEST_ZONE").unwrap();
        let domain = format!("test-set.{}", zone);

        let ips = [IpAddr::from([127, 0, 0, 1]), IpAddr::from([127, 0, 0, 2])];

        cf_dns
            .set_dns_record(&domain, &zone, RecordKind::A, &ips)
            .await
            .unwrap();

        let dns_records = cf_dns
            .get_dns_record(&domain, &zone, RecordKind::A)
            .await
            .unwrap();
        assert_eq!(dns_records.len(), 2);

        let set: HashSet<_> = HashSet::from_iter(dns_records);

        for ip in &ips {
            if !set.contains(ip) {
                panic!("ip {} not exist", ip);
            }
        }
    }

    #[tokio::test]
    async fn remove_dns_record() {
        init_tracing();

        let cf_dns = CfDns::new().await.unwrap();

        let zone = env::var("TEST_ZONE").unwrap();
        let domain = format!("test-remove.{}", zone);

        let ips = [IpAddr::from([127, 0, 0, 1]), IpAddr::from([127, 0, 0, 2])];

        cf_dns
            .set_dns_record(&domain, &zone, RecordKind::A, &ips)
            .await
            .unwrap();

        let dns_records = cf_dns
            .get_dns_record(&domain, &zone, RecordKind::A)
            .await
            .unwrap();
        assert_eq!(dns_records.len(), 2);

        let set: HashSet<_> = HashSet::from_iter(dns_records);

        for ip in &ips {
            if !set.contains(ip) {
                panic!("ip {} not exist", ip);
            }
        }

        cf_dns
            .remove_dns_records(&domain, &zone, RecordKind::A)
            .await
            .unwrap();

        let dns_records = cf_dns
            .get_dns_record(&domain, &zone, RecordKind::A)
            .await
            .unwrap();
        assert_eq!(dns_records.len(), 0);
    }
}
