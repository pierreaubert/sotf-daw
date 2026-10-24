use super::consts::USER_AGENT;
use super::hls_byte_range::HlsByteRange;
use super::hls_segment::HlsSegment;
use crate::http_range::parse_content_range;
use reqwest::blocking::Client;
use reqwest::header::RANGE;
use reqwest::StatusCode;
use std::io::{self, Read};
use url::Url;

pub(super) fn fetch_text(client: &Client, url: &Url, max_bytes: usize) -> io::Result<String> {
    let bytes = fetch_bytes(client, url, None, max_bytes)?;
    String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
}

pub(super) fn fetch_segment(
    client: &Client,
    segment: &HlsSegment,
    max_bytes: usize,
) -> io::Result<Vec<u8>> {
    fetch_bytes(client, &segment.url, segment.byte_range, max_bytes)
}

pub(super) fn fetch_bytes(
    client: &Client,
    url: &Url,
    byte_range: Option<HlsByteRange>,
    max_bytes: usize,
) -> io::Result<Vec<u8>> {
    let mut request = client.get(url.clone()).header("User-Agent", USER_AGENT);
    if let Some(range) = byte_range {
        request = request.header(RANGE, range.header_value()?);
    }

    let response = request
        .send()
        .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e.to_string()))?;

    if !response.status().is_success() {
        return Err(io::Error::other(format!(
            "HTTP {} for {}",
            response.status(),
            url
        )));
    }

    if let Some(range) = byte_range {
        let content_range = parse_content_range(response.headers())?;
        let expected_end = range
            .end_exclusive()?
            .checked_sub(1)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "empty HLS byte range"))?;
        if response.status() != StatusCode::PARTIAL_CONTENT
            || content_range.start != range.offset
            || content_range.end != expected_end
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP response does not match the HLS byte range",
            ));
        }
    }

    if response.content_length().is_some_and(|length| length > max_bytes as u64) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "HTTP body exceeds the configured size limit",
        ));
    }

    let mut bytes = Vec::new();
    let read_limit = u64::try_from(max_bytes).unwrap_or(u64::MAX).saturating_add(1);
    response
        .take(read_limit)
        .read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "HTTP body exceeds the configured size limit",
        ));
    }
    if let Some(range) = byte_range
        && bytes.len() as u64 != range.length
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "incomplete HLS byte-range response",
        ));
    }
    Ok(bytes)
}
