//! Read-only HTTP overview and JSON API for a YuanLai node.
//!
//! Normative API definition: `spec/08-api.md`.

use std::io::{self, BufRead, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use yuanlai_consensus::Chain;
use yuanlai_consensus::genesis::genesis_block;
use yuanlai_storage::read_blocks;
use yuanlai_types::{Block, Network};

const MAX_REQUEST_LINE: usize = 2_048;
const MAX_HEADER_LINE: usize = 2_048;
const MAX_HEADER_BYTES: usize = 8_192;
const MAX_HEADERS: usize = 32;

const DASHBOARD: &str = include_str!("../dashboard.html");

struct Request {
    method: String,
    path: String,
    has_body: bool,
}

#[derive(Default)]
struct OverviewCache {
    initialized: bool,
    fingerprint: Option<(u64, SystemTime)>,
    body: Option<Vec<u8>>,
}

impl OverviewCache {
    fn get(&mut self, network: Network, blocks_path: &Path) -> Result<Vec<u8>, String> {
        let fingerprint = match std::fs::metadata(blocks_path) {
            Ok(metadata) => Some((
                metadata.len(),
                metadata.modified().map_err(|error| {
                    format!("cannot read block file modification time: {error}")
                })?,
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("cannot inspect block file: {error}")),
        };
        if self.initialized
            && fingerprint == self.fingerprint
            && let Some(body) = &self.body
        {
            return Ok(body.clone());
        }

        let blocks = read_blocks(blocks_path).map_err(|error| error.to_string())?;
        let body =
            serde_json::to_vec(&overview(network, &blocks)?).map_err(|error| error.to_string())?;
        self.fingerprint = fingerprint;
        self.body = Some(body.clone());
        self.initialized = true;
        Ok(body)
    }
}

fn read_limited_line<R: BufRead>(reader: &mut R, limit: usize) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    let read = (&mut *reader)
        .take(u64::try_from(limit.saturating_add(1)).unwrap_or(u64::MAX))
        .read_until(b'\n', &mut line)?;
    if read == 0 {
        return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
    }
    if line.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "HTTP line exceeds limit",
        ));
    }
    Ok(line)
}

fn read_request(stream: &TcpStream) -> io::Result<Request> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = io::BufReader::new(stream.try_clone()?);
    let request_line = read_limited_line(&mut reader, MAX_REQUEST_LINE)?;
    let request_line = std::str::from_utf8(&request_line)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "request line is not UTF-8"))?;
    let mut parts = request_line.split_ascii_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing HTTP method"))?;
    let path = parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing HTTP path"))?;
    let version = parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing HTTP version"))?;
    if parts.next().is_some() || !matches!(version, "HTTP/1.0" | "HTTP/1.1") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "malformed HTTP request line",
        ));
    }

    let mut header_bytes = request_line.len();
    let mut has_body = false;
    for _ in 0..=MAX_HEADERS {
        let line = read_limited_line(&mut reader, MAX_HEADER_LINE)?;
        header_bytes = header_bytes.saturating_add(line.len());
        if header_bytes > MAX_HEADER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP headers exceed limit",
            ));
        }
        if line == b"\r\n" || line == b"\n" {
            return Ok(Request {
                method: method.to_owned(),
                path: path.to_owned(),
                has_body,
            });
        }
        let line = std::str::from_utf8(&line)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "header is not UTF-8"))?;
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "malformed HTTP header"))?;
        if name.is_empty() || name.trim() != name {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "malformed HTTP header name",
            ));
        }
        if name.eq_ignore_ascii_case("content-length") {
            let content_length = value.trim().parse::<u64>().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid content length")
            })?;
            has_body |= content_length != 0;
        }
        if name.eq_ignore_ascii_case("transfer-encoding") {
            has_body = true;
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "too many HTTP headers",
    ))
}

fn overview(network: Network, blocks: &[Block]) -> Result<Value, String> {
    let genesis = genesis_block(network).ok_or("genesis block is not configured")?;
    let mut chain = Chain::new(network.params(), &genesis).map_err(|error| error.to_string())?;
    for block in blocks {
        chain.connect(block).map_err(|error| error.to_string())?;
    }
    let totals = chain.state().totals();
    let unspent = chain
        .state()
        .utxos()
        .try_fold(yuanlai_types::Credit::ZERO, |total, (_, utxo)| {
            total.checked_add(utxo.amount)
        })
        .ok_or("unspent credit total overflow")?;
    let recent_blocks = std::iter::once(&genesis)
        .chain(blocks.iter())
        .rev()
        .take(16)
        .map(|block| {
            json!({
                "height": block.header.height.0.to_string(),
                "id": block.header.id().to_string(),
                "timestamp": block.header.timestamp.0.to_string(),
                "transactions": block.txs.len().to_string(),
                "target": block.header.target.0.to_string(),
            })
        })
        .collect::<Vec<_>>();
    let transaction_count = std::iter::once(&genesis)
        .chain(blocks.iter())
        .try_fold(0_u64, |count, block| {
            count.checked_add(u64::from(block.txs.len()))
        })
        .ok_or("transaction count overflow")?;

    Ok(json!({
        "network": network.name(),
        "chain_id": network.chain_id().to_string(),
        "height": chain.height().0.to_string(),
        "tip": chain.tip().id().to_string(),
        "tip_timestamp": chain.tip().timestamp.0.to_string(),
        "target": chain.tip().target.0.to_string(),
        "block_count": chain.height().0.saturating_add(1).to_string(),
        "transaction_count": transaction_count.to_string(),
        "credit": {
            "minted": totals.minted.0.to_string(),
            "burned": totals.burned.0.to_string(),
            "expired": totals.expired.0.to_string(),
            "unspent": unspent.0.to_string(),
        },
        "recent_blocks": recent_blocks,
    }))
}

