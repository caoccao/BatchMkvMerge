/*
 *   Copyright (c) 2026. caoccao.com Sam Cao
 *   All rights reserved.

 *   Licensed under the Apache License, Version 2.0 (the "License");
 *   you may not use this file except in compliance with the License.
 *   You may obtain a copy of the License at

 *   http://www.apache.org/licenses/LICENSE-2.0

 *   Unless required by applicable law or agreed to in writing, software
 *   distributed under the License is distributed on an "AS IS" BASIS,
 *   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 *   See the License for the specific language governing permissions and
 *   limitations under the License.
 */

//! DVB Service descriptor (tag 0x48) — ETSI EN 300 468 §6.2.33.
//!
//! Body layout:
//!
//! ```text
//! u8 service_type
//! u8 service_provider_name_length
//! [service_provider_name_length bytes of DVB string]
//! u8 service_name_length
//! [service_name_length bytes of DVB string]
//! ```
//!
//! DVB strings can carry an encoding prefix byte (0x01..=0x1F) — we strip it
//! and treat the remainder as Latin-1 / UTF-8 best-effort.

pub fn decode(body: &[u8]) -> Option<String> {
    if body.len() < 2 {
        return None;
    }
    // skip service_type (1) + provider_name
    let provider_len = body[1] as usize;
    let after_provider = 2 + provider_len;
    if after_provider >= body.len() {
        return None;
    }
    let service_name_len = body[after_provider] as usize;
    let name_start = after_provider + 1;
    let name_end = name_start + service_name_len;
    if name_end > body.len() {
        return None;
    }
    let raw = &body[name_start..name_end];
    Some(decode_dvb_string(raw))
}

fn decode_dvb_string(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    let body = if matches!(bytes[0], 0x01..=0x1F) {
        &bytes[1..]
    } else {
        bytes
    };
    // Try UTF-8 first; fall back to Latin-1 lossy.
    match std::str::from_utf8(body) {
        Ok(s) => s.to_string(),
        Err(_) => body.iter().map(|b| *b as char).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(service_type: u8, provider: &[u8], service_name: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.push(service_type);
        v.push(provider.len() as u8);
        v.extend_from_slice(provider);
        v.push(service_name.len() as u8);
        v.extend_from_slice(service_name);
        v
    }

    #[test]
    fn extracts_service_name_utf8() {
        let b = body(0x01, b"BBC", b"BBC One");
        assert_eq!(decode(&b).as_deref(), Some("BBC One"));
    }

    #[test]
    fn strips_dvb_charset_prefix() {
        let b = body(0x01, b"BBC", &[0x05, b'B', b'B', b'C']); // 0x05 = ISO-8859-9
        assert_eq!(decode(&b).as_deref(), Some("BBC"));
    }

    #[test]
    fn rejects_truncated_body() {
        assert!(decode(&[0x01]).is_none());
        assert!(decode(&[0x01, 5, b'a', b'b']).is_none());
    }

    #[test]
    fn returns_empty_when_service_name_zero_length() {
        let b = body(0x01, b"BBC", b"");
        assert_eq!(decode(&b).as_deref(), Some(""));
    }
}
