use aws_sdk_route53::types;
use std::net::ToSocketAddrs;
use std::str::FromStr;
use std::{env, error, fmt, io, str};

#[derive(Debug)]
enum DNSUpdateError {
    Route53Error(aws_sdk_route53::Error),
}

impl fmt::Display for DNSUpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Route53Error(e) => write!(f, "route53 error: {e}"),
        }
    }
}

impl error::Error for DNSUpdateError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Route53Error(e) => Some(e),
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
        Self::Route53Error(e)
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
}

impl FromStr for Provider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "route53" => Ok(Self::Route53),
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

        let updater = match provider {
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
            }
        };

        updater
            .update(host_name, external_ip)
            .await
            .expect("Failed to update DNS records");
    }
}
