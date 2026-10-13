//! Asynchronous SeedLink client state machine.
//!
//! States: Connect -> Hello -> (Select)* -> BeginData -> Streaming.
//!
//! The streaming loop runs on a spawned Tokio task and pushes parsed packets
//! into an `mpsc` channel, so consumers (Tauri commands, UI threads, CLI
//! tools) never block on socket I/O.
//!
//! GAIA TAU notes (uni-station embedded server):
//! - `HELLO` returns TWO ident lines; both are drained here.
//! - No `STATION` command is ever sent — the TAU rejects it.
//! - Streaming is started with `END` (the TAU streams after SELECTs + END;
//!   slinktool's `DATA` path after SELECTs is silently ignored by some TAU
//!   firmware revisions). Standard SeedLink servers also accept `END`.
//! - If no SELECTs are issued, the TAU streams its full ring buffer output.

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, info, instrument, warn};

use crate::parser::MiniSeedHeader;
use crate::protocol::{
    cmd, expand_tau_selectors, SeedLinkError, CRLF, DEFAULT_PORT, MSEED_RECORD_SIZE, SL_FRAME_SIZE,
    SL_SIGNATURE,
};

/// Client configuration.
#[derive(Debug, Clone)]
pub struct SeedLinkConfig {
    pub host: String,
    pub port: u16,
    /// Explicit uni-station SELECT tokens (e.g. `["00HNZ", "00HNN"]`).
    /// Empty = request the server's full stream (recommended on GAIA TAU).
    pub selectors: Vec<String>,
    /// Abort the read loop if no frame arrives within this window.
    pub network_timeout: Duration,
    /// Channel capacity for the packet queue.
    pub packet_queue: usize,
}

impl SeedLinkConfig {
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            port: DEFAULT_PORT,
            selectors: Vec::new(),
            network_timeout: Duration::from_secs(600),
            packet_queue: 256,
        }
    }

    /// Convenience: expand `(loc, chan)` pairs into TAU selector tokens.
    pub fn with_tau_channels(mut self, loc: &str, chan: &str) -> Self {
        self.selectors = expand_tau_selectors(loc, chan);
        self
    }
}

/// One decoded SeedLink frame.
#[derive(Debug, Clone)]
pub struct SeedLinkPacket {
    /// SeedLink frame sequence (from the ASCII `SLnnnnnn` header).
    pub sl_sequence: String,
    pub header: MiniSeedHeader,
    /// Raw 512-byte miniSEED record (header stripped of SL framing).
    pub record: Vec<u8>,
}

/// Server identification returned by the HELLO handshake.
#[derive(Debug, Clone)]
pub struct ServerId {
    /// e.g. `SeedLink v3.0 (GAIA TAU <FIRMWARE_ID>) : SLPROTO:3.0`
    pub server: String,
    /// e.g. `XX.TEST1`
    pub organization: String,
}

pub struct SeedLinkClient {
    config: SeedLinkConfig,
}

impl SeedLinkClient {
    pub fn new(config: SeedLinkConfig) -> Self {
        Self { config }
    }

    /// Connect and perform the HELLO handshake. Returns the stream and the
    /// server identification lines.
    #[instrument(skip(self), fields(addr = %format!("{}:{}", self.config.host, self.config.port)))]
    async fn connect(&self) -> Result<(BufReader<TcpStream>, ServerId), SeedLinkError> {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        let stream = TcpStream::connect(&addr).await?;
        stream.set_nodelay(true)?;
        let mut reader = BufReader::new(stream);

        reader
            .write_all(format!("{}{}", cmd::HELLO, CRLF).as_bytes())
            .await?;

        // HELLO returns exactly two ident lines.
        let server = Self::read_line(&mut reader, self.config.network_timeout).await?;
        let organization = Self::read_line(&mut reader, self.config.network_timeout).await?;
        info!(%server, %organization, "SeedLink handshake complete");

        Ok((
            reader,
            ServerId {
                server,
                organization,
            },
        ))
    }

