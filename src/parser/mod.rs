//! miniSEED fixed-section data header (FSDH) parsing.
//!
//! Header-only parser: enough for routing, continuity checking, and display
//! without decoding the data payload (STEIM-1/2 decompression lives in
//! gaia_dsp; this crate stays dependency-light for reuse).
//!
//! FSDH layout (big-endian):
//! ```text
//!  0..6   sequence number (ASCII)
//!  6      data quality indicator (D/R/Q/M)
//!  7      reserved
//!  8..13  station code (ASCII)
//! 13..15  location id (ASCII)
//! 15..18  channel code (ASCII)
//! 18..20  network code (ASCII)
//! 20..30  BTIME: yr(u16) doy(u16) hr min sec unused tenth-ms(u16)
//! 30..32  number of samples (u16)
//! 32..34  sample rate factor (i16)
//! 34..36  sample rate multiplier (i16)
//! ```

use crate::protocol::{SeedLinkError, MSEED_RECORD_SIZE};

/// Parsed miniSEED record header plus the raw 512-byte record.
#[derive(Debug, Clone)]
pub struct MiniSeedHeader {
    pub sequence: String,
    pub quality: char,
    pub station: String,
    pub location: String,
    pub channel: String,
    pub network: String,
    /// Record start time: (year, day-of-year 1-based, hour, min, sec, tenth-ms).
    pub start: (u16, u16, u8, u8, u8, u16),
    pub num_samples: u16,
    /// Effective sample rate in Hz (0.0 for non-data records such as LOG).
    pub sample_rate: f64,
}

impl MiniSeedHeader {
    /// Parse the FSDH from a 512-byte miniSEED record.
    pub fn parse(record: &[u8]) -> Result<Self, SeedLinkError> {
        if record.len() < MSEED_RECORD_SIZE {
            return Err(SeedLinkError::Parse(format!(
                "record too short: {} bytes (expected {MSEED_RECORD_SIZE})",
                record.len()
            )));
        }

        let ascii = |r: &[u8]| String::from_utf8_lossy(r).trim().to_string();
        let be_u16 = |o: usize| u16::from_be_bytes([record[o], record[o + 1]]);
        let be_i16 = |o: usize| i16::from_be_bytes([record[o], record[o + 1]]);

        let factor = be_i16(32) as f64;
        let multiplier = be_i16(34) as f64;
        let sample_rate = if factor == 0.0 || multiplier == 0.0 {
            0.0
        } else {
            factor * multiplier
        };

        Ok(Self {
            sequence: ascii(&record[0..6]),
            quality: record[6] as char,
            station: ascii(&record[8..13]),
            location: ascii(&record[13..15]),
            channel: ascii(&record[15..18]),
            network: ascii(&record[18..20]),
            start: (
                be_u16(20),
                be_u16(22),
                record[24],
                record[25],
                record[26],
                be_u16(28),
            ),
            num_samples: be_u16(30),
            sample_rate,
        })
    }

    /// True for LOG records (no waveform payload).
    pub fn is_log(&self) -> bool {
        self.channel == "LOG"
    }

    /// `NET.STA.LOC.CHAN` stream identifier.
    pub fn stream_id(&self) -> String {
        format!(
            "{}.{}.{}.{}",
            self.network, self.station, self.location, self.channel
        )
    }

    /// Record start time as Unix seconds (f64). Leap-year correct; DOY is
    /// 1-based per SEED convention; tenth-milliseconds scaled by 1e-4.
    pub fn start_unix(&self) -> f64 {
        let (year, doy, h, m, s, tms) = self.start;
        let mut days: i64 = 0;
        for y in 1970..year {
            days += if is_leap(y) { 366 } else { 365 };
        }
        days += (doy as i64) - 1;
        let secs = days * 86_400 + (h as i64) * 3600 + (m as i64) * 60 + s as i64;
        secs as f64 + (tms as f64) * 1e-4
    }
}

fn is_leap(y: u16) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic 512-byte record with a known header.
    fn fixture() -> Vec<u8> {
        let mut r = vec![b' '; MSEED_RECORD_SIZE];
        r[0..6].copy_from_slice(b"000042");
        r[6] = b'D';
        r[8..13].copy_from_slice(b"TEST1");
        r[13..15].copy_from_slice(b"00");
        r[15..18].copy_from_slice(b"HNZ");
        r[18..20].copy_from_slice(b"XX");
        r[20..22].copy_from_slice(&2026u16.to_be_bytes());
        r[22..24].copy_from_slice(&238u16.to_be_bytes()); // DOY
        r[24] = 8;
        r[25] = 4;
        r[26] = 44;
        r[28..30].copy_from_slice(&8450u16.to_be_bytes()); // tenth-ms -> 0.8450s
        r[30..32].copy_from_slice(&532u16.to_be_bytes());
        r[32..34].copy_from_slice(&200i16.to_be_bytes());
        r[34..36].copy_from_slice(&1i16.to_be_bytes());
        r
    }

    #[test]
    fn parses_header_fields() {
        let h = MiniSeedHeader::parse(&fixture()).unwrap();
        assert_eq!(h.sequence, "000042");
        assert_eq!(h.stream_id(), "XX.TEST1.00.HNZ");
        assert_eq!(h.num_samples, 532);
        assert_eq!(h.sample_rate, 200.0);
        assert!(!h.is_log());
    }

    #[test]
    fn start_time_unix() {
        let h = MiniSeedHeader::parse(&fixture()).unwrap();
        // 2026-08-26 is DOY 238; 08:04:44.845 UTC.
        assert_eq!(h.start, (2026, 238, 8, 4, 44, 8450));
        let expected = {
            // Cross-check via days-from-epoch arithmetic.
            let days = (1970..2026)
                .map(|y| if is_leap(y) { 366 } else { 365 })
                .sum::<i64>()
                + 237;
            days as f64 * 86_400.0 + 8.0 * 3600.0 + 4.0 * 60.0 + 44.0 + 0.845
        };
        assert!((h.start_unix() - expected).abs() < 1e-6);
    }

    #[test]
    fn rejects_short_record() {
        assert!(MiniSeedHeader::parse(&[0u8; 100]).is_err());
    }

    #[test]
    fn detects_log_channel() {
        let mut r = fixture();
        r[15..18].copy_from_slice(b"LOG");
        r[32..34].copy_from_slice(&0i16.to_be_bytes());
        let h = MiniSeedHeader::parse(&r).unwrap();
        assert!(h.is_log());
        assert_eq!(h.sample_rate, 0.0);
    }
}
