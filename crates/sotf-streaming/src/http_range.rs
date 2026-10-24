// Rust guideline compliant 2026-02-21

use reqwest::header::{CONTENT_RANGE, HeaderMap};
use std::io;

pub(crate) struct ContentRange {
    pub(crate) start: u64,
    pub(crate) end: u64,
    pub(crate) total: Option<u64>,
}

pub(crate) fn parse_content_range(headers: &HeaderMap) -> io::Result<ContentRange> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid HTTP Content-Range");
    let value = headers
        .get(CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(invalid)?;
    let (unit, positions) = value.split_once(' ').ok_or_else(invalid)?;
    if unit != "bytes" {
        return Err(invalid());
    }
    let (span, total) = positions.split_once('/').ok_or_else(invalid)?;
    let (start, end) = span.split_once('-').ok_or_else(invalid)?;
    let start = start.parse::<u64>().map_err(|_| invalid())?;
    let end = end.parse::<u64>().map_err(|_| invalid())?;
    if end < start {
        return Err(invalid());
    }
    let total = if total == "*" {
        None
    } else {
        let total = total.parse::<u64>().map_err(|_| invalid())?;
        if total <= end {
            return Err(invalid());
        }
        Some(total)
    };
    Ok(ContentRange { start, end, total })
}
