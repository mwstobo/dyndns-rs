use aws_sdk_route53::model;
use std::net::ToSocketAddrs;
use std::{env, io, time};

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
    client
        .change_resource_record_sets()
        .hosted_zone_id(hosted_zone_id)
        .change_batch(
            model::ChangeBatch::builder()
                .changes(
                    model::Change::builder()
                        .action(model::ChangeAction::Upsert)
                        .resource_record_set(
                            model::ResourceRecordSet::builder()
                                .name(host_name)
                                .ttl(300)
                                .r#type(model::RrType::A)
                                .resource_records(
                                    model::ResourceRecord::builder().value(current).build(),
                                )
                                .build(),
                        )
                        .build(),
                )
                .build(),
        )
        .send()
        .await
        .map(|_| ())
        .map_err(|e| e.into())
}

async fn push(
    push_gateway_host: String,
    job: &str,
    current_time: u64,
) -> Result<(), reqwest::Error> {
    reqwest::Client::new()
        .post(format!("{}/metrics/job/{}", push_gateway_host, job))
        .body(format!(
            "#TYPE last_successful_execution_timestamp_seconds gauge\n\
             #HELP last_successful_execution_timestamp_seconds \
                   Timestamp of the last successful execution of a job\n\
             last_successful_execution_timestamp_seconds {}\n",
            current_time
        ))
        .send()
        .await?
        .error_for_status()
        .map(|_| ())
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
    let push_gateway_host = RequiredEnvVar::new("PUSH_GATEWAY_HOST").value;

    let external_ip = current().await.expect("Unable to get current IP address");
    let host_ip = lookup(&host_name, 80)
        .expect(&format!("Unable to get IP address of host {host_name}"))
        .expect(&format!("Missing IP address for host {host_name}"));

    println!("Current external IP address is {}", external_ip);
    println!("IP address of {} is {}", host_name, host_ip);

    if host_ip != external_ip {
        println!("Updating DNS record of {} to {}", host_name, external_ip);
        let config = aws_config::load_from_env().await;
        let client = aws_sdk_route53::Client::new(&config);
        update(client, hosted_zone_id, host_name, external_ip)
            .await
            .expect("Failed to update DNS records");
    }
    let current_time = time::SystemTime::now()
        .duration_since(time::UNIX_EPOCH)
        .expect("Unable to get current system time")
        .as_secs();
    push(push_gateway_host, "dyndns_route53", current_time)
        .await
        .expect("Failed to push metrics to push gateway")
}
