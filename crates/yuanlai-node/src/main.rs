//! `yuanlai-node`: the YuanLai full node.
//!
//! Replays and re-validates the stored chain on startup; mines locally or serves bounded inbound
//! P2P with one-shot sync and optional persistent transaction relay, plus a read-only HTTP overview.

use std::collections::BTreeMap;
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rand::{TryRng, rngs::SysRng};
use yuanlai_codec::{BoundedBytes, BoundedVec, Decode, Encode, Reader, decode_exact};
use yuanlai_consensus::genesis::genesis_block;
use yuanlai_consensus::{Chain, HeaderTree};
use yuanlai_crypto::is_valid_public_key;
use yuanlai_mempool::{Mempool, SubmitError};
use yuanlai_p2p::{
    Handshake, HandshakeEvent, MAX_HEADERS_PER_MESSAGE, MAX_LOCATOR_HASHES, Message, read_message,
    write_message,
};
use yuanlai_storage::BlockStore;
use yuanlai_types::params::{MAX_BLOCK_SIZE, MAX_BLOCK_TXS};
use yuanlai_types::{Block, Network, PubKey, Timestamp, Tx};

const USAGE: &str = "\
usage: yuanlai-node [--network devnet|regtest] [--datadir PATH]
                    [--tx-file PATH ... (--mine --beneficiary HEX_PUBKEY | --connect ADDR ...)]
                    [--listen ADDR [--beneficiary HEX_PUBKEY] | --connect ADDR [--connect ADDR ...] [--stay-connected]]
                    [--api-listen ADDR]";

/// Nonces tried before refreshing the timestamp.
const NONCE_BATCH: u64 = 1 << 22;
const MAX_INBOUND_PEERS: usize = 8;
const MAX_INBOUND_PEERS_PER_IP: usize = 2;
const MAX_MEMPOOL_FILE_BYTES: usize = MAX_BLOCK_SIZE + 4;
const PEER_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const PEER_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const PEER_MESSAGE_TIMEOUT: Duration = Duration::from_secs(60);
const PEER_RESYNC_INTERVAL: Duration = Duration::from_secs(30);
const MAX_STATIC_PEERS: usize = 64;
const MAX_PEER_ADDRESS_BYTES: usize = 512;
const MAX_PEER_CONFIG_BYTES: usize = 64 * 1024;

#[derive(Default)]
struct PeerCounts {
    total: usize,
    by_ip: BTreeMap<IpAddr, usize>,
}

struct PeerSlot {
    counts: Arc<Mutex<PeerCounts>>,
    ip: IpAddr,
}

impl PeerSlot {
    fn acquire(shared_counts: &Arc<Mutex<PeerCounts>>, ip: IpAddr) -> Option<Self> {
        let ip = match ip {
            IpAddr::V6(address) => address
                .to_ipv4_mapped()
                .map_or(IpAddr::V6(address), IpAddr::V4),
            IpAddr::V4(address) => IpAddr::V4(address),
        };
        let mut counts = shared_counts.lock().ok()?;
        let per_ip = counts.by_ip.get(&ip).copied().unwrap_or_default();
        if counts.total >= MAX_INBOUND_PEERS || per_ip >= MAX_INBOUND_PEERS_PER_IP {
            return None;
        }
        counts.total = counts.total.checked_add(1)?;
        counts.by_ip.insert(ip, per_ip.checked_add(1)?);
        drop(counts);
        Some(Self {
            counts: Arc::clone(shared_counts),
            ip,
        })
    }
}

impl Drop for PeerSlot {
    fn drop(&mut self) {
        if let Ok(mut counts) = self.counts.lock() {
            counts.total = counts.total.saturating_sub(1);
            let remove_ip = if let Some(count) = counts.by_ip.get_mut(&self.ip) {
                *count = count.saturating_sub(1);
                *count == 0
            } else {
                false
            };
            if remove_ip {
                counts.by_ip.remove(&self.ip);
            }
        }
    }
}

#[derive(Default)]
struct PeerWriters {
    peers: Mutex<BTreeMap<SocketAddr, Arc<Mutex<TcpStream>>>>,
}

impl PeerWriters {
    fn register(&self, address: SocketAddr, writer: Arc<Mutex<TcpStream>>) -> Result<(), String> {
        self.peers
            .lock()
            .map_err(|_| "peer registry lock is poisoned".to_owned())?
            .insert(address, writer);
        Ok(())
    }

    fn remove(&self, address: SocketAddr) {
        if let Ok(mut peers) = self.peers.lock() {
            peers.remove(&address);
        }
    }

    fn broadcast(&self, source: SocketAddr, transaction: &Message) -> Result<(), String> {
        let peers = self
            .peers
            .lock()
            .map_err(|_| "peer registry lock is poisoned".to_owned())?
            .iter()
            .filter(|(address, _)| **address != source)
            .map(|(address, writer)| (*address, Arc::clone(writer)))
            .collect::<Vec<_>>();
        let mut failed = Vec::new();
        for (address, writer) in peers {
            let mut stream = writer
                .lock()
                .map_err(|_| "peer writer lock is poisoned".to_owned())?;
            if write_message(&mut *stream, transaction).is_err() {
                let _ = stream.shutdown(Shutdown::Both);
                failed.push((address, writer.clone()));
            }
        }
        if !failed.is_empty() {
            let mut peers = self
                .peers
                .lock()
                .map_err(|_| "peer registry lock is poisoned".to_owned())?;
            for (address, failed_writer) in failed {
                if peers
                    .get(&address)
                    .is_some_and(|writer| Arc::ptr_eq(writer, &failed_writer))
                {
                    peers.remove(&address);
                }
            }
        }
        Ok(())
    }
}

struct PeerRegistration {
    address: SocketAddr,
    writers: Arc<PeerWriters>,
}

impl Drop for PeerRegistration {
    fn drop(&mut self) {
        self.writers.remove(self.address);
    }
}

struct Options {
    network: Network,
    datadir: PathBuf,
    mine: Option<PubKey>,
    max_blocks: Option<u64>,
    tx_files: Vec<PathBuf>,
    listen: Option<String>,
    connect: Vec<String>,
    stay_connected: bool,
    api_listen: Option<String>,
    relay_beneficiary: Option<PubKey>,
}

