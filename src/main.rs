use aws_sdk_route53::types;
use cloudflare::endpoints::dns::dns;
use cloudflare::framework::client::{self, async_api};
use cloudflare::framework::{self, response};
use std::net::{self, ToSocketAddrs};
use std::str::FromStr;
use std::{env, error, fmt, io, str};

#[derive(Debug)]
enum DNSUpdateError {
    Route53(aws_sdk_route53::Error),
    AddrParse(net::AddrParseError),
    Cloudflare(response::ApiFailure),
}

impl fmt::Display for DNSUpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Route53(e) => write!(f, "route53 error: {e}"),
            Self::AddrParse(e) => write!(f, "addr parse error: {e}"),
            Self::Cloudflare(e) => write!(f, "cloudflare error: {e}"),
        }
    }
}

impl error::Error for DNSUpdateError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Route53(e) => Some(e),
            Self::AddrParse(e) => Some(e),
            Self::Cloudflare(e) => Some(e),
        }
    }
}

trait DNSUpdater {
    async fn update(&self, host_name: String, record_value: String) -> Result<(), DNSUpdateError>;
}

struct Route53Updater {
    client: aws_sdk_route53::Client,
    hosted_zone_id: String,
}

impl Route53Updater {
    pub fn new(client: aws_sdk_route53::Client, hosted_zone_id: String) -> Self {
        Self {
            client,
            hosted_zone_id,
        }
    }
}

impl From<aws_sdk_route53::Error> for DNSUpdateError {
    fn from(e: aws_sdk_route53::Error) -> Self {
        Self::Route53(e)
    }
}

impl DNSUpdater for Route53Updater {
    async fn update(&self, host_name: String, record_value: String) -> Result<(), DNSUpdateError> {
        let resource_record = types::ResourceRecord::builder()
            .value(record_value)
            .build()
            .map_err(Into::<aws_sdk_route53::Error>::into)?;
        let resource_record_set = types::ResourceRecordSet::builder()
            .name(host_name)
            .ttl(300)
            .r#type(types::RrType::A)
            .resource_records(resource_record)
            .build()
            .map_err(Into::<aws_sdk_route53::Error>::into)?;
        let change = types::Change::builder()
            .action(types::ChangeAction::Upsert)
            .resource_record_set(resource_record_set)
            .build()
            .map_err(Into::<aws_sdk_route53::Error>::into)?;
        let change_batch = types::ChangeBatch::builder()
            .changes(change)
            .build()
            .map_err(Into::<aws_sdk_route53::Error>::into)?;
        let hosted_zone_id = &self.hosted_zone_id;
        self.client
            .change_resource_record_sets()
            .hosted_zone_id(hosted_zone_id)
            .change_batch(change_batch)
            .send()
            .await
            .map(|_| ())
            .map_err(Into::<aws_sdk_route53::Error>::into)
            .map_err(Into::<DNSUpdateError>::into)
    }
}

struct CloudflareUpdater {
    client: async_api::Client,
    zone_identifier: String,
    identifier: String,
}

impl CloudflareUpdater {
    pub fn new(client: async_api::Client, zone_identifier: String, identifier: String) -> Self {
        CloudflareUpdater {
            client,
            zone_identifier,
            identifier,
        }
    }
}

impl From<net::AddrParseError> for DNSUpdateError {
    fn from(e: net::AddrParseError) -> Self {
        Self::AddrParse(e)
    }
}

impl From<response::ApiFailure> for DNSUpdateError {
    fn from(e: response::ApiFailure) -> Self {
        Self::Cloudflare(e)
    }
}

impl DNSUpdater for CloudflareUpdater {
    async fn update(&self, host_name: String, record_value: String) -> Result<(), DNSUpdateError> {
        let record_ip: net::Ipv4Addr = record_value.parse()?;
        let endpoint = dns::UpdateDnsRecord {
            zone_identifier: &self.zone_identifier,
            identifier: &self.identifier,
            params: dns::UpdateDnsRecordParams {
                ttl: Some(60),
                proxied: None,
                name: &host_name,
                content: dns::DnsContent::A { content: record_ip },
            },
        };
        self.client.request(&endpoint).await?;
        Ok(())
    }
}

async fn current() -> Result<String, reqwest::Error> {
    reqwest::Client::new()
        .get("https://ifconfig.co")
        .header("Accept", "text/plain")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await
        .map(|t| String::from(t.trim()))
}

fn lookup(host_name: &str, port: u16) -> Result<Option<String>, io::Error> {
    Ok((host_name, port)
        .to_socket_addrs()?
        .next()
        .map(|addr| String::from(addr.ip().to_string().trim())))
}

enum Provider {
    Route53,
    Cloudflare,
}

impl FromStr for Provider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "route53" => Ok(Self::Route53),
            "cloudflare" => Ok(Self::Cloudflare),
            _ => Err("not found".to_string()),
        }
    }
}

fn required_env_var(env_var: &str) -> String {
    env::var(env_var).unwrap_or_else(|_| panic!("Missing value for env var {env_var}"))
}

#[tokio::main]
async fn main() {
    let provider_str = required_env_var("PROVIDER");
    let provider = Provider::from_str(&provider_str)
        .unwrap_or_else(|_| panic!("Unknown provider {provider_str}"));

    let host_name = required_env_var("HOST_NAME");

    let external_ip = current().await.expect("Unable to get current IP address");
    let host_ip = lookup(&host_name, 80)
        .unwrap_or_else(|_| panic!("Unable to get IP address of host {host_name}"))
        .unwrap_or_else(|| panic!("Missing IP address for host {host_name}"));

    println!("Current external IP address is {}", external_ip);
    println!("IP address of {} is {}", host_name, host_ip);

    if host_ip != external_ip {
        println!("Updating DNS record of {} to {}", host_name, external_ip);

        match provider {
            Provider::Route53 => {
                let hosted_zone_id = required_env_var("HOSTED_ZONE_ID");
                let assume_role_arn = required_env_var("ASSUME_ROLE_ARN");
                let config =
                    aws_config::load_defaults(aws_config::BehaviorVersion::v2025_01_17()).await;
                let provider = aws_config::sts::AssumeRoleProvider::builder(assume_role_arn)
                    .configure(&config)
                    .build()
                    .await;
                let local_config = aws_config::defaults(aws_config::BehaviorVersion::v2025_01_17())
                    .credentials_provider(provider)
                    .load()
                    .await;
                let client = aws_sdk_route53::Client::new(&local_config);

                Route53Updater::new(client, hosted_zone_id)
                    .update(host_name, external_ip)
                    .await
                    .expect("Failed to update DNS records")
            }
            Provider::Cloudflare => {
                let zone_identifier = required_env_var("CLOUDFLARE_ZONE_IDENTIFIER");
                let identifier = required_env_var("CLOUDFLARE_IDENTIFIER");
                let token = required_env_var("CLOUDFLARE_TOKEN");

                let creds = cloudflare::framework::auth::Credentials::UserAuthToken { token };
                let config = client::ClientConfig {
                    http_timeout: std::time::Duration::new(60, 0),
                    default_headers: http::HeaderMap::new(),
                    resolve_ip: None,
                };
                let environment = framework::Environment::Production;
                let client = async_api::Client::new(creds, config, environment)
                    .expect("Couldn't make Cloudflare API client");
                CloudflareUpdater::new(client, zone_identifier, identifier)
                    .update(host_name, external_ip)
                    .await
                    .expect("Failed to update DNS records")
            }
        }
    }
}
