use anyhow::{Result, bail, ensure};
use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use url::Url;

pub const MAX_RESPONSE: usize = 5 * 1024 * 1024;
pub fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            let [x, y, z, _] = a.octets();
            !(x == 0
                || x == 10
                || x == 127
                || x >= 224
                || x == 169 && y == 254
                || x == 172 && (16..=31).contains(&y)
                || x == 192 && y == 168
                || x == 100 && (64..=127).contains(&y)
                || x == 198 && (y == 18 || y == 19)
                || x == 192 && y == 0
                || x == 192 && y == 2
                || x == 198 && y == 51 && z == 100
                || x == 203 && y == 0 && z == 113)
        }
        IpAddr::V6(a) => {
            let s = a.segments();
            if let Some(v4) = a.to_ipv4_mapped() {
                return public_ip(IpAddr::V4(v4));
            }
            (s[0] & 0xe000) == 0x2000 && !(s[0] == 0x2001 && (s[1] == 0xdb8 || s[1] < 0x200))
        }
    }
}
pub fn normalize_url(input: &str) -> Result<Url> {
    ensure!(input.len() <= 4096, "URL exceeds 4,096 characters");
    let mut u = Url::parse(input.trim())?;
    ensure!(
        matches!(u.scheme(), "http" | "https") && u.username().is_empty() && u.password().is_none(),
        "Use an HTTP(S) URL without credentials"
    );
    ensure!(
        u.port().is_none_or(|p| p == 80 || p == 443),
        "Only standard web ports are supported"
    );
    let host = u
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("URL has no host"))?;
    ensure!(
        !host.eq_ignore_ascii_case("localhost")
            && !host.to_ascii_lowercase().ends_with(".localhost")
            && !host.to_ascii_lowercase().ends_with(".local"),
        "Private hosts cannot be saved"
    );
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
        ensure!(public_ip(ip), "Private network addresses cannot be saved");
    }
    u.set_fragment(None);
    Ok(u)
}
#[derive(Clone, Debug)]
pub struct Resource {
    pub url: Url,
    pub body: Vec<u8>,
    pub content_type: String,
}
pub struct ConditionalResource {
    pub resource: Option<Resource>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}
pub async fn fetch(input: &str, image: bool) -> Result<Resource> {
    tokio::time::timeout(
        Duration::from_secs(if image { 8 } else { 25 }),
        fetch_inner(input, image, None, None, MAX_RESPONSE),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Download timed out"))??
    .resource
    .ok_or_else(|| anyhow::anyhow!("Unexpected empty publisher response"))
}
pub async fn fetch_icon(input: &str) -> Result<Resource> {
    tokio::time::timeout(
        Duration::from_secs(8),
        fetch_inner(input, true, None, None, 256 * 1024),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Icon download timed out"))??
    .resource
    .ok_or_else(|| anyhow::anyhow!("Empty icon response"))
}
pub async fn fetch_conditional(
    input: &str,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> Result<ConditionalResource> {
    for value in [etag, last_modified].into_iter().flatten() {
        ensure!(
            value.len() <= 1024 && !value.contains(['\r', '\n']),
            "Invalid feed validator"
        );
    }
    tokio::time::timeout(
        Duration::from_secs(25),
        fetch_inner(input, false, etag, last_modified, MAX_RESPONSE),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Download timed out"))?
}
async fn fetch_inner(
    input: &str,
    image: bool,
    etag: Option<&str>,
    last_modified: Option<&str>,
    max_bytes: usize,
) -> Result<ConditionalResource> {
    let mut url = normalize_url(input)?;
    for redirect in 0..=5 {
        let host = url
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("Missing hostname"))?
            .trim_matches(['[', ']'])
            .to_owned();
        let port = url
            .port_or_known_default()
            .ok_or_else(|| anyhow::anyhow!("Missing port"))?;
        let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
            .await?
            .collect();
        ensure!(
            !addresses.is_empty() && addresses.iter().all(|a| public_ip(a.ip())),
            "URL resolves to a private network"
        );
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(25))
            .resolve(&host, addresses[0])
            .build()?;
        let mut request=client.get(url.clone()).header("User-Agent","ReadLater/0.2 (+private article reader)").header("Accept",if image{"image/png,image/jpeg,image/webp,image/gif,image/x-icon,image/vnd.microsoft.icon"}else{"text/html,application/rss+xml,application/atom+xml,application/xml,text/xml,text/plain"}).header("Accept-Encoding","identity");
        if redirect == 0 {
            if let Some(value) = etag {
                request = request.header("If-None-Match", value);
            }
            if let Some(value) = last_modified {
                request = request.header("If-Modified-Since", value);
            }
        }
        let mut response = request.send().await?;
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .filter(|v| v.len() <= 1024)
                .map(str::to_owned)
        };
        let response_etag = header("etag");
        let response_modified = header("last-modified");
        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            ensure!(
                redirect == 0 && (etag.is_some() || last_modified.is_some()),
                "Unsolicited unchanged feed response"
            );
            return Ok(ConditionalResource {
                resource: None,
                etag: response_etag,
                last_modified: response_modified,
            });
        }
        if response.status().is_redirection() {
            ensure!(redirect < 5, "Too many redirects");
            let location = response
                .headers()
                .get("location")
                .ok_or_else(|| anyhow::anyhow!("Missing redirect location"))?
                .to_str()?;
            url = normalize_url(url.join(location)?.as_str())?;
            continue;
        }
        ensure!(
            response.status().is_success(),
            "Publisher returned HTTP {}",
            response.status()
        );
        let content_type = response
            .headers()
            .get("content-type")
            .ok_or_else(|| anyhow::anyhow!("Missing content type"))?
            .to_str()?
            .split(';')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if image {
            ensure!(
                matches!(
                    content_type.as_str(),
                    "image/png"
                        | "image/jpeg"
                        | "image/webp"
                        | "image/gif"
                        | "image/x-icon"
                        | "image/vnd.microsoft.icon"
                ),
                "Unsupported raster image format"
            );
        } else {
            ensure!(
                content_type.contains("html")
                    || content_type.contains("xml")
                    || content_type == "text/plain",
                "URL is not an HTML page or feed"
            );
        }
        ensure!(
            response
                .content_length()
                .is_none_or(|n| n <= max_bytes as u64),
            "Response exceeds its {max_bytes}-byte limit"
        );
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                body.len() + chunk.len() <= max_bytes,
                "Response exceeds its {max_bytes}-byte limit"
            );
            body.extend_from_slice(&chunk);
        }
        return Ok(ConditionalResource {
            resource: Some(Resource {
                url,
                body,
                content_type,
            }),
            etag: response_etag,
            last_modified: response_modified,
        });
    }
    bail!("Too many redirects")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_boundary() {
        for u in [
            "http://127.0.0.1/a",
            "https://[::1]",
            "https://192.168.1.1",
            "https://100.64.1.1",
            "https://example.local",
            "https://u:p@example.com",
            "https://example.com:8080",
        ] {
            assert!(normalize_url(u).is_err(), "{u}");
        }
        assert_eq!(
            normalize_url("https://Example.com/a#b").unwrap().as_str(),
            "https://example.com/a"
        );
        assert!(public_ip("8.8.8.8".parse().unwrap()));
    }
}