fn parse_hex_key(s: &str) -> Result<PubKey, String> {
    let bytes = s.as_bytes();
    if bytes.len() != 64 {
        return Err("beneficiary must be 64 hex characters".into());
    }
    let mut out = [0_u8; 32];
    let (pairs, _) = bytes.as_chunks::<2>();
    for (byte, pair) in out.iter_mut().zip(pairs) {
        let text = std::str::from_utf8(pair).map_err(|_| "invalid hex")?;
        *byte = u8::from_str_radix(text, 16).map_err(|_| "invalid hex")?;
    }
    if !is_valid_public_key(&out) {
        return Err("beneficiary is not a valid Ed25519 public key (TX-CB-02)".into());
    }
    Ok(PubKey(out))
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut network = Network::Devnet;
    let mut datadir = None;
    let mut mine = false;
    let mut beneficiary = None;
    let mut max_blocks = None;
    let mut tx_files = Vec::new();
    let mut listen = None;
    let mut connect = Vec::new();
    let mut stay_connected = false;
    let mut api_listen = None;
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--network" => {
                network = match value()?.as_str() {
                    "devnet" => Network::Devnet,
                    "regtest" => Network::Regtest,
                    other => return Err(format!("unsupported network {other}")),
                };
            }
            "--datadir" => datadir = Some(PathBuf::from(value()?)),
            "--tx-file" => tx_files.push(PathBuf::from(value()?)),
            "--mine" => mine = true,
            "--beneficiary" => beneficiary = Some(parse_hex_key(&value()?)?),
            "--blocks" => {
                max_blocks = Some(value()?.parse().map_err(|_| "--blocks needs a number")?);
            }
            "--listen" => listen = Some(value()?),
            "--connect" => connect.push(value()?),
            "--stay-connected" => stay_connected = true,
            "--api-listen" => api_listen = Some(value()?),
            "-h" | "--help" => return Err(USAGE.into()),
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    let datadir = if let Some(d) = datadir {
        d
    } else {
        let home = std::env::var_os("HOME").ok_or("HOME is not set; use --datadir")?;
        PathBuf::from(home).join(".yuanlai").join(network.name())
    };
    let relay_beneficiary = if listen.is_some() && !mine {
        beneficiary
    } else {
        None
    };
    let mine = match (mine, beneficiary) {
        (true, Some(key)) => Some(key),
        (true, None) => return Err("--mine requires --beneficiary".into()),
        (false, _) => None,
    };
    if !tx_files.is_empty() && mine.is_none() && connect.is_empty() && !stay_connected {
        return Err("--tx-file requires --mine or --connect".into());
    }
    if listen.is_some() && !connect.is_empty() {
        return Err("choose only one of --listen or --connect".into());
    }
    if stay_connected && listen.is_some() {
        return Err("--stay-connected cannot be combined with --listen".into());
    }
    if (listen.is_some() || !connect.is_empty() || stay_connected) && mine.is_some() {
        return Err("network mode cannot be combined with --mine".into());
    }
    if listen.is_some() && !tx_files.is_empty() {
        return Err("--tx-file cannot be combined with --listen".into());
    }
    Ok(Options {
        network,
        datadir,
        mine,
        max_blocks,
        tx_files,
        listen,
        connect,
        stay_connected,
        api_listen,
        relay_beneficiary,
    })
}

fn now() -> Timestamp {
    Timestamp(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    )
}

fn load_chain(network: Network, stored: &[Block]) -> Result<Chain, String> {
    let genesis = genesis_block(network).ok_or("genesis block not defined for this network")?;
    let mut chain = Chain::new(network.params(), &genesis).map_err(|e| format!("genesis: {e}"))?;
    for block in stored {
        chain.connect(block).map_err(|e| {
            format!(
                "stored block at height {} is invalid: {e}",
                block.header.height
            )
        })?;
    }
    Ok(chain)
}

fn connection_nonce() -> Result<u64, String> {
    SysRng
        .try_next_u64()
        .map(|nonce| nonce.max(1))
        .map_err(|_| "operating system RNG is unavailable".to_owned())
}

struct DeadlineReader<'a> {
    stream: &'a mut TcpStream,
    deadline: Instant,
}

impl std::io::Read for DeadlineReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let remaining = self
            .deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "P2P handshake deadline exceeded",
                )
            })?;
        self.stream.set_read_timeout(Some(remaining))?;
        let bytes_read = std::io::Read::read(self.stream, buffer)?;
        if Instant::now() >= self.deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "P2P handshake deadline exceeded",
            ));
        }
        Ok(bytes_read)
    }
}

fn read_peer_message(stream: &mut TcpStream, timeout: Duration) -> Result<Message, String> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or("P2P message deadline is out of range")?;
    let previous_read_timeout = stream.read_timeout().map_err(|error| error.to_string())?;
    let result = {
        let mut reader = DeadlineReader { stream, deadline };
        read_message(&mut reader).map_err(|error| error.to_string())
    };
    stream
        .set_read_timeout(previous_read_timeout)
        .map_err(|error| error.to_string())?;
    result
}

fn peer_handshake(
    stream: &mut TcpStream,
    network: Network,
    chain: &Chain,
) -> Result<Handshake, String> {
    peer_handshake_with_timeout(stream, network, chain, PEER_HANDSHAKE_TIMEOUT)
}

fn peer_handshake_with_timeout(
    stream: &mut TcpStream,
    network: Network,
    chain: &Chain,
    timeout: Duration,
) -> Result<Handshake, String> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or("P2P handshake deadline is out of range")?;
    let previous_read_timeout = stream.read_timeout().map_err(|error| error.to_string())?;
    let (mut handshake, version) = Handshake::new(
        network.chain_id(),
        chain.height(),
        chain.tip().id(),
        connection_nonce()?,
    )
    .map_err(|error| error.to_string())?;
    write_message(stream, &version).map_err(|error| error.to_string())?;
    loop {
        let message = {
            let mut reader = DeadlineReader { stream, deadline };
            read_message(&mut reader).map_err(|error| error.to_string())?
        };
        match handshake
            .receive(message)
            .map_err(|error| error.to_string())?
        {
            HandshakeEvent::SendVerack => {
                write_message(stream, &Message::Verack).map_err(|error| error.to_string())?;
            }
            HandshakeEvent::Established => {
                stream
                    .set_read_timeout(previous_read_timeout)
                    .map_err(|error| error.to_string())?;
                return Ok(handshake);
            }
            HandshakeEvent::Message(_) => return Err("peer sent data during handshake".into()),
        }
    }
}

fn headers_after_locator(
    chain: &Chain,
    stored: &[Block],
    network: Network,
    locator: &BoundedVec<yuanlai_crypto::Hash, MAX_LOCATOR_HASHES>,
    stop: yuanlai_crypto::Hash,
) -> Result<BoundedVec<yuanlai_types::BlockHeader, MAX_HEADERS_PER_MESSAGE>, String> {
    let genesis = genesis_block(network).ok_or("genesis block not defined for this network")?;
    let genesis_id = genesis.header.id();
    let common_height = locator
        .iter()
        .find_map(|id| {
            if *id == genesis_id {
                return Some(0);
            }
            stored
                .iter()
                .position(|block| block.header.id() == *id)
                .and_then(|index| u64::try_from(index).ok())
                .and_then(|index| index.checked_add(1))
        })
        .unwrap_or(0);
    let mut headers = Vec::new();
    for offset in 1..=u64::from(MAX_HEADERS_PER_MESSAGE) {
        let Some(height) = common_height.checked_add(offset) else {
            break;
        };
        if height > chain.height().0 {
            break;
        }
        let header = chain
            .header_at(yuanlai_types::Height(height))
            .ok_or("active chain header index is inconsistent")?;
        headers.push(*header);
        if stop != yuanlai_crypto::Hash::ZERO && header.id() == stop {
            break;
        }
    }
    BoundedVec::new(headers).map_err(|error| error.to_string())
}

#[cfg(test)]
fn serve_peer(
    stream: TcpStream,
    network: Network,
    chain: &Chain,
    stored: &[Block],
) -> Result<(), String> {
    serve_peer_with_mempool(stream, network, chain, stored, None, None)
}

