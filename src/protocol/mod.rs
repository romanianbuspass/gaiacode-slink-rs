//! SeedLink wire protocol constants, commands, and error types.
//!
//! Covers both standard SeedLink 3.0 servers and the GaiaCode GAIA TAU
//! embedded (uni-station, non-standard) server:
//! - TAU rejects `STATION` / `*` wildcards — uni-station handshake only.
//! - TAU streams 520-byte frames: 8-byte ASCII `SLnnnnnn` header + 512-byte
//!   miniSEED record.
//! - TAU selectors are LOC+CHAN concatenated tokens (e.g. `00HNZ`), and with
//!   slinktool-style negotiation they are accepted but yield nothing; the
//!   reliable TAU pattern is HELLO -> SELECT (LOCCHAN)* -> END.

use thiserror::Error;

/// Total SeedLink frame size: 8-byte header + 512-byte miniSEED record.
pub const SL_FRAME_SIZE: usize = 520;
/// Size of the miniSEED record payload within a frame.
pub const MSEED_RECORD_SIZE: usize = 512;
/// SeedLink frame signature (first two bytes of the 8-byte header).
pub const SL_SIGNATURE: &[u8; 2] = b"SL";
/// Line terminator for all SeedLink commands.
pub const CRLF: &str = "\r\n";

/// Default SeedLink port (GaiaCode Tau digitizers).
pub const DEFAULT_PORT: u16 = 18000;

/// SeedLink protocol commands.
pub mod cmd {
    pub const HELLO: &str = "HELLO";
    pub const SELECT: &str = "SELECT";
    pub const STATION: &str = "STATION";
    pub const DATA: &str = "DATA";
    pub const FETCH: &str = "FETCH";
    pub const END: &str = "END";
    pub const INFO: &str = "INFO";
    pub const CAT: &str = "CAT";
    pub const BYE: &str = "BYE";
}

/// Errors surfaced by the SeedLink client state machine.
#[derive(Debug, Error)]
pub enum SeedLinkError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("server rejected command `{command}`: {response}")]
    CommandRejected { command: String, response: String },

    #[error("protocol violation: {0}")]
    Protocol(String),

    #[error("invalid SeedLink frame signature (expected `SL`)")]
    BadFrameSignature,

    #[error("connection closed by server")]
    ConnectionClosed,

    #[error("timed out waiting for server data ({0:?})")]
    Timeout(std::time::Duration),

    #[error("miniSEED parse error: {0}")]
    Parse(String),
}

/// Expand a channel selector into GAIA TAU explicit LOC+CHAN SELECT tokens.
///
/// Rules (see the gaia-tau-seedlink-protocol knowledge base):
/// - The TAU wants one explicit `SELECT <LOCCHAN>` per channel, no wildcards.
/// - `chan` may use `?`/`*` suffixes; these expand to the Z/N/E triad.
///
/// Examples:
/// - `("00", "HNZ")`  -> `["00HNZ"]`
/// - `("00", "HN?")`  -> `["00HNZ", "00HNN", "00HNE"]`
/// - `("10", "H??")`  -> `["10HNZ", "10HNN", "10HNE"]`
pub fn expand_tau_selectors(loc: &str, chan: &str) -> Vec<String> {
    let prefix: String = chan
        .chars()
        .take_while(|c| *c != '?' && *c != '*')
        .collect();
    let has_wildcard = prefix.len() < chan.len();

    if !has_wildcard {
        return vec![format!("{loc}{chan}")];
    }

    // Normalise a bare band code ("H") to the family prefix ("HN").
    let family = match prefix.as_str() {
        "H" => "HN",
        "B" => "BN",
        "L" => "LN",
        other => other,
    };

    ['Z', 'N', 'E']
        .iter()
        .map(|comp| format!("{loc}{family}{comp}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_channel_passthrough() {
        assert_eq!(expand_tau_selectors("00", "HNZ"), vec!["00HNZ"]);
    }

    #[test]
    fn wildcard_expands_to_triad() {
        assert_eq!(
            expand_tau_selectors("00", "HN?"),
            vec!["00HNZ", "00HNN", "00HNE"]
        );
        assert_eq!(
            expand_tau_selectors("10", "H??"),
            vec!["10HNZ", "10HNN", "10HNE"]
        );
    }

    #[test]
    fn bare_band_normalises() {
        assert_eq!(
            expand_tau_selectors("00", "B?"),
            vec!["00BNZ", "00BNN", "00BNE"]
        );
    }
}
