use std::time::Duration;

use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tracing::Instrument;

use crate::termtext::sanitize;

/// Errors from talking to the PDS. `Display` matches Go: `nsid: message (error)`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The server answered with a non-200 status.
    #[error("{0}")]
    Api(String),
    #[error("{0}")]
    Transport(String),
    #[error("{0}")]
    Decode(String),
}

pub struct Client {
    base: String,
    http: reqwest::Client,
    jwt: String,
    pub did: String,
    /// Your handle as the server reports it.
    pub handle: String,
}

impl Client {
    pub fn new(base: &str) -> Client {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("HTTP client");
        Client {
            base: base.trim_end_matches('/').to_string(),
            http,
            jwt: String::new(),
            did: String::new(),
            handle: String::new(),
        }
    }

    fn url(&self, nsid: &str, query: &[(&str, String)]) -> String {
        let mut u = format!("{}/xrpc/{}", self.base, nsid);
        if !query.is_empty() {
            // Go's url.Values.Encode: keys sorted, values in insertion order.
            let mut q: Vec<&(&str, String)> = query.iter().collect();
            q.sort_by(|a, b| a.0.cmp(b.0));
            u.push('?');
            u.push_str(&encode_query(&q));
        }
        u
    }

    /// Sends a request. Telemetry (when a subscriber is installed) records only the operation
    /// name, status, outcome and, for API errors, the XRPC error *code*: never the URL, query,
    /// body, tokens or error messages, which can contain handles and DIDs.
    async fn send(&self, nsid: &str, req: reqwest::RequestBuilder) -> Result<Vec<u8>, Error> {
        let span = tracing::info_span!(
            "xrpc",
            logfire.msg = %format_args!("xrpc {nsid}"),
            nsid,
            http.response.status_code = tracing::field::Empty,
            outcome = tracing::field::Empty,
            error.code = tracing::field::Empty,
        );
        self.send_inner(nsid, req, &span)
            .instrument(span.clone())
            .await
    }

    async fn send_inner(
        &self,
        nsid: &str,
        req: reqwest::RequestBuilder,
        span: &tracing::Span,
    ) -> Result<Vec<u8>, Error> {
        let req = if self.jwt.is_empty() {
            req
        } else {
            req.bearer_auth(&self.jwt)
        };
        let resp = req.send().await.map_err(|e| {
            span.record("outcome", "transport_error");
            Error::Transport(chain(&e))
        })?;
        let status = resp.status();
        span.record("http.response.status_code", status.as_u16());
        let data = resp.bytes().await.map_err(|e| {
            span.record("outcome", "transport_error");
            Error::Transport(chain(&e))
        })?;
        if status != StatusCode::OK {
            let v: Value = serde_json::from_slice(&data).unwrap_or(Value::Null);
            let field = |name: &str| -> String {
                v.as_object()
                    .and_then(|o| o.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)))
                    .and_then(|(_, v)| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let (err, msg) = (field("error"), field("message"));
            span.record("outcome", "api_error");
            if !err.is_empty() {
                span.record("error.code", sanitize(&err).as_str());
            }
            return Err(Error::Api(if err.is_empty() {
                format!("{nsid}: HTTP {}", status.as_u16())
            } else {
                format!("{nsid}: {} ({})", sanitize(&msg), sanitize(&err))
            }));
        }
        span.record("outcome", "ok");
        Ok(data.to_vec())
    }

    pub(crate) async fn get<T: DeserializeOwned>(
        &self,
        nsid: &str,
        query: &[(&str, String)],
    ) -> Result<T, Error> {
        let data = self
            .send(nsid, self.http.get(self.url(nsid, query)))
            .await?;
        serde_json::from_slice(&data).map_err(|e| Error::Decode(e.to_string()))
    }

    /// POSTs `body` as JSON to `nsid`; returns the raw 200 response.
    pub(crate) async fn post_raw(
        &self,
        nsid: &str,
        body: &serde_json::Value,
    ) -> Result<Vec<u8>, Error> {
        let req = self
            .http
            .post(self.url(nsid, &[]))
            .header("Content-Type", "application/json")
            .body(body.to_string());
        self.send(nsid, req).await
    }

    pub(crate) async fn post<T: DeserializeOwned>(
        &self,
        nsid: &str,
        body: &serde_json::Value,
    ) -> Result<T, Error> {
        let data = self.post_raw(nsid, body).await?;
        serde_json::from_slice(&data).map_err(|e| Error::Decode(e.to_string()))
    }

    pub async fn login(&mut self, identifier: &str, password: &str) -> Result<(), Error> {
        #[derive(serde::Deserialize, Default)]
        #[serde(default)]
        struct Out {
            #[serde(rename = "accessJwt", deserialize_with = "super::nz")]
            access_jwt: String,
            #[serde(deserialize_with = "super::nz")]
            did: String,
            #[serde(deserialize_with = "super::nz")]
            handle: String,
        }
        let nsid = "com.atproto.server.createSession";
        let body = serde_json::json!({"identifier": identifier, "password": password});
        let req = self
            .http
            .post(self.url(nsid, &[]))
            .header("Content-Type", "application/json")
            .body(body.to_string());
        let data = self.send(nsid, req).await?;
        let out: Out = serde_json::from_slice(&data).map_err(|e| Error::Decode(e.to_string()))?;
        self.jwt = out.access_jwt;
        self.did = out.did;
        self.handle = out.handle;
        Ok(())
    }
}

fn chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        s.push_str(": ");
        s.push_str(&c.to_string());
        cur = c.source();
    }
    s
}

/// Percent-encodes like Go's `url.QueryEscape` (space becomes `+`).
fn encode_query(pairs: &[&(&str, String)]) -> String {
    fn esc(s: &str, out: &mut String) {
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(b as char)
                }
                b' ' => out.push('+'),
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
    }
    let mut out = String::new();
    for (i, (k, v)) in pairs.iter().map(|p| (&p.0, &p.1)).enumerate() {
        if i > 0 {
            out.push('&');
        }
        esc(k, &mut out);
        out.push('=');
        esc(v, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_encoding_matches_go() {
        let c = Client::new("http://x/");
        let u = c.url(
            "a.b",
            &[
                ("limit", "100".into()),
                ("cursor", "a b/c+d".into()),
                ("actors", "x".into()),
                ("actors", "y".into()),
            ],
        );
        assert_eq!(
            u,
            "http://x/xrpc/a.b?actors=x&actors=y&cursor=a+b%2Fc%2Bd&limit=100"
        );
    }
}