fn serve_peer_with_mempool(
    mut stream: TcpStream,
    network: Network,
    chain: &Chain,
    stored: &[Block],
    relay: Option<(&Arc<Mutex<Mempool>>, PubKey, &Path)>,
    peer_writers: Option<(&Arc<PeerWriters>, SocketAddr)>,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(PEER_HANDSHAKE_TIMEOUT))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| error.to_string())?;
    let mut handshake = peer_handshake(&mut stream, network, chain)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .map_err(|error| error.to_string())?;
    let (peer_writer, _registration) = if let Some((writers, address)) = peer_writers {
        let writer = Arc::new(Mutex::new(
            stream.try_clone().map_err(|error| error.to_string())?,
        ));
        writers.register(address, Arc::clone(&writer))?;
        (
            Some(writer),
            Some(PeerRegistration {
                address,
                writers: Arc::clone(writers),
            }),
        )
    } else {
        (None, None)
    };
    let genesis = genesis_block(network).ok_or("genesis block not defined for this network")?;
    loop {
        let message = read_peer_message(&mut stream, PEER_MESSAGE_TIMEOUT)?;
        let HandshakeEvent::Message(message) = handshake
            .receive(message)
            .map_err(|error| error.to_string())?
        else {
            return Err("peer repeated handshake message".into());
        };
        match message {
            Message::GetHeaders { locator, stop } => {
                let headers = headers_after_locator(chain, stored, network, &locator, stop)?;
                send_peer_message(
                    &mut stream,
                    peer_writer.as_ref(),
                    &Message::Headers(headers),
                )?;
            }
            Message::GetBlock(id) => {
                let block = if id == genesis.header.id() {
                    Some(genesis.clone())
                } else {
                    stored.iter().find(|block| block.header.id() == id).cloned()
                };
                let response = block.map_or(Message::NotFound(id), Message::Block);
                send_peer_message(&mut stream, peer_writer.as_ref(), &response)?;
            }
            Message::Ping(nonce) => {
                send_peer_message(&mut stream, peer_writer.as_ref(), &Message::Pong(nonce))?;
            }
            Message::Pong(_) => {}
            Message::Transaction(transaction) => {
                let transaction = *transaction;
                let txid = transaction.txid();
                let relay_message = Message::Transaction(Box::new(transaction.clone()));
                let (accepted, is_new) = if let Some((mempool, beneficiary, path)) = relay {
                    let mut mempool = mempool
                        .lock()
                        .map_err(|_| "mempool lock is poisoned".to_owned())?;
                    let original = mempool.clone();
                    match mempool.submit(chain, beneficiary, transaction) {
                        Ok(()) => match persist_mempool(path, &mempool) {
                            Ok(()) => (true, true),
                            Err(error) => {
                                *mempool = original;
                                eprintln!("could not persist accepted transaction: {error}");
                                (false, false)
                            }
                        },
                        Err(SubmitError::Duplicate) => (true, false),
                        Err(_) => (false, false),
                    }
                } else {
                    (false, false)
                };
                send_peer_message(
                    &mut stream,
                    peer_writer.as_ref(),
                    &Message::TransactionAck { txid, accepted },
                )?;
                if !accepted {
                    return Err(format!("peer transaction {txid} was rejected"));
                }
                if is_new && let Some((writers, source)) = peer_writers {
                    writers.broadcast(source, &relay_message)?;
                }
            }
            _ => return Err("unexpected P2P request".into()),
        }
    }
}

fn send_peer_message(
    stream: &mut TcpStream,
    writer: Option<&Arc<Mutex<TcpStream>>>,
    message: &Message,
) -> Result<(), String> {
    if let Some(writer) = writer {
        let mut writer = writer
            .lock()
            .map_err(|_| "peer writer lock is poisoned".to_owned())?;
        write_message(&mut *writer, message).map_err(|error| error.to_string())
    } else {
        write_message(stream, message).map_err(|error| error.to_string())
    }
}

fn listen_for_peers(
    address: &str,
    network: Network,
    chain: Chain,
    stored: Vec<Block>,
    relay_beneficiary: Option<PubKey>,
    datadir: &Path,
) -> Result<(), String> {
    let listener = TcpListener::bind(address).map_err(|error| format!("{address}: {error}"))?;
    let mempool_path = datadir.join("mempool.dat");
    let mempool = match relay_beneficiary {
        Some(beneficiary) => load_mempool(&mempool_path, &chain, beneficiary)?,
        None => Mempool::default(),
    };
    let chain = Arc::new(chain);
    let stored: Arc<[Block]> = Arc::from(stored);
    let active_peers = Arc::new(Mutex::new(PeerCounts::default()));
    let mempool = Arc::new(Mutex::new(mempool));
    let peer_writers = Arc::new(PeerWriters::default());
    println!(
        "listening on {}",
        listener.local_addr().map_err(|e| e.to_string())?
    );
    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                let address = match stream.peer_addr() {
                    Ok(address) => address,
                    Err(error) => {
                        eprintln!("cannot identify inbound peer: {error}");
                        continue;
                    }
                };
                let peer = address.to_string();
                let Some(slot) = PeerSlot::acquire(&active_peers, address.ip()) else {
                    eprintln!("peer {peer} rejected: inbound peer limit reached");
                    continue;
                };
                let chain = Arc::clone(&chain);
                let stored = Arc::clone(&stored);
                let mempool = Arc::clone(&mempool);
                let peer_writers = Arc::clone(&peer_writers);
                let mempool_path = mempool_path.clone();
                if let Err(error) = std::thread::Builder::new()
                    .name("yuanlai-peer".into())
                    .spawn(move || {
                        let _slot = slot;
                        let relay = relay_beneficiary
                            .map(|beneficiary| (&mempool, beneficiary, mempool_path.as_path()));
                        let writer_context = Some((&peer_writers, address));
                        if let Err(error) = serve_peer_with_mempool(
                            stream,
                            network,
                            &chain,
                            &stored,
                            relay,
                            writer_context,
                        ) {
                            eprintln!("peer {peer} disconnected: {error}");
                        }
                    })
                {
                    eprintln!("cannot start peer handler: {error}");
                }
            }
            Err(error) => eprintln!("accept failed: {error}"),
        }
    }
    Ok(())
}

fn active_locator(
    chain: &Chain,
) -> Result<BoundedVec<yuanlai_crypto::Hash, MAX_LOCATOR_HASHES>, String> {
    let mut ids = Vec::new();
    for offset in 0..u64::from(MAX_LOCATOR_HASHES) {
        let height = chain.height().0.saturating_sub(offset);
        let header = chain
            .header_at(yuanlai_types::Height(height))
            .ok_or("active chain header index is inconsistent")?;
        ids.push(header.id());
        if height == 0 {
            break;
        }
    }
    BoundedVec::new(ids).map_err(|error| error.to_string())
}