fn respond(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}

fn handle_connection(
    mut stream: TcpStream,
    network: Network,
    blocks_path: &Path,
    cache: &mut OverviewCache,
) -> io::Result<()> {
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let request = match read_request(&stream) {
        Ok(request) => request,
        Err(error) => {
            let body = error.to_string();
            return respond(
                &mut stream,
                "400 Bad Request",
                "text/plain; charset=utf-8",
                body.as_bytes(),
            );
        }
    };
    if request.method != "GET" || request.has_body {
        return respond(
            &mut stream,
            "405 Method Not Allowed",
            "text/plain; charset=utf-8",
            b"GET without a request body is required",
        );
    }
    match request.path.as_str() {
        "/" => respond(
            &mut stream,
            "200 OK",
            "text/html; charset=utf-8",
            DASHBOARD.as_bytes(),
        ),
        "/api/overview" => match cache.get(network, blocks_path) {
            Ok(body) => respond(
                &mut stream,
                "200 OK",
                "application/json; charset=utf-8",
                &body,
            ),
            Err(error) => respond(
                &mut stream,
                "503 Service Unavailable",
                "application/json; charset=utf-8",
                &json!({ "error": error }).to_string().into_bytes(),
            ),
        },
        _ => respond(
            &mut stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"not found",
        ),
    }
}

/// Serves the read-only project dashboard and `/api/overview` from the node's active block file.
///
/// The server binds only to the supplied address. Use a loopback address for local inspection;
/// this API does not authenticate clients.
///
/// # Errors
///
/// Returns an I/O error if the listener cannot bind or accept connections.
pub fn serve(address: &str, network: Network, datadir: &Path) -> io::Result<()> {
    let listener = TcpListener::bind(address)?;
    serve_listener(&listener, network, datadir)
}

/// Serves requests using an already-bound listener.
///
/// # Errors
///
/// Returns an I/O error if the listener cannot accept connections.
pub fn serve_listener(listener: &TcpListener, network: Network, datadir: &Path) -> io::Result<()> {
    let blocks_path = datadir.join("blocks.dat");
    let mut cache = OverviewCache::default();
    eprintln!(
        "YuanLai overview available at http://{}",
        listener.local_addr()?
    );
    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                if let Err(error) = handle_connection(stream, network, &blocks_path, &mut cache) {
                    eprintln!("dashboard request failed: {error}");
                }
            }
            Err(error) => eprintln!("dashboard accept failed: {error}"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    use yuanlai_types::Network;

    use super::{OverviewCache, handle_connection, overview};

    #[test]
    fn overview_reports_real_genesis_chain_totals_and_recent_blocks()
    -> Result<(), Box<dyn std::error::Error>> {
        let network = Network::Regtest;
        let result = overview(network, &[])?;
        assert_eq!(result["network"], network.name());
        assert_eq!(result["height"], "0");
        assert_eq!(result["block_count"], "1");
        assert_eq!(result["transaction_count"], "1");
        assert_eq!(result["recent_blocks"][0]["height"], "0");
        assert_eq!(result["credit"]["minted"], "0");
        Ok(())
    }

    #[test]
    fn overview_route_returns_json_over_http() -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let mut client = TcpStream::connect(address)?;
        let (server, _) = listener.accept()?;
        client.write_all(b"GET /api/overview HTTP/1.1\r\nHost: localhost\r\n\r\n")?;

        let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let datadir =
            std::env::temp_dir().join(format!("yuanlai-rpc-test-{}-{unique}", std::process::id()));
        handle_connection(
            server,
            Network::Regtest,
            &datadir.join("blocks.dat"),
            &mut OverviewCache::default(),
        )?;

        let mut response = Vec::new();
        client.read_to_end(&mut response)?;
        let response = std::str::from_utf8(&response)?;
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("\"height\":\"0\""));
        Ok(())
    }

    #[test]
    fn overview_route_rejects_get_requests_with_transfer_encoding()
    -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let mut client = TcpStream::connect(address)?;
        let (server, _) = listener.accept()?;
        client.write_all(
            b"GET /api/overview HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n",
        )?;

        handle_connection(
            server,
            Network::Regtest,
            Path::new("/missing-yuanlai-rpc-test/blocks.dat"),
            &mut OverviewCache::default(),
        )?;
        let mut response = Vec::new();
        client.read_to_end(&mut response)?;
        let response = std::str::from_utf8(&response)?;
        assert!(response.starts_with("HTTP/1.1 405 Method Not Allowed\r\n"));
        Ok(())
    }
}
