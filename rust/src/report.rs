//! The self-contained HTML report: one file, no network, no fonts, no frameworks.
//!
//! Only differing cells are embedded, and the payload is gzip then base64, which
//! the browser decodes natively with `DecompressionStream`. `--no-compress`
//! writes plain JSON for anything older than about 2023.
//!
//! The template is byte for byte the one the Python, TypeScript, Java and Go
//! implementations use, embedded here so the binary is the only thing to ship.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use flate2::Compression;
use flate2::write::GzEncoder;
use std::io::Write;

use crate::contract::CompareResult;
use crate::error::Result;

const TEMPLATE: &str = include_str!("report.html");

/// Renders the report.
///
/// Pass `compress = false` to embed plain JSON instead of the gzip payload.
pub fn render(result: &CompareResult, compress: bool) -> Result<String> {
    let raw = serde_json::to_string(result)?;

    let (payload, mode) = if compress {
        // Level 6, not 9. On a 60,000-change report -- 3.7 MB of JSON, the
        // largest this cap allows -- level 9 buys 0.71% of file size for 29% of
        // the report's whole render time, and it is the one part of a run that
        // cannot be spread across cores. 1.163 MB against 1.154 MB is not worth
        // a third of the tail; the size invariant this project cares about is
        // that only differing cells are embedded, which is what keeps the
        // payload near a megabyte at all.
        let mut encoder = GzEncoder::new(Vec::new(), Compression::new(6));
        encoder.write_all(raw.as_bytes())?;
        (BASE64.encode(encoder.finish()?), "gzip")
    } else {
        // The payload sits inside a <script> element, so a literal "</" would end
        // it early.
        (raw.replace("</", "<\\/"), "json")
    };

    let title = format!("{} vs {}", result.meta.a.name, result.meta.b.name);
    Ok(TEMPLATE
        .replace("__TITLE__", &escape_html(&title))
        .replace("__MODE__", mode)
        .replace("__PAYLOAD__", &payload))
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}
