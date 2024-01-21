use aws_sdk_route53::types;
use std::net::ToSocketAddrs;
use std::{env, io};

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
    (host_name, port).to_socket_addrs().map(|mut addrs| {
        addrs
            .nth(0)
            .map(|addr| String::from(addr.ip().to_string().trim()))
    })
}

async fn update(
    client: aws_sdk_route53::Client,
    hosted_zone_id: String,
    host_name: String,
    current: String,
) -> Result<(), aws_sdk_route53::Error> {
    let resource_record = types::ResourceRecord::builder().value(current).build()?;
    let resource_record_set = types::ResourceRecordSet::builder()
        .name(host_name)
        .ttl(300)
        .r#type(types::RrType::A)
        .resource_records(resource_record)
        .build()?;
    let change = types::Change::builder()
        .action(types::ChangeAction::Upsert)
        .resource_record_set(resource_record_set)
        .build()?;
    let change_batch = types::ChangeBatch::builder().changes(change).build()?;
    client
        .change_resource_record_sets()
        .hosted_zone_id(hosted_zone_id)
        .change_batch(change_batch)
        .send()
        .await
        .map(|_| ())
        .map_err(|e| e.into())
}

struct RequiredEnvVar {
    value: String,
}

impl RequiredEnvVar {
    fn new(env_var: &str) -> Self {
        let value = env::var(env_var).expect(&format!("Missing value for env var {env_var}"));
        Self { value }
    }
}

#[tokio::main]
async fn main() {
    let host_name = RequiredEnvVar::new("HOST_NAME").value;
    let hosted_zone_id = RequiredEnvVar::new("HOSTED_ZONE_ID").value;

    let external_ip = current().await.expect("Unable to get current IP address");
    let host_ip = lookup(&host_name, 80)
        .expect(&format!("Unable to get IP address of host {host_name}"))
        .expect(&format!("Missing IP address for host {host_name}"));

    println!("Current external IP address is {}", external_ip);
    println!("IP address of {} is {}", host_name, host_ip);

    if host_ip != external_ip {
        println!("Updating DNS record of {} to {}", host_name, external_ip);
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::v2023_11_09()).await;
        let client = aws_sdk_route53::Client::new(&config);
        update(client, hosted_zone_id, host_name, external_ip)
            .await
            .expect("Failed to update DNS records");
    }
}