fn connect_peer(address: &str) -> Result<TcpStream, String> {
    let deadline = Instant::now()
        .checked_add(PEER_CONNECT_TIMEOUT)
        .ok_or("peer connection deadline is out of range")?;
    let addresses = address
        .to_socket_addrs()
        .map_err(|error| format!("{address}: {error}"))?;
    let mut last_error = None;
    for socket_address in addresses {
        let Some(remaining) = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
        else {
            return Err(format!("{address}: peer connection deadline exceeded"));
        };
        match TcpStream::connect_timeout(&socket_address, remaining) {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.map_or_else(
        || format!("{address}: no socket addresses resolved"),
        |error| format!("{address}: {error}"),
    ))
}

fn sync_from_peer(
    address: &str,
    network: Network,
    chain: &mut Chain,
    store: &mut BlockStore,
    stored: &mut Vec<Block>,
    transactions: &[Tx],
) -> Result<(), String> {
    let mut stream = connect_peer(address)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(60)))
        .map_err(|error| error.to_string())?;
    peer_handshake(&mut stream, network, chain)?;
    let genesis = genesis_block(network).ok_or("genesis block not defined for this network")?;
    let mut tree =
        HeaderTree::new(network.params(), genesis.header).map_err(|error| error.to_string())?;
    for block in stored.iter() {
        tree.insert(block.header)
            .map_err(|error| error.to_string())?;
    }
    let old_tip = chain.tip().id();
    let mut locator = active_locator(chain)?;
    loop {
        write_message(
            &mut stream,
            &Message::GetHeaders {
                locator: locator.clone(),
                stop: yuanlai_crypto::Hash::ZERO,
            },
        )
        .map_err(|error| error.to_string())?;
        let response = read_peer_message(&mut stream, PEER_MESSAGE_TIMEOUT)?;
        let Message::Headers(headers) = response else {
            return Err("peer did not answer GetHeaders with Headers".into());
        };
        let Some(last) = headers.as_slice().last().copied() else {
            break;
        };
        if locator
            .as_slice()
            .first()
            .is_some_and(|id| *id == last.id())
        {
            return Err("peer returned a non-advancing header page".into());
        }
        for header in &headers {
            tree.insert(*header).map_err(|error| error.to_string())?;
        }
        if headers.len() < MAX_HEADERS_PER_MESSAGE {
            break;
        }
        locator = BoundedVec::new(vec![last.id()]).map_err(|error| error.to_string())?;
    }
    let new_tip = tree.best_id();
    if new_tip == old_tip {
        println!("already synchronized at height {}", chain.height());
        return submit_transactions(&mut stream, transactions);
    }
    let fork = tree
        .common_ancestor(old_tip, new_tip)
        .ok_or("header tree lost the active chain ancestor")?;
    let fork_height = tree
        .header(fork)
        .ok_or("header tree lost the fork header")?
        .height;
    let branch_headers = tree
        .headers_after(fork, new_tip)
        .ok_or("best header is not descended from the fork point")?;
    let mut branch_blocks = Vec::with_capacity(branch_headers.len());
    for header in branch_headers {
        write_message(&mut stream, &Message::GetBlock(header.id()))
            .map_err(|error| error.to_string())?;
        let response = read_peer_message(&mut stream, PEER_MESSAGE_TIMEOUT)?;
        let Message::Block(block) = response else {
            return Err(format!("peer did not provide block {}", header.id()));
        };
        if block.header != header {
            return Err(format!("peer returned the wrong block for {}", header.id()));
        }
        branch_blocks.push(block);
    }

    let keep = usize::try_from(fork_height.0).map_err(|_| "fork height does not fit usize")?;
    let mut replacement = stored.iter().take(keep).cloned().collect::<Vec<_>>();
    replacement.extend(branch_blocks.iter().cloned());
    let old_chain = chain.clone();
    chain
        .reorganize(fork_height, &branch_blocks)
        .map_err(|error| error.to_string())?;
    if let Err(error) = store.replace_all(&replacement) {
        *chain = old_chain;
        return Err(error.to_string());
    }
    *stored = replacement;
    println!(
        "synchronized height {} id {}",
        chain.height(),
        chain.tip().id()
    );
    submit_transactions(&mut stream, transactions)
}

fn sync_from_peers(
    addresses: &[String],
    network: Network,
    chain: &mut Chain,
    store: &mut BlockStore,
    stored: &mut Vec<Block>,
    transactions: &[Tx],
) -> Result<(), String> {
    let mut synchronized = false;
    let mut failures = Vec::new();
    for address in addresses {
        match sync_from_peer(address, network, chain, store, stored, transactions) {
            Ok(()) => synchronized = true,
            Err(error) => {
                eprintln!("peer {address} failed: {error}");
                failures.push(address.as_str());
            }
        }
    }
    if synchronized {
        return Ok(());
    }
    Err(format!(
        "all configured peers failed: {}",
        failures.join(", ")
    ))
}

fn peer_retry_delay(consecutive_failures: u32) -> Duration {
    let shift = consecutive_failures.saturating_sub(1).min(5);
    Duration::from_secs(1_u64.checked_shl(shift).unwrap_or(30).min(30))
}

struct PeerSyncState {
    address: String,
    consecutive_failures: u32,
    next_attempt: Instant,
    last_success: Option<Instant>,
}

impl PeerSyncState {
    fn new(address: String, now: Instant) -> Self {
        Self {
            address,
            consecutive_failures: 0,
            next_attempt: now,
            last_success: None,
        }
    }

    fn is_due(&self, now: Instant) -> bool {
        now >= self.next_attempt
    }

    fn record_success(&mut self, now: Instant) {
        self.consecutive_failures = 0;
        self.last_success = Some(now);
        self.next_attempt = now.checked_add(PEER_RESYNC_INTERVAL).unwrap_or(now);
    }

    fn record_failure(&mut self, now: Instant) -> Duration {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        let delay = peer_retry_delay(self.consecutive_failures);
        self.next_attempt = now.checked_add(delay).unwrap_or(now);
        delay
    }
}

fn run_peer_sync_loop(
    addresses: &[String],
    network: Network,
    chain: &mut Chain,
    store: &mut BlockStore,
    stored: &mut Vec<Block>,
    transactions: &[Tx],
) -> ! {
    let mut peers = addresses
        .iter()
        .cloned()
        .map(|address| PeerSyncState::new(address, Instant::now()))
        .collect::<Vec<_>>();
    loop {
        for peer in &mut peers {
            let now = Instant::now();
            if !peer.is_due(now) {
                continue;
            }
            match sync_from_peer(&peer.address, network, chain, store, stored, transactions) {
                Ok(()) => peer.record_success(Instant::now()),
                Err(error) => {
                    let failed_at = Instant::now();
                    let last_success = peer.last_success.map_or_else(
                        || "never".to_owned(),
                        |last_success| {
                            format!(
                                "{:?} ago",
                                failed_at.saturating_duration_since(last_success)
                            )
                        },
                    );
                    let delay = peer.record_failure(failed_at);
                    eprintln!(
                        "peer {} sync failed; retrying in {delay:?} (last success: {last_success}): {error}",
                        peer.address
                    );
                }
            }
        }
        let now = Instant::now();
        let delay = peers
            .iter()
            .map(|peer| peer.next_attempt.saturating_duration_since(now))
            .min()
            .unwrap_or(PEER_RESYNC_INTERVAL);
        std::thread::sleep(delay);
    }
}

fn run_persistent_peer_sync(
    addresses: &[String],
    options: &Options,
    chain: &mut Chain,
    store: &mut BlockStore,
    stored: &mut Vec<Block>,
) -> Result<(), String> {
    let transactions = options
        .tx_files
        .iter()
        .map(read_transaction)
        .collect::<Result<Vec<_>, _>>()?;
    run_peer_sync_loop(
        addresses,
        options.network,
        chain,
        store,
        stored,
        &transactions,
    )
}

fn submit_transactions(stream: &mut TcpStream, transactions: &[Tx]) -> Result<(), String> {
    for transaction in transactions {
        let txid = transaction.txid();
        write_message(stream, &Message::Transaction(Box::new(transaction.clone())))
            .map_err(|error| error.to_string())?;
        match read_peer_message(stream, PEER_MESSAGE_TIMEOUT)? {
            Message::TransactionAck {
                txid: acknowledged,
                accepted: true,
            } if acknowledged == txid => println!("peer accepted transaction {txid}"),
            Message::TransactionAck {
                txid: acknowledged,
                accepted: false,
            } if acknowledged == txid => return Err(format!("peer rejected transaction {txid}")),
            _ => return Err("peer did not acknowledge the submitted transaction".into()),
        }
    }
    Ok(())
}

fn mine_one(chain: &Chain, beneficiary: PubKey, user_txs: &[Tx]) -> Option<Block> {
    let mut start = 0_u64;
    loop {
        let timestamp = now().max(chain.min_next_timestamp());
        let mut block = chain.template_with(
            beneficiary,
            BoundedBytes::empty(),
            timestamp,
            user_txs.to_vec(),
        )?;
        let end = start.saturating_add(NONCE_BATCH - 1);
        if let Some(header) = yuanlai_miner::solve(&block.header, start..=end) {
            block.header = header;
            return Some(block);
        }
        start = end.checked_add(1).unwrap_or(0);
    }
}

fn read_transaction(path: &PathBuf) -> Result<Tx, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut reader = Reader::new(&bytes);
    let transaction = Tx::decode_from(&mut reader)
        .map_err(|error| format!("{}: invalid transaction: {error}", path.display()))?;
    if !reader.is_empty() {
        return Err(format!(
            "{}: trailing bytes after transaction",
            path.display()
        ));
    }
    Ok(transaction)
}

