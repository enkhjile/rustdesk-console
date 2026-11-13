//! Connection metadata is server-owned and never participates in report identity.
use axum::http::HeaderMap;
use serde_json::{json, Value};
use std::net::IpAddr;

struct DatabaseCache {
    path: std::path::PathBuf,
    modified: std::time::SystemTime,
    length: u64,
    reader: maxminddb::Reader<Vec<u8>>,
}
static DATABASE: std::sync::Mutex<Option<DatabaseCache>> = std::sync::Mutex::new(None);

pub fn trusted_proxies() -> Vec<IpAddr> {
    std::env::var("CONSOLE_AGENT_TRUSTED_PROXIES")
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect()
}

pub fn source_ip(peer: Option<IpAddr>, headers: &HeaderMap, trusted: &[IpAddr]) -> Option<IpAddr> {
    let mut source = peer?;
    if !trusted.contains(&source) {
        return Some(source);
    }
    // Walk from the nearest hop; never trust an arbitrary leftmost client value.
    if let Some(value) = headers.get("x-forwarded-for").and_then(|h| h.to_str().ok()) {
        if value.len() > 2048 {
            return Some(source);
        }
        let hops: Result<Vec<IpAddr>, _> = value.split(',').map(|s| s.trim().parse()).collect();
        if let Ok(hops) = hops {
            for hop in hops.into_iter().rev() {
                if !trusted.contains(&source) {
                    break;
                }
                source = hop;
            }
        }
    }
    Some(source)
}

pub fn location(config: &crate::config::Config, ip: Option<IpAddr>) -> Value {
    let Some(ip) = ip else { return Value::Null };
    if ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || matches!(ip, IpAddr::V4(v) if v.is_private() || v.is_link_local())
        || matches!(ip, IpAddr::V6(v) if v.is_unique_local() || v.is_unicast_link_local())
    {
        return Value::Null;
    }
    let Some(directory) = std::path::Path::new(config.rustdesk.key_file.trim()).parent() else {
        return Value::Null;
    };
    // Reuse the City database managed by Console's Geo routing page. No HTTP
    // lookup, API key, or dependency on hbbs being reachable is required.
    let path = directory.join("GeoLite2-City.mmdb");
    let Ok(metadata) = std::fs::metadata(&path) else {
        return Value::Null;
    };
    let Ok(modified) = metadata.modified() else {
        return Value::Null;
    };
    let Ok(mut cached) = DATABASE.lock() else {
        return Value::Null;
    };
    if cached
        .as_ref()
        .is_none_or(|c| c.path != path || c.modified != modified || c.length != metadata.len())
    {
        let Ok(reader) = maxminddb::Reader::open_readfile(&path) else {
            return Value::Null;
        };
        *cached = Some(DatabaseCache {
            path,
            modified,
            length: metadata.len(),
            reader,
        });
    }
    let Ok(result) = cached.as_ref().unwrap().reader.lookup(ip) else {
        return Value::Null;
    };
    let Ok(Some(record)) = result.decode::<Value>() else {
        return Value::Null;
    };
    location_fields(&record)
}

fn location_fields(record: &Value) -> Value {
    let country = record
        .pointer("/country/names/en")
        .and_then(Value::as_str)
        .or_else(|| record.pointer("/country/iso_code").and_then(Value::as_str));
    let region = record
        .pointer("/subdivisions/0/names/en")
        .and_then(Value::as_str);
    let city = record.pointer("/city/names/en").and_then(Value::as_str);
    if country.is_none() && region.is_none() && city.is_none() {
        return Value::Null;
    }
    json!({"country":country,"region":region,"city":city})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn forwarded_headers_require_a_trusted_peer_and_stop_at_untrusted_hop() {
        let proxy = "127.0.0.1".parse().unwrap();
        let remote = "203.0.113.42".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "1.2.3.4, 203.0.113.42".parse().unwrap());
        assert_eq!(source_ip(Some(remote), &headers, &[proxy]), Some(remote));
        assert_eq!(source_ip(Some(proxy), &headers, &[proxy]), Some(remote));
        assert_eq!(source_ip(None, &headers, &[proxy]), None);
        headers.insert("x-forwarded-for", "bad".parse().unwrap());
        assert_eq!(source_ip(Some(proxy), &headers, &[proxy]), Some(proxy));
    }
    #[test]
    fn missing_geo_data_is_not_a_location() {
        assert_eq!(location_fields(&json!({})), Value::Null);
        assert_eq!(
            location_fields(&json!({"country":{"iso_code":"MN"}})),
            json!({"country":"MN","region":null,"city":null})
        );
        assert_eq!(
            location_fields(
                &json!({"city":{"names":{"en":"Ulaanbaatar"}},"country":{"names":{"en":"Mongolia"}}})
            ),
            json!({"country":"Mongolia","region":null,"city":"Ulaanbaatar"})
        );
    }
}
