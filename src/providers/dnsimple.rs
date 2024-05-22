/*
 * Copyright Stalwart Labs Ltd. See the COPYING
 * file at the top-level directory of this distribution.
 *
 * Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
 * https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
 * <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
 * option. This file may not be copied, modified, or distributed
 * except according to those terms.
 */

 use std::{
    net::{Ipv4Addr, Ipv6Addr},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{http::HttpClientBuilder, DnsRecord, Error, IntoFqdn};

#[derive(Clone)]
pub struct DNSimpleProvider {
    client: HttpClientBuilder,
    account_id: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct IdMap {
    pub id: String,
    pub name: String,
}

#[derive(Serialize, Debug)]
pub struct Query {
    name: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct CreateDnsRecordParams<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxied: Option<bool>,
    pub name: &'a str,
    #[serde(flatten)]
    pub content: DnsContent,
}

#[derive(Serialize, Clone, Debug)]
pub struct UpdateDnsRecordParams<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxied: Option<bool>,
    pub name: &'a str,
    #[serde(flatten)]
    pub content: DnsContent,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(tag = "type")]
#[allow(clippy::upper_case_acronyms)]
pub enum DnsContent {
    A { content: Ipv4Addr },
    AAAA { content: Ipv6Addr },
    CNAME { content: String },
    NS { content: String },
    MX { content: String, priority: u16 },
    TXT { content: String },
    SRV { content: String },
}

#[derive(Deserialize, Serialize, Debug)]
struct ApiResult<T> {
    errors: Vec<ApiError>,
    success: bool,
    result: T,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct ApiError {
    pub code: u16,
    pub message: String,
}

impl DNSimpleProvider {
    pub(crate) fn new(
        secret: impl AsRef<str>,
        account_id: Option<impl AsRef<str>>,
        timeout: Option<Duration>,
    ) -> crate::Result<Self> {
        let client = HttpClientBuilder::default()
            .with_header("Authorization", format!("Bearer {}", secret.as_ref()))
            .with_timeout(timeout);

        Ok(Self {
            client,
            account_id: account_id.map(|id| id.as_ref().to_string()),
        })
    }

    async fn obtain_zone_id(&self, origin: impl IntoFqdn<'_>) -> crate::Result<String> {
        let origin = origin.into_name();
        let account_id = self
            .account_id
            .as_ref()
            .ok_or_else(|| Error::Api("Account ID not provided".to_string()))?;
        self.client
            .get(format!(
                "https://api.dnsimple.com/{}/zones/{}",
                account_id,
                Query::name(origin.as_ref()).serialize()
            ))
            .send::<ApiResult<Vec<IdMap>>>()
            .await
            .and_then(|r| r.unwrap_response("list zones"))
            .and_then(|result| {
                result
                    .into_iter()
                    .find(|zone| zone.name == origin.as_ref())
                    .map(|zone| zone.id)
                    .ok_or_else(|| Error::Api(format!("Zone {} not found", origin.as_ref())))
            })
    }

    async fn obtain_record_id(
        &self,
        zone_id: &str,
        name: impl IntoFqdn<'_>,
    ) -> crate::Result<String> {
        let name = name.into_name();
        let account_id = self
            .account_id
            .as_ref()
            .ok_or_else(|| Error::Api("Account ID not provided".to_string()))?;
        self.client
            .get(format!(
                "https://api.dnsimple.com/{}/zones/{zone_id}/records{}",
                account_id,
                Query::name(name.as_ref()).serialize()
            ))
            .send::<ApiResult<Vec<IdMap>>>()
            .await
            .and_then(|r| r.unwrap_response("list DNS records"))
            .and_then(|result| {
                result
                    .into_iter()
                    .find(|record| record.name == name.as_ref())
                    .map(|record| record.id)
                    .ok_or_else(|| Error::Api(format!("DNS Record {} not found", name.as_ref())))
            })
    }

    pub(crate) async fn create(
        &self,
        name: impl IntoFqdn<'_>,
        record: DnsRecord,
        ttl: u32,
        origin: impl IntoFqdn<'_>,
    ) -> crate::Result<()> {
        self.client
            .post(format!(
                "https://api.dnsimple.com/client/v4/zones/{}/dns_records",
                self.obtain_zone_id(origin).await?
            ))
            .with_body(CreateDnsRecordParams {
                ttl: ttl.into(),
                priority: record.priority(),
                proxied: false.into(),
                name: name.into_name().as_ref(),
                content: record.into(),
            })?
            .send::<ApiResult<Value>>()
            .await
            .map_err(Into::into)
            .map(|_| ())
    }

    pub(crate) async fn update(
        &self,
        name: impl IntoFqdn<'_>,
        record: DnsRecord,
        ttl: u32,
        origin: impl IntoFqdn<'_>,
    ) -> crate::Result<()> {
        let name = name.into_name();
        self.client
            .patch(format!(
                "https://api.dnsimple.com/client/v4/zones/{}/dns_records/{}",
                self.obtain_zone_id(origin).await?,
                name.as_ref()
            ))
            .with_body(UpdateDnsRecordParams {
                ttl: ttl.into(),
                proxied: None,
                name: name.as_ref(),
                content: record.into(),
            })?
            .send::<ApiResult<Value>>()
            .await
            .map_err(Into::into)
            .map(|_| ())
    }

    pub(crate) async fn delete(
        &self,
        name: impl IntoFqdn<'_>,
        origin: impl IntoFqdn<'_>,
    ) -> crate::Result<()> {
        let zone_id = self.obtain_zone_id(origin).await?;
        let record_id = self.obtain_record_id(&zone_id, name).await?;

        self.client
            .delete(format!(
                "https://api.dnsimple.com/client/v4/zones/{zone_id}/dns_records/{record_id}",
            ))
            .send::<ApiResult<Value>>()
            .await
            .map_err(Into::into)
            .map(|_| ())
    }
}

impl<T> ApiResult<T> {
    fn unwrap_response(self, action_name: &str) -> crate::Result<T> {
        if self.success {
            Ok(self.result)
        } else {
            Err(Error::Api(format!(
                "Failed to {action_name}: {:?}",
                self.errors
            )))
        }
    }
}

impl Query {
    pub fn name(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }

    pub fn serialize(&self) -> String {
        serde_urlencoded::to_string(self).unwrap()
    }
}

impl From<DnsRecord> for DnsContent {
    fn from(record: DnsRecord) -> Self {
        match record {
            DnsRecord::A { content } => DnsContent::A { content },
            DnsRecord::AAAA { content } => DnsContent::AAAA { content },
            DnsRecord::CNAME { content } => DnsContent::CNAME { content },
            DnsRecord::NS { content } => DnsContent::NS { content },
            DnsRecord::MX { content, priority } => DnsContent::MX { content, priority },
            DnsRecord::TXT { content } => DnsContent::TXT { content },
            DnsRecord::SRV { content, .. } => DnsContent::SRV { content },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{mock, Matcher};
    use tokio;

    #[tokio::test]
    async fn test_obtain_zone_id() {
        let _m = mock("GET", Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"errors": [], "success": true, "result": [{"id": "12345", "name": "example.com"}]}"#)
            .create();

        let provider = DNSimpleProvider::new("secret", Some("account_id"), Some(Duration::from_secs(10))).unwrap();
        let zone_id = provider.obtain_zone_id("example.com").await.unwrap();
        assert_eq!(zone_id, "12345");
    }

    #[tokio::test]
    async fn test_obtain_record_id() {
        let _m = mock("GET", Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"errors": [], "success": true, "result": [{"id": "67890", "name": "www.example.com"}]}"#)
            .create();

        let provider = DNSimpleProvider::new("secret", Some("account_id"), Some(Duration::from_secs(10))).unwrap();
        let record_id = provider.obtain_record_id("12345", "www.example.com").await.unwrap();
        assert_eq!(record_id, "67890");
    }

    #[tokio::test]
    async fn test_create_dns_record() {
        let _m = mock("POST", Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"errors": [], "success": true, "result": {}}"#)
            .create();

        let provider = DNSimpleProvider::new("secret", Some("account_id"), Some(Duration::from_secs(10))).unwrap();
        let record = DnsRecord::A {
            content: "127.0.0.1".parse().unwrap(),
        };
        let result = provider.create("www", record, 3600, "example.com").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_update_dns_record() {
        let _m = mock("PATCH", Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"errors": [], "success": true, "result": {}}"#)
            .create();

        let provider = DNSimpleProvider::new("secret", Some("account_id"), Some(Duration::from_secs(10))).unwrap();
        let record = DnsRecord::A {
            content: "127.0.0.1".parse().unwrap(),
        };
        let result = provider.update("www", record, 3600, "example.com").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_delete_dns_record() {
        let _m = mock("DELETE", Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"errors": [], "success": true, "result": {}}"#)
            .create();

        let provider = DNSimpleProvider::new("secret", Some("account_id"), Some(Duration::from_secs(10))).unwrap();
        let result = provider.delete("www", "example.com").await;
        assert!(result.is_ok());
    }
}