fn persist_mempool(path: &Path, mempool: &Mempool) -> Result<(), String> {
    if mempool.is_empty() {
        return match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("{}: {error}", path.display())),
        };
    }
    let transactions = BoundedVec::<Tx, MAX_BLOCK_TXS>::new(mempool.transactions().to_vec())
        .map_err(|error| error.to_string())?;
    let bytes = transactions.encode();
    if bytes.len() > MAX_MEMPOOL_FILE_BYTES {
        return Err(format!(
            "{}: mempool file exceeds size limit",
            path.display()
        ));
    }
    let temporary_path = path.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temporary_path)
        .map_err(|error| format!("{}: {error}", temporary_path.display()))?;
    std::io::Write::write_all(&mut file, &bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    std::fs::rename(&temporary_path, path).map_err(|error| error.to_string())
}

fn load_mempool(path: &Path, chain: &Chain, beneficiary: PubKey) -> Result<Mempool, String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Mempool::default()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let max_bytes =
        u64::try_from(MAX_MEMPOOL_FILE_BYTES).map_err(|_| "mempool size limit does not fit u64")?;
    if file
        .metadata()
        .map_err(|error| format!("{}: {error}", path.display()))?
        .len()
        > max_bytes
    {
        return Err(format!(
            "{}: mempool file exceeds size limit",
            path.display()
        ));
    }
    let mut reader = std::io::Read::take(file, max_bytes.saturating_add(1));
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut bytes)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() > MAX_MEMPOOL_FILE_BYTES {
        return Err(format!(
            "{}: mempool file exceeds size limit",
            path.display()
        ));
    }
    let transactions: BoundedVec<Tx, MAX_BLOCK_TXS> = decode_exact(&bytes)
        .map_err(|error| format!("{}: invalid mempool file: {error}", path.display()))?;
    let mut mempool = Mempool::default();
    for transaction in &transactions {
        match mempool.submit(chain, beneficiary, transaction.clone()) {
            Ok(()) | Err(SubmitError::Duplicate) => {}
            Err(error) => eprintln!(
                "discarding stale mempool transaction {}: {error}",
                transaction.txid()
            ),
        }
    }
    persist_mempool(path, &mempool)?;
    Ok(mempool)
}

fn normalize_peer_addresses(
    addresses: impl IntoIterator<Item = String>,
) -> Result<Vec<String>, String> {
    let mut normalized = Vec::new();
    for address in addresses {
        if address.is_empty()
            || address.len() > MAX_PEER_ADDRESS_BYTES
            || address.chars().any(char::is_whitespace)
            || address.chars().any(char::is_control)
        {
            return Err(
                "peer addresses must be non-empty, whitespace-free, and at most 512 bytes".into(),
            );
        }
        if !normalized.contains(&address) {
            if normalized.len() >= MAX_STATIC_PEERS {
                return Err(format!("peer list exceeds {MAX_STATIC_PEERS} entries"));
            }
            normalized.push(address);
        }
    }
    Ok(normalized)
}

fn load_peer_addresses(path: &Path) -> Result<Vec<String>, String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let max_bytes = u64::try_from(MAX_PEER_CONFIG_BYTES)
        .map_err(|_| "peer config size limit does not fit u64")?;
    if file
        .metadata()
        .map_err(|error| format!("{}: {error}", path.display()))?
        .len()
        > max_bytes
    {
        return Err(format!(
            "{}: peer config exceeds size limit",
            path.display()
        ));
    }
    let mut reader = std::io::Read::take(file, max_bytes.saturating_add(1));
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut bytes)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() > MAX_PEER_CONFIG_BYTES {
        return Err(format!(
            "{}: peer config exceeds size limit",
            path.display()
        ));
    }
    let contents = std::str::from_utf8(&bytes)
        .map_err(|error| format!("{}: peer config is not UTF-8: {error}", path.display()))?;
    normalize_peer_addresses(contents.lines().map(str::to_owned))
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn persist_peer_addresses(path: &Path, addresses: &[String]) -> Result<(), String> {
    let mut contents = String::new();
    for address in addresses {
        contents.push_str(address);
        contents.push('\n');
    }
    if contents.len() > MAX_PEER_CONFIG_BYTES {
        return Err(format!(
            "{}: peer config exceeds size limit",
            path.display()
        ));
    }
    let temporary_path = path.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temporary_path)
        .map_err(|error| format!("{}: {error}", temporary_path.display()))?;
    std::io::Write::write_all(&mut file, contents.as_bytes()).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    std::fs::rename(&temporary_path, path).map_err(|error| error.to_string())
}

fn resolve_peer_addresses(datadir: &Path, cli_addresses: &[String]) -> Result<Vec<String>, String> {
    let path = datadir.join("peers.txt");
    let mut addresses = load_peer_addresses(&path)?;
    addresses.extend_from_slice(cli_addresses);
    let addresses = normalize_peer_addresses(addresses)?;
    if addresses.is_empty() {
        return Err(format!(
            "no peers configured; use --connect or add addresses to {}",
            path.display()
        ));
    }
    persist_peer_addresses(&path, &addresses)?;
    Ok(addresses)
}

fn start_api(options: &Options) -> Result<Option<std::thread::JoinHandle<()>>, String> {
    let Some(address) = &options.api_listen else {
        return Ok(None);
    };
    let listener = TcpListener::bind(address)
        .map_err(|error| format!("cannot bind API listener at {address}: {error}"))?;
    if options.mine.is_none()
        && options.listen.is_none()
        && options.connect.is_empty()
        && !options.stay_connected
    {
        yuanlai_rpc::serve_listener(&listener, options.network, &options.datadir)
            .map_err(|error| format!("API server failed: {error}"))?;
        return Ok(None);
    }
    let network = options.network;
    let datadir = options.datadir.clone();
    std::thread::Builder::new()
        .name("yuanlai-api".into())
        .spawn(move || {
            if let Err(error) = yuanlai_rpc::serve_listener(&listener, network, &datadir) {
                eprintln!("API server failed: {error}");
            }
        })
        .map(Some)
        .map_err(|error| format!("cannot start API server: {error}"))
}

