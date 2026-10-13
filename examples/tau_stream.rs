//! Live smoke test against a GAIA TAU (or any SeedLink) server.
//!
//! Usage: cargo run --example tau_stream -- <SEEDLINK_SERVER> [port] [count]

use gaiacode_slink_rs::{SeedLinkClient, SeedLinkConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let host = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "<SEEDLINK_SERVER>".into());
    let port: u16 = std::env::args()
        .nth(2)
        .and_then(|p| p.parse().ok())
        .unwrap_or(18000);
    let count: usize = std::env::args()
        .nth(3)
        .and_then(|c| c.parse().ok())
        .unwrap_or(10);

    // Full-stream mode (no selectors) — the reliable GAIA TAU pattern.
    let mut config = SeedLinkConfig::new(host);
    config.port = port;

    let client = SeedLinkClient::new(config);
    let (server, mut rx, task) = client.stream().await?;
    println!(
        "connected: {} | org: {}",
        server.server, server.organization
    );

    let mut received = 0;
    while let Some(result) = rx.recv().await {
        let pkt = result?;
        if pkt.header.is_log() {
            continue; // skip LOG records for the waveform summary
        }
        println!(
            "seq {:>6}  {:<17} {:>4} samples  {:>6.1} Hz  start_unix {:.4}",
            pkt.sl_sequence,
            pkt.header.stream_id(),
            pkt.header.num_samples,
            pkt.header.sample_rate,
            pkt.header.start_unix(),
        );
        received += 1;
        if received >= count {
            break;
        }
    }

    task.abort();
    println!("received {received} waveform packets — OK");
    Ok(())
}