    /// Issue one SELECT per configured token (uni-station mode only).
    async fn negotiate_selectors(
        &self,
        reader: &mut BufReader<TcpStream>,
    ) -> Result<(), SeedLinkError> {
        for sel in &self.config.selectors {
            let command = format!("{} {}", cmd::SELECT, sel);
            reader
                .write_all(format!("{command}{CRLF}").as_bytes())
                .await?;
            let response = Self::read_line(reader, self.config.network_timeout).await?;
            debug!(%sel, %response, "SELECT response");
            if response.starts_with("ERROR") {
                return Err(SeedLinkError::CommandRejected { command, response });
            }
        }
        Ok(())
    }

    /// Start streaming: send END and return the packet receiver plus the
    /// background task handle. Dropping the receiver stops consumption; call
    /// [`JoinHandle::abort`] to halt the loop explicitly.
    pub async fn stream(
        &self,
    ) -> Result<
        (
            ServerId,
            mpsc::Receiver<Result<SeedLinkPacket, SeedLinkError>>,
            JoinHandle<()>,
        ),
        SeedLinkError,
    > {
        let (mut reader, server_id) = self.connect().await?;
        self.negotiate_selectors(&mut reader).await?;

        // END requests streaming from the next available data. On the GAIA
        // TAU, END elicits no reply — streaming begins immediately.
        reader
            .write_all(format!("{}{}", cmd::END, CRLF).as_bytes())
            .await?;
        info!(selectors = ?self.config.selectors, "streaming commenced");

        let (tx, rx) = mpsc::channel(self.config.packet_queue);
        let timeout = self.config.network_timeout;

        let task = tokio::spawn(async move {
            if let Err(e) = Self::read_loop(&mut reader, &tx, timeout).await {
                warn!(error = %e, "stream terminated");
                // Deliver the terminal error so consumers can react/reconnect.
                let _ = tx.send(Err(e)).await;
            }
        });

        Ok((server_id, rx, task))
    }

    /// Frame read loop: exact 520-byte reads, signature check with
    /// resync-on-garbage, header parse, channel dispatch. Never blocks the
    /// caller — runs entirely on the spawned task.
    async fn read_loop(
        reader: &mut BufReader<TcpStream>,
        tx: &mpsc::Sender<Result<SeedLinkPacket, SeedLinkError>>,
        timeout: Duration,
    ) -> Result<(), SeedLinkError> {
        let mut frame = vec![0u8; SL_FRAME_SIZE];

        loop {
            match tokio::time::timeout(timeout, reader.read_exact(&mut frame)).await {
                Err(_) => return Err(SeedLinkError::Timeout(timeout)),
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Err(SeedLinkError::ConnectionClosed)
                }
                Ok(Err(e)) => return Err(e.into()),
                Ok(Ok(_)) => {}
            }

            if &frame[0..2] != SL_SIGNATURE {
                // Resync: drop one byte and continue; TCP fragmentation or a
                // stray keepalive can misalign us.
                warn!("bad frame signature, resyncing");
                frame.copy_within(1.., 0);
                let mut tail = [0u8; 1];
                reader.read_exact(&mut tail).await?;
                frame[SL_FRAME_SIZE - 1] = tail[0];
                continue;
            }

            let sl_sequence = String::from_utf8_lossy(&frame[2..8]).to_string();
            let record = frame[8..8 + MSEED_RECORD_SIZE].to_vec();
            let header = MiniSeedHeader::parse(&record)?;

            // Receiver dropped -> consumer is gone, exit quietly.
            if tx
                .send(Ok(SeedLinkPacket {
                    sl_sequence,
                    header,
                    record,
                }))
                .await
                .is_err()
            {
                return Ok(());
            }
        }
    }

    async fn read_line(
        reader: &mut BufReader<TcpStream>,
        timeout: Duration,
    ) -> Result<String, SeedLinkError> {
        let mut line = String::new();
        match tokio::time::timeout(timeout, reader.read_line(&mut line)).await {
            Err(_) => Err(SeedLinkError::Timeout(timeout)),
            Ok(Err(e)) => Err(e.into()),
            Ok(Ok(0)) => Err(SeedLinkError::ConnectionClosed),
            Ok(Ok(_)) => Ok(line.trim_end().to_string()),
        }
    }
}
