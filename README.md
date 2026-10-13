# gaiacode-slink-rs

Asynchronous [SeedLink](https://www.fdsn.org/seedlink/) client and miniSEED
record-header parsing for GaiaCode tooling. Connects to a SeedLink server
(standard SeedLink 3.0 or the GaiaCode GAIA TAU embedded digitizer server),
performs the handshake, streams waveform frames on a background Tokio task,
and delivers parsed packets over an `mpsc` channel so consumers (UI threads,
Tauri commands, CLI tools) never block on socket I/O.

## Technology Stack

- **Rust** (edition 2021)
- **tokio** — async runtime: TCP, timeouts, `sync::mpsc`, spawned read loop
- **bytes** / **thiserror** / **tracing**

## Features

- SeedLink wire protocol: command constants (`HELLO`, `SELECT`, `END`, ...),
  520-byte frame layout (8-byte ASCII `SLnnnnnn` header + 512-byte miniSEED
  record), signature checking with resync-on-garbage.
- GAIA TAU compatibility: uni-station handshake (no `STATION` command),
  explicit `LOC+CHAN` SELECT tokens with wildcard expansion
  (`expand_tau_selectors`), `END`-initiated full-stream mode.
- miniSEED fixed-section data header (FSDH) parsing: stream identifiers,
  start time (leap-year-correct Unix conversion), sample counts and rates,
  LOG-record detection. Header-only by design — STEIM decompression is out
  of scope to keep the crate dependency-light.
- Typed error surface (`SeedLinkError`): I/O, command rejection, protocol
  violations, timeouts, parse failures.
- Structured logging via `tracing`.

## API Overview

| Item | Purpose |
| --- | --- |
| `SeedLinkConfig` | Host/port, selectors, network timeout, channel capacity |
| `SeedLinkClient::stream()` | Connect, handshake, negotiate selectors; returns `(ServerId, mpsc::Receiver<Result<SeedLinkPacket, SeedLinkError>>, JoinHandle)` |
| `SeedLinkPacket` | One decoded frame: SL sequence, parsed header, raw 512-byte record |
| `MiniSeedHeader` | Parsed miniSEED FSDH: `stream_id()`, `start_unix()`, `is_log()` |
| `SeedLinkError` | Error enum for all failure modes |

## Setup

Not published to crates.io — use as a path or git dependency:

```toml
[dependencies]
gaiacode-slink-rs = { path = "../gaiacode-slink-rs" }
# or
gaiacode-slink-rs = { git = "https://github.com/romanianbuspass/gaiacode-slink-rs" }
```

## Usage

```rust
use gaiacode_slink_rs::{SeedLinkClient, SeedLinkConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Full-stream mode (no selectors) — recommended on the GAIA TAU.
    let mut config = SeedLinkConfig::new("<SEEDLINK_SERVER>");
    config.port = 18000;

    let client = SeedLinkClient::new(config);
    let (server, mut rx, task) = client.stream().await?;
    println!("connected: {} | org: {}", server.server, server.organization);

    while let Some(result) = rx.recv().await {
        let pkt = result?;
        if pkt.header.is_log() {
            continue;
        }
        println!(
            "seq {:>6}  {:<17} {:>4} samples  {:>6.1} Hz",
            pkt.sl_sequence,
            pkt.header.stream_id(),
            pkt.header.num_samples,
            pkt.header.sample_rate,
        );
    }

    task.abort();
    Ok(())
}
```

A runnable smoke test against a live server:

```sh
cargo run --example tau_stream -- <SEEDLINK_SERVER> [port] [count]
```

To select specific channels on a GAIA TAU instead of the full stream:

```rust
let config = SeedLinkConfig::new("<SEEDLINK_SERVER>")
    .with_tau_channels("00", "HN?"); // expands to 00HNZ, 00HNN, 00HNE
```

## Data Formats / Protocols

- **SeedLink 3.0** — TCP streaming protocol for near-real-time seismological
  data; default port 18000 on GaiaCode TAU digitizers. Commands are
  CRLF-terminated ASCII lines; data arrives as 520-byte frames
  (`SL` + 6-digit ASCII sequence + miniSEED record).
- **miniSEED** — Standard for the Exchange of Earthquake Data (SEED),
  512-byte fixed-length records. This crate parses the fixed-section data
  header (FSDH): sequence number, quality indicator, station/location/
  channel/network codes, BTIME start time, sample count, and sample-rate
  factor/multiplier. Data payload decompression (STEIM-1/2) is intentionally
  not included.

## Development

```sh
cargo test          # unit tests for header parsing and selector expansion
cargo build
```

## License

Apache-2.0
