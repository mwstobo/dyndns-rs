use aws_sdk_route53::{
    model::{Change, ChangeAction, ChangeBatch, ResourceRecord, ResourceRecordSet, RrType},
    Client,
};
use std::net::ToSocketAddrs;

#[derive(Debug)]
enum Error {
    Reqwest(reqwest::Error),
    Io(std::io::Error),
    Route53(aws_sdk_route53::Error),
    Time(std::time::SystemTimeError),
    Env(std::env::VarError),
    Message(String),
}

async fn current() -> Result<String, Error> {
    reqwest::Client::new()
        .get("https://ifconfig.co")
        .header("User-Agent", "curl/7.81.0")
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(Error::Reqwest)?
        .text()
        .await
        .map(|t| String::from(t.trim()))
        .map_err(Error::Reqwest)
}

fn lookup(host_name: &str, port: u16) -> Result<String, Error> {
    (host_name, port)
        .to_socket_addrs()
        .map_err(Error::Io)
        .and_then(|mut addrs| {
            addrs.nth(0).ok_or(Error::Message(format!(
                "Received 0 addresses for {}",
                host_name
            )))
        })
        .map(|addr| String::from(addr.ip().to_string().trim()))
}

async fn update(
    client: aws_sdk_route53::Client,
    hosted_zone_id: String,
    host_name: String,
    current: String,
) -> Result<(), Error> {
    client
        .change_resource_record_sets()
        .hosted_zone_id(hosted_zone_id)
        .change_batch(
            ChangeBatch::builder()
                .changes(
                    Change::builder()
                        .action(ChangeAction::Upsert)
                        .resource_record_set(
                            ResourceRecordSet::builder()
                                .name(host_name)
                                .r#type(RrType::Cname)
                                .resource_records(ResourceRecord::builder().value(current).build())
                                .build(),
                        )
                        .build(),
                )
                .build(),
        )
        .send()
        .await
        .map(|_| ())
        .map_err(|e| Error::Route53(e.into()))
}

async fn push(push_gateway_host: String, job: &str) -> Result<(), Error> {
    let current_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(Error::Time)?
        .as_secs();
    reqwest::Client::new()
        .post(format!("{}/metrics/job/{}", push_gateway_host, job))
        .body(format!(
            "last_successful_execution_timestamp_seconds {}",
            current_time
        ))
        .send()
        .await
        .map(|_| ())
        .map_err(Error::Reqwest)
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let host_name = std::env::var("HOST_NAME").map_err(Error::Env)?;
    let hosted_zone_id = std::env::var("HOSTED_ZONE_ID").map_err(Error::Env)?;
    let push_gateway_host = std::env::var("PUSH_GATEWAY_HOST").map_err(Error::Env)?;

    let external_ip = current().await?;
    let host_ip = lookup(&host_name, 80)?;

    println!("Current external IP address is {}", external_ip);
    println!("IP address of {} is {}", host_name, host_ip);

    if host_ip != external_ip {
        println!("Updating DNS recrod of {} to {}", host_name, external_ip);
        let config = aws_config::load_from_env().await;
        let client = Client::new(&config);
        update(client, hosted_zone_id, host_name, external_ip).await?;
    }
    push(push_gateway_host, "dyndns_route53").await?;
    Ok(())
}