fn run(options: &Options) -> Result<(), String> {
    std::fs::create_dir_all(&options.datadir)
        .map_err(|e| format!("{}: {e}", options.datadir.display()))?;
    let opened =
        BlockStore::open(&options.datadir.join("blocks.dat")).map_err(|e| e.to_string())?;
    if opened.truncated_bytes > 0 {
        eprintln!(
            "recovered from an interrupted write: truncated {} bytes",
            opened.truncated_bytes
        );
    }
    let mut store = opened.store;
    let mut stored_blocks = opened.blocks;
    let mut chain = load_chain(options.network, &stored_blocks)?;
    let persistent_peers = if options.stay_connected {
        Some(resolve_peer_addresses(&options.datadir, &options.connect)?)
    } else {
        None
    };
    println!(
        "{} tip height {} id {}",
        options.network.name(),
        chain.height(),
        chain.tip().id()
    );

    let _api_thread = start_api(options)?;

    if options.stay_connected {
        let addresses = persistent_peers
            .as_deref()
            .ok_or("persistent peer list was not initialized")?;
        return run_persistent_peer_sync(
            addresses,
            options,
            &mut chain,
            &mut store,
            &mut stored_blocks,
        );
    }
    if !options.connect.is_empty() {
        let transactions = options
            .tx_files
            .iter()
            .map(read_transaction)
            .collect::<Result<Vec<_>, _>>()?;
        return sync_from_peers(
            &options.connect,
            options.network,
            &mut chain,
            &mut store,
            &mut stored_blocks,
            &transactions,
        );
    }
    if let Some(address) = &options.listen {
        return listen_for_peers(
            address,
            options.network,
            chain,
            stored_blocks,
            options.relay_beneficiary,
            &options.datadir,
        );
    }

    let Some(beneficiary) = options.mine else {
        return Ok(());
    };
    let mempool_path = options.datadir.join("mempool.dat");
    let mut mempool = load_mempool(&mempool_path, &chain, beneficiary)?;
    for path in &options.tx_files {
        let transaction = read_transaction(path)?;
        mempool
            .submit(&chain, beneficiary, transaction)
            .map_err(|error| format!("{}: {error}", path.display()))?;
    }
    if !mempool.is_empty() {
        println!("accepted {} transaction(s)", mempool.len());
    }
    persist_mempool(&mempool_path, &mempool)?;
    let mut mined = 0_u64;
    while options.max_blocks.is_none_or(|max| mined < max) {
        let block = mine_one(&chain, beneficiary, mempool.transactions())
            .ok_or("cannot extend the chain")?;
        chain
            .connect(&block)
            .map_err(|e| format!("mined an invalid block: {e}"))?;
        store.append(&block).map_err(|e| e.to_string())?;
        mempool.clear();
        persist_mempool(&mempool_path, &mempool)?;
        mined = mined.saturating_add(1);

        let height = block.header.height;
        let expired = chain.delta_at(height).map_or(0, |d| d.expired.len());
        println!(
            "mined height {height} id {} balance {} expired {expired}",
            block.header.id(),
            chain.state().balance(&beneficiary),
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    let options = match parse_args(std::env::args().skip(1)) {
        Ok(o) => o,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    match run(&options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, TcpListener, TcpStream};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use yuanlai_codec::{BoundedBytes, BoundedVec, Encode};
    use yuanlai_consensus::Chain;
    use yuanlai_consensus::genesis::genesis_block;
    use yuanlai_crypto::{Hash, public_key, sign};
    use yuanlai_mempool::Mempool;
    use yuanlai_storage::BlockStore;
    use yuanlai_types::{
        Coinbase, Credit, Height, Network, PubKey, RegisterIdentity, Signature, Tx, UserTx,
    };

    use super::{
        MAX_INBOUND_PEERS, MAX_INBOUND_PEERS_PER_IP, MAX_MEMPOOL_FILE_BYTES, MAX_PEER_CONFIG_BYTES,
        MAX_STATIC_PEERS, PeerCounts, PeerSlot, PeerSyncState, PeerWriters, connect_peer,
        load_chain, load_mempool, mine_one, parse_args, peer_handshake,
        peer_handshake_with_timeout, peer_retry_delay, read_peer_message, read_transaction,
        resolve_peer_addresses, serve_peer, serve_peer_with_mempool, submit_transactions,
        sync_from_peers,
    };

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn outbound_peer_connect_uses_resolved_socket_addresses()
    -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let stream = connect_peer(&address.to_string()).map_err(std::io::Error::other)?;
        let (accepted, _) = listener.accept()?;
        assert_eq!(stream.peer_addr()?, accepted.local_addr()?);

        let refused_listener = TcpListener::bind("127.0.0.1:0")?;
        let refused_address = refused_listener.local_addr()?;
        drop(refused_listener);
        let error = connect_peer(&refused_address.to_string())
            .err()
            .ok_or("connection to a closed peer unexpectedly succeeded")?;
        assert!(error.contains(&refused_address.to_string()));
        Ok(())
    }

    #[test]
    fn inbound_handshake_deadline_stops_slow_byte_drips() -> Result<(), Box<dyn std::error::Error>>
    {
        let network = Network::Regtest;
        let genesis = genesis_block(network).ok_or("missing regtest genesis")?;
        let chain = Chain::new(network.params(), &genesis)?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let server_chain = chain.clone();
        let server = thread::spawn(move || -> Result<(), String> {
            let (mut stream, _) = listener.accept().map_err(|error| error.to_string())?;
            peer_handshake_with_timeout(
                &mut stream,
                network,
                &server_chain,
                Duration::from_millis(300),
            )
            .map(|_| ())
        });
        let mut client = TcpStream::connect(address)?;
        let drip_sender = thread::spawn(move || {
            for byte in *b"YLP2\x01\x00\x00\x00" {
                thread::sleep(Duration::from_millis(80));
                if std::io::Write::write_all(&mut client, &[byte]).is_err() {
                    break;
                }
            }
        });
        let peer_result = server
            .join()
            .map_err(|_| std::io::Error::other("slow handshake server panicked"))?;
        assert!(peer_result.is_err());
        drip_sender
            .join()
            .map_err(|_| std::io::Error::other("slow handshake client panicked"))?;
        Ok(())
    }

    #[test]
    fn peer_message_deadline_stops_slow_frame_drips() -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let server = thread::spawn(move || -> Result<(), String> {
            let (mut stream, _) = listener.accept().map_err(|error| error.to_string())?;
            read_peer_message(&mut stream, Duration::from_millis(300)).map(|_| ())
        });
        let mut client = TcpStream::connect(address)?;
        let drip_sender = thread::spawn(move || {
            for byte in *b"YLP2\x01\x00\x00\x00" {
                thread::sleep(Duration::from_millis(80));
                if std::io::Write::write_all(&mut client, &[byte]).is_err() {
                    break;
                }
            }
        });
        let result = server
            .join()
            .map_err(|_| std::io::Error::other("slow frame server panicked"))?;
        assert!(result.is_err());
        drip_sender
            .join()
            .map_err(|_| std::io::Error::other("slow frame client panicked"))?;
        Ok(())
    }

    #[test]
    fn inbound_peer_slots_are_bounded_and_released() -> Result<(), Box<dyn std::error::Error>> {
        let active = Arc::new(Mutex::new(PeerCounts::default()));
        let mut slots = Vec::new();
        let shared_ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        for _ in 0..MAX_INBOUND_PEERS_PER_IP {
            slots.push(
                PeerSlot::acquire(&active, shared_ip)
                    .ok_or("peer slot unexpectedly unavailable")?,
            );
        }
        let mapped_ip = IpAddr::V6(Ipv6Addr::from([0, 0, 0, 0, 0, 0xffff, 0x7f00, 1]));
        assert!(PeerSlot::acquire(&active, mapped_ip).is_none());
        for index in 1..=(MAX_INBOUND_PEERS - MAX_INBOUND_PEERS_PER_IP) {
            let last_octet = u8::try_from(index)?;
            let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, last_octet));
            slots.push(PeerSlot::acquire(&active, ip).ok_or("global peer slot was unavailable")?);
        }
        assert_eq!(slots.len(), MAX_INBOUND_PEERS);
        let other_ip = IpAddr::V4(Ipv4Addr::new(10, 0, 1, 1));
        assert!(PeerSlot::acquire(&active, other_ip).is_none());

        drop(slots.pop());
        let replacement =
            PeerSlot::acquire(&active, other_ip).ok_or("released global slot was not reusable")?;
        drop(replacement);
        drop(slots);
        let replacement =
            PeerSlot::acquire(&active, shared_ip).ok_or("released per-IP slot was not reusable")?;
        drop(replacement);
        Ok(())
    }

    #[test]
    fn relay_listener_and_one_shot_transaction_submit_options_parse()
    -> Result<(), Box<dyn std::error::Error>> {
        let beneficiary = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
        let listener = parse_args(
            [
                "--network",
                "regtest",
                "--datadir",
                "./node-a",
                "--listen",
                "127.0.0.1:18444",
                "--beneficiary",
                beneficiary,
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .map_err(std::io::Error::other)?;
        assert!(listener.relay_beneficiary.is_some());

        let sender = parse_args(
            [
                "--network",
                "regtest",
                "--datadir",
                "./node-b",
                "--connect",
                "127.0.0.1:18444",
                "--connect",
                "127.0.0.1:18445",
                "--stay-connected",
                "--tx-file",
                "./signed-tx.bin",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .map_err(std::io::Error::other)?;
        assert_eq!(sender.connect.len(), 2);
        assert!(sender.stay_connected);
        assert_eq!(sender.tx_files.len(), 1);
        let stored_peer_mode = parse_args(
            ["--datadir", "./node-c", "--stay-connected"]
                .into_iter()
                .map(str::to_owned),
        )
        .map_err(std::io::Error::other)?;
        assert!(stored_peer_mode.stay_connected);
        assert!(stored_peer_mode.connect.is_empty());
        Ok(())
    }

    #[test]
    fn persistent_peer_config_merges_deduplicates_and_reloads()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = temp_dir()?;
        let config_path = directory.join("peers.txt");
        let seeded = ["127.0.0.1:18444".to_owned(), "127.0.0.1:18444".to_owned()];
        let configured = resolve_peer_addresses(&directory, &seeded)?;
        assert_eq!(configured, ["127.0.0.1:18444"]);
        assert_eq!(fs::read_to_string(&config_path)?, "127.0.0.1:18444\n");
        assert_eq!(resolve_peer_addresses(&directory, &[])?, configured);

        fs::write(&config_path, "invalid peer address\n")?;
        let error = resolve_peer_addresses(&directory, &[])
            .err()
            .ok_or("whitespace-containing peer address was accepted")?;
        assert!(error.contains("whitespace-free"));

        fs::write(&config_path, vec![b'x'; MAX_PEER_CONFIG_BYTES + 1])?;
        let error = resolve_peer_addresses(&directory, &[])
            .err()
            .ok_or("oversized peer config was accepted")?;
        assert!(error.contains("exceeds size limit"));

        fs::remove_file(&config_path)?;
        let too_many = (0..=MAX_STATIC_PEERS)
            .map(|index| format!("peer-{index}:18444"))
            .collect::<Vec<_>>();
        let error = resolve_peer_addresses(&directory, &too_many)
            .err()
            .ok_or("peer count limit was not enforced")?;
        assert!(error.contains("exceeds 64 entries"));
        fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn peer_retry_delay_grows_exponentially_and_is_capped() {
        assert_eq!(peer_retry_delay(1), Duration::from_secs(1));
        assert_eq!(peer_retry_delay(2), Duration::from_secs(2));
        assert_eq!(peer_retry_delay(3), Duration::from_secs(4));
        assert_eq!(peer_retry_delay(6), Duration::from_secs(30));
        assert_eq!(peer_retry_delay(u32::MAX), Duration::from_secs(30));
    }

    #[test]
    fn peer_retry_state_is_independent_per_address() -> Result<(), Box<dyn std::error::Error>> {
        let now = Instant::now();
        let after_one_second = now
            .checked_add(Duration::from_secs(1))
            .ok_or("test instant overflow")?;
        let mut offline = PeerSyncState::new("offline:1".to_owned(), now);
        let mut healthy = PeerSyncState::new("healthy:1".to_owned(), now);

        assert_eq!(offline.record_failure(now), Duration::from_secs(1));
        healthy.record_success(now);
        assert!(offline.is_due(after_one_second));
        assert!(!healthy.is_due(after_one_second));
        assert_eq!(offline.consecutive_failures, 1);
        assert_eq!(healthy.consecutive_failures, 0);
        assert_eq!(healthy.last_success, Some(now));
        Ok(())
    }

    #[test]
    fn peer_broadcast_excludes_the_source_connection() -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let mut source = TcpStream::connect(address)?;
        let (source_writer, _) = listener.accept()?;
        let mut destination = TcpStream::connect(address)?;
        let (destination_writer, _) = listener.accept()?;
        let source_address = source.local_addr()?;
        let destination_address = destination.local_addr()?;
        let peers = PeerWriters::default();
        peers.register(
            source_address,
            Arc::new(Mutex::new(source_writer.try_clone()?)),
        )?;
        peers.register(
            destination_address,
            Arc::new(Mutex::new(destination_writer)),
        )?;

        peers.broadcast(source_address, &yuanlai_p2p::Message::Ping(17))?;
        destination.set_read_timeout(Some(Duration::from_millis(100)))?;
        assert_eq!(
            yuanlai_p2p::read_message(&mut destination)?,
            yuanlai_p2p::Message::Ping(17)
        );
        source.set_read_timeout(Some(Duration::from_millis(20)))?;
        assert!(yuanlai_p2p::read_message(&mut source).is_err());
        Ok(())
    }

    #[test]
    fn transaction_submit_checks_acknowledged_txid() -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let transaction = Tx::Coinbase(Coinbase {
            height: Height(1),
            beneficiary: PubKey([7; 32]),
            extra: BoundedBytes::empty(),
        });
        let expected_id = transaction.txid();
        let server = thread::spawn(move || -> Result<(), String> {
            let (mut stream, _) = listener.accept().map_err(|error| error.to_string())?;
            let message =
                yuanlai_p2p::read_message(&mut stream).map_err(|error| error.to_string())?;
            let yuanlai_p2p::Message::Transaction(_) = message else {
                return Err("client did not send a transaction".into());
            };
            yuanlai_p2p::write_message(
                &mut stream,
                &yuanlai_p2p::Message::TransactionAck {
                    txid: expected_id,
                    accepted: true,
                },
            )
            .map_err(|error| error.to_string())
        });
        let mut client = TcpStream::connect(address)?;
        submit_transactions(&mut client, &[transaction])?;
        server
            .join()
            .map_err(|_| std::io::Error::other("ack test server panicked"))?
            .map_err(std::io::Error::other)?;
        Ok(())
    }

    #[test]
    fn peer_admits_and_relays_a_valid_signed_transaction() -> Result<(), Box<dyn std::error::Error>>
    {
        let network = Network::Regtest;
        let genesis = genesis_block(network).ok_or("missing regtest genesis")?;
        let mut chain = Chain::new(network.params(), &genesis)?;
        let secret = [
            0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec,
            0x2c, 0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03,
            0x1c, 0xae, 0x7f, 0x60,
        ];
        let beneficiary = PubKey(public_key(&secret));
        let mut stored = Vec::new();
        for _ in 0..12 {
            let block = mine_one(&chain, beneficiary, &[]).ok_or("could not mine test block")?;
            chain.connect(&block)?;
            stored.push(block);
        }
        let height = Height(chain.height().0.saturating_add(1));
        let inputs = chain
            .state()
            .select_inputs(&beneficiary, Credit(20), height, |_| true)
            .ok_or("mature credits are missing")?;
        let mut transaction = Tx::RegisterIdentity(UserTx {
            sender: beneficiary,
            nonce: chain.state().nonce(&beneficiary),
            body: RegisterIdentity {
                inputs: BoundedVec::new(inputs)?,
                commitment: Hash([1; 32]),
            },
            signature: Signature::EMPTY,
        });
        let message = transaction
            .sig_msg(&network.chain_id())
            .ok_or("transaction has no signing message")?;
        transaction.set_signature(Signature(sign(&secret, message.as_bytes())));

        let mempool = Arc::new(Mutex::new(Mempool::default()));
        let mempool_directory = temp_dir()?;
        let mempool_path = mempool_directory.join("mempool.dat");
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let mut client = TcpStream::connect(address)?;
        let source_address = client.local_addr()?;
        let destination_listener = TcpListener::bind("127.0.0.1:0")?;
        let mut destination = TcpStream::connect(destination_listener.local_addr()?)?;
        let destination_address = destination.local_addr()?;
        let (destination_writer, _) = destination_listener.accept()?;
        destination.set_read_timeout(Some(Duration::from_millis(200)))?;
        let peer_writers = Arc::new(PeerWriters::default());
        peer_writers.register(
            destination_address,
            Arc::new(Mutex::new(destination_writer)),
        )?;
        let server_chain = chain.clone();
        let server_mempool = Arc::clone(&mempool);
        let server_mempool_path = mempool_path.clone();
        let server_peer_writers = Arc::clone(&peer_writers);
        let server = thread::spawn(move || -> Result<(), String> {
            let (stream, _) = listener.accept().map_err(|error| error.to_string())?;
            serve_peer_with_mempool(
                stream,
                network,
                &server_chain,
                &stored,
                Some((&server_mempool, beneficiary, &server_mempool_path)),
                Some((&server_peer_writers, source_address)),
            )
        });

        assert_transaction_ack_and_relay(
            &mut client,
            &mut destination,
            network,
            &chain,
            &transaction,
        )?;
        let pending_count = mempool
            .lock()
            .map_err(|_| std::io::Error::other("mempool lock is poisoned"))?
            .len();
        assert_eq!(pending_count, 1);
        let restored = load_mempool(&mempool_path, &chain, beneficiary)?;
        assert_eq!(restored.len(), 1);
        let included = mine_one(&chain, beneficiary, &[transaction])
            .ok_or("could not mine pending test transaction")?;
        chain.connect(&included)?;
        let restored_after_inclusion = load_mempool(&mempool_path, &chain, beneficiary)?;
        assert!(restored_after_inclusion.is_empty());
        assert!(!mempool_path.exists());
        drop(client);
        let _server_result = server
            .join()
            .map_err(|_| std::io::Error::other("test peer server panicked"))?;
        fs::remove_dir_all(mempool_directory)?;
        Ok(())
    }

    fn temp_dir() -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
        let id = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory =
            std::env::temp_dir().join(format!("yuanlai-node-test-{}-{id}", std::process::id()));
        fs::create_dir(&directory)?;
        Ok(directory)
    }

    fn assert_transaction_ack_and_relay(
        client: &mut TcpStream,
        destination: &mut TcpStream,
        network: Network,
        chain: &Chain,
        transaction: &Tx,
    ) -> Result<(), Box<dyn std::error::Error>> {
        peer_handshake(client, network, chain)?;
        let txid = transaction.txid();
        yuanlai_p2p::write_message(
            client,
            &yuanlai_p2p::Message::Transaction(Box::new(transaction.clone())),
        )?;
        assert_eq!(
            yuanlai_p2p::read_message(client)?,
            yuanlai_p2p::Message::TransactionAck {
                txid,
                accepted: true,
            }
        );
        let relayed = yuanlai_p2p::read_message(destination)?;
        let yuanlai_p2p::Message::Transaction(relayed) = relayed else {
            return Err("transaction was not relayed to the other peer".into());
        };
        assert_eq!(relayed.txid(), txid);
        Ok(())
    }

    #[test]
    fn mempool_loader_rejects_malformed_and_oversized_files()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = temp_dir()?;
        let path = directory.join("mempool.dat");
        let network = Network::Regtest;
        let genesis = genesis_block(network).ok_or("missing regtest genesis")?;
        let chain = Chain::new(network.params(), &genesis)?;
        let beneficiary = PubKey([7; 32]);

        fs::write(&path, [0xff])?;
        assert!(load_mempool(&path, &chain, beneficiary).is_err());

        fs::write(&path, vec![0; MAX_MEMPOOL_FILE_BYTES + 1])?;
        let error = load_mempool(&path, &chain, beneficiary)
            .err()
            .ok_or("oversized mempool file was accepted")?;
        assert!(error.contains("exceeds size limit"));

        fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn transaction_file_must_contain_exactly_one_transaction()
    -> Result<(), Box<dyn std::error::Error>> {
        let id = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory =
            std::env::temp_dir().join(format!("yuanlai-node-tx-{}-{id}", std::process::id()));
        fs::create_dir(&directory)?;
        let path = directory.join("transaction.bin");
        let transaction = Tx::Coinbase(Coinbase {
            height: Height(1),
            beneficiary: PubKey([0; 32]),
            extra: BoundedBytes::empty(),
        });
        fs::write(&path, transaction.encode())?;
        assert_eq!(read_transaction(&path)?, transaction);

        let mut with_trailing_byte = transaction.encode();
        with_trailing_byte.push(0);
        fs::write(&path, with_trailing_byte)?;
        let error = read_transaction(&path)
            .err()
            .ok_or("trailing byte was accepted")?;
        assert!(error.contains("trailing bytes"));

        fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn node_reorganizes_over_tcp_and_rewrites_the_active_store()
    -> Result<(), Box<dyn std::error::Error>> {
        let network = Network::Regtest;
        let genesis = genesis_block(network).ok_or("missing regtest genesis")?;
        let mut remote_chain = Chain::new(network.params(), &genesis)?;
        let remote_owner = PubKey(public_key(&[0x33; 32]));
        let mut remote_blocks = Vec::new();
        for _ in 0..3 {
            let block = mine_one(&remote_chain, remote_owner, &[])
                .ok_or("could not mine remote test block")?;
            remote_chain.connect(&block)?;
            remote_blocks.push(block);
        }

        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let second_listener = TcpListener::bind("127.0.0.1:0")?;
        let second_address = second_listener.local_addr()?;
        let remote_tip = remote_chain.tip().id();
        let server_blocks = remote_blocks.clone();
        let server_chain = remote_chain.clone();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let _ = serve_peer(stream, network, &server_chain, &server_blocks);
        });
        let second_server_chain = remote_chain.clone();
        let second_server_blocks = remote_blocks.clone();
        let second_server = thread::spawn(move || {
            let (stream, _) = second_listener.accept().unwrap();
            let _ = serve_peer(stream, network, &second_server_chain, &second_server_blocks);
        });
        let unavailable_listener = TcpListener::bind("127.0.0.1:0")?;
        let unavailable_address = unavailable_listener.local_addr()?;
        drop(unavailable_listener);

        let directory = temp_dir()?;
        let mut opened = BlockStore::open(&directory.join("blocks.dat"))?;
        let local_owner = PubKey(public_key(&[0x44; 32]));
        let mut local_chain = Chain::new(network.params(), &genesis)?;
        let mut local_blocks = Vec::new();
        for _ in 0..2 {
            let block = mine_one(&local_chain, local_owner, &[])
                .ok_or("could not mine local test block")?;
            local_chain.connect(&block)?;
            opened.store.append(&block)?;
            local_blocks.push(block);
        }
        local_chain = load_chain(network, &local_blocks).map_err(std::io::Error::other)?;
        let addresses = [
            unavailable_address.to_string(),
            address.to_string(),
            second_address.to_string(),
        ];
        sync_from_peers(
            &addresses,
            network,
            &mut local_chain,
            &mut opened.store,
            &mut local_blocks,
            &[],
        )
        .map_err(std::io::Error::other)?;
        server.join().map_err(|_| "peer server thread panicked")?;
        second_server
            .join()
            .map_err(|_| "second peer server thread panicked")?;

        assert_eq!(local_chain.tip().id(), remote_tip);
        assert_eq!(local_blocks, remote_blocks);
        assert_eq!(local_chain.state().balance(&local_owner).0, 0);
        assert_eq!(local_chain.state().balance(&remote_owner).0, 30);
        drop(opened);
        assert_eq!(
            BlockStore::open(&directory.join("blocks.dat"))?.blocks,
            remote_blocks
        );
        fs::remove_dir_all(directory)?;
        Ok(())
    }
}
