//! Bounded peer-to-peer messages and framing for YuanLai nodes.
//!
//! Normative definition: `spec/07-p2p.md`.

use core::fmt;
use std::io::{Read, Write};

use yuanlai_codec::{BoundedVec, Decode, DecodeError, Encode, Reader, decode_exact};
use yuanlai_types::params::MAX_BLOCK_SIZE;
use yuanlai_types::{Block, BlockHeader, Hash, Height, Tx};

/// Maximum hashes in a `GetHeaders` locator.
pub const MAX_LOCATOR_HASHES: u32 = 32;
/// Maximum headers in one `Headers` response.
pub const MAX_HEADERS_PER_MESSAGE: u32 = 2_000;
/// Maximum payload bytes in one frame, including its message tag.
pub const MAX_FRAME_PAYLOAD: usize = MAX_BLOCK_SIZE + 1;
/// Current P2P protocol version.
pub const PROTOCOL_VERSION: u32 = 1;

const FRAME_MAGIC: [u8; 4] = *b"YLP2";
const FRAME_HEADER_SIZE: usize = 8;

/// A P2P message (`spec/07-p2p.md` §2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// Initial peer capabilities and active-tip announcement.
    Version {
        /// P2P protocol version.
        protocol_version: u32,
        /// Must equal the local network's chain ID.
        chain_id: Hash,
        /// Active-chain height announced by the peer.
        best_height: Height,
        /// Active-chain tip ID announced by the peer.
        best_id: Hash,
        /// Per-connection nonce used to reject self-connections.
        nonce: u64,
    },
    /// Completes the version handshake.
    Verack,
    /// Requests headers after a locator, optionally stopping at a known ID.
    GetHeaders {
        /// Recent active-chain IDs, newest first. Empty means start at genesis.
        locator: BoundedVec<Hash, MAX_LOCATOR_HASHES>,
        /// Stop after this ID, or use zero to request up to the peer's tip.
        stop: Hash,
    },
    /// Headers following the first common locator entry.
    Headers(BoundedVec<BlockHeader, MAX_HEADERS_PER_MESSAGE>),
    /// Requests one block body by its header ID.
    GetBlock(Hash),
    /// A complete block body.
    Block(Block),
    /// The requested block is not available from this peer.
    NotFound(Hash),
    /// Liveness probe.
    Ping(u64),
    /// Response to a liveness probe with the same nonce.
    Pong(u64),
    /// A signed user transaction offered for mempool admission and gossip.
    Transaction(Box<Tx>),
    /// Admission result for a transaction, correlated by transaction ID.
    TransactionAck {
        /// ID of the submitted transaction.
        txid: Hash,
        /// Whether the transaction was admitted or was already pending.
        accepted: bool,
    },
}

impl Encode for Message {
    fn encode_to(&self, out: &mut Vec<u8>) {
        match self {
            Self::Version {
                protocol_version,
                chain_id,
                best_height,
                best_id,
                nonce,
            } => {
                0_u8.encode_to(out);
                protocol_version.encode_to(out);
                chain_id.encode_to(out);
                best_height.encode_to(out);
                best_id.encode_to(out);
                nonce.encode_to(out);
            }
            Self::Verack => 1_u8.encode_to(out),
            Self::GetHeaders { locator, stop } => {
                2_u8.encode_to(out);
                locator.encode_to(out);
                stop.encode_to(out);
            }
            Self::Headers(headers) => {
                3_u8.encode_to(out);
                headers.encode_to(out);
            }
            Self::GetBlock(id) => {
                4_u8.encode_to(out);
                id.encode_to(out);
            }
            Self::Block(block) => {
                5_u8.encode_to(out);
                block.encode_to(out);
            }
            Self::NotFound(id) => {
                8_u8.encode_to(out);
                id.encode_to(out);
            }
            Self::Ping(nonce) => {
                6_u8.encode_to(out);
                nonce.encode_to(out);
            }
            Self::Pong(nonce) => {
                7_u8.encode_to(out);
                nonce.encode_to(out);
            }
            Self::Transaction(transaction) => {
                9_u8.encode_to(out);
                transaction.encode_to(out);
            }
            Self::TransactionAck { txid, accepted } => {
                10_u8.encode_to(out);
                txid.encode_to(out);
                accepted.encode_to(out);
            }
        }
    }
}

impl Decode for Message {
    fn decode_from(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        match u8::decode_from(reader)? {
            0 => Ok(Self::Version {
                protocol_version: u32::decode_from(reader)?,
                chain_id: Hash::decode_from(reader)?,
                best_height: Height::decode_from(reader)?,
                best_id: Hash::decode_from(reader)?,
                nonce: u64::decode_from(reader)?,
            }),
            1 => Ok(Self::Verack),
            2 => Ok(Self::GetHeaders {
                locator: BoundedVec::decode_from(reader)?,
                stop: Hash::decode_from(reader)?,
            }),
            3 => Ok(Self::Headers(BoundedVec::decode_from(reader)?)),
            4 => Ok(Self::GetBlock(Hash::decode_from(reader)?)),
            5 => Ok(Self::Block(Block::decode_from(reader)?)),
            6 => Ok(Self::Ping(u64::decode_from(reader)?)),
            7 => Ok(Self::Pong(u64::decode_from(reader)?)),
            8 => Ok(Self::NotFound(Hash::decode_from(reader)?)),
            9 => Ok(Self::Transaction(Box::new(Tx::decode_from(reader)?))),
            10 => Ok(Self::TransactionAck {
                txid: Hash::decode_from(reader)?,
                accepted: bool::decode_from(reader)?,
            }),
            tag => Err(DecodeError::UnknownTag(tag)),
        }
    }
}

/// A frame is malformed or exceeds the protocol's size limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// The fixed frame magic did not match `YLP2`.
    WrongMagic,
    /// The payload exceeds [`MAX_FRAME_PAYLOAD`].
    PayloadTooLarge(usize),
    /// The declared payload length does not equal the bytes in this frame.
    LengthMismatch {
        /// Length in the frame header.
        declared: usize,
        /// Number of payload bytes supplied.
        actual: usize,
    },
    /// The message payload is not a canonical P2P message.
    Decode(DecodeError),
}

impl fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongMagic => formatter.write_str("invalid P2P frame magic"),
            Self::PayloadTooLarge(size) => {
                write!(
                    formatter,
                    "P2P payload size {size} exceeds {MAX_FRAME_PAYLOAD}"
                )
            }
            Self::LengthMismatch { declared, actual } => {
                write!(
                    formatter,
                    "P2P frame declares {declared} bytes, found {actual}"
                )
            }
            Self::Decode(error) => error.fmt(formatter),
        }
    }
}

impl core::error::Error for FrameError {}

/// A transport read or write failed, or carried a malformed frame.
#[derive(Debug)]
pub enum TransportError {
    /// Socket or stream I/O error.
    Io(std::io::Error),
    /// Frame validation error.
    Frame(FrameError),
}

impl fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Frame(error) => error.fmt(formatter),
        }
    }
}

impl core::error::Error for TransportError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Frame(error) => Some(error),
        }
    }
}

/// Writes one complete frame to a byte stream.
///
/// # Errors
///
/// Returns [`TransportError::Frame`] for an oversized message and [`TransportError::Io`] if the
/// stream cannot accept the complete frame.
pub fn write_message(writer: &mut impl Write, message: &Message) -> Result<(), TransportError> {
    let frame = encode_frame(message).map_err(TransportError::Frame)?;
    writer.write_all(&frame).map_err(TransportError::Io)
}

/// Reads one complete frame, checking its declared length before allocating the payload buffer.
///
/// # Errors
///
/// Returns [`TransportError::Frame`] for malformed frames and [`TransportError::Io`] if the
/// stream ends before a full frame arrives.
pub fn read_message(reader: &mut impl Read) -> Result<Message, TransportError> {
    let mut header = [0_u8; FRAME_HEADER_SIZE];
    reader.read_exact(&mut header).map_err(TransportError::Io)?;
    if header.get(..4) != Some(FRAME_MAGIC.as_slice()) {
        return Err(TransportError::Frame(FrameError::WrongMagic));
    }
    let length_bytes: [u8; 4] = header
        .get(4..)
        .ok_or(TransportError::Frame(FrameError::Decode(
            DecodeError::UnexpectedEnd,
        )))?
        .try_into()
        .map_err(|_| TransportError::Frame(FrameError::Decode(DecodeError::UnexpectedEnd)))?;
    let payload_len = usize::try_from(u32::from_le_bytes(length_bytes))
        .map_err(|_| TransportError::Frame(FrameError::PayloadTooLarge(usize::MAX)))?;
    if payload_len > MAX_FRAME_PAYLOAD {
        return Err(TransportError::Frame(FrameError::PayloadTooLarge(
            payload_len,
        )));
    }
    let frame_len = FRAME_HEADER_SIZE.saturating_add(payload_len);
    let mut frame = Vec::with_capacity(frame_len);
    frame.extend_from_slice(&header);
    frame.resize(frame_len, 0);
    reader
        .read_exact(
            frame
                .get_mut(FRAME_HEADER_SIZE..)
                .ok_or(TransportError::Frame(FrameError::Decode(
                    DecodeError::UnexpectedEnd,
                )))?,
        )
        .map_err(TransportError::Io)?;
    decode_frame(&frame).map_err(TransportError::Frame)
}

/// A peer passed the version handshake or supplied a post-handshake message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandshakeEvent {
    /// The caller must send `Verack` immediately.
    SendVerack,
    /// Both peers have exchanged `Version` and `Verack`.
    Established,
    /// A valid application message received after establishment.
    Message(Message),
}

/// Why a peer message violated handshake sequencing or identity checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeError {
    /// A peer must send `Version` as its first message.
    VersionRequired,
    /// A repeated `Version` is forbidden.
    DuplicateVersion,
    /// The peer does not support this P2P version.
    UnsupportedVersion(u32),
    /// The peer announced another chain.
    WrongChain,
    /// A version nonce must be nonzero.
    ZeroNonce,
    /// Both sides announced the same connection nonce.
    SelfConnection,
    /// `Verack` arrived without a valid peer `Version` or was repeated.
    UnexpectedVerack,
}

impl fmt::Display for HandshakeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VersionRequired => formatter.write_str("peer must begin with Version"),
            Self::DuplicateVersion => formatter.write_str("peer sent Version more than once"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported P2P protocol version {version}")
            }
            Self::WrongChain => formatter.write_str("peer announced a different chain ID"),
            Self::ZeroNonce => formatter.write_str("peer announced a zero connection nonce"),
            Self::SelfConnection => formatter.write_str("self-connection detected"),
            Self::UnexpectedVerack => formatter.write_str("unexpected or repeated Verack"),
        }
    }
}

impl core::error::Error for HandshakeError {}

/// Validates one connection's `Version`/`Verack` exchange.
#[derive(Debug, Clone)]
pub struct Handshake {
    chain_id: Hash,
    local_nonce: u64,
    got_version: bool,
    sent_verack: bool,
    got_verack: bool,
}

impl Handshake {
    /// Creates a handshake and returns the local `Version` message to send first.
    ///
    /// # Errors
    ///
    /// Returns [`HandshakeError::ZeroNonce`] if the local connection nonce is zero.
    pub fn new(
        chain_id: Hash,
        best_height: Height,
        best_id: Hash,
        local_nonce: u64,
    ) -> Result<(Self, Message), HandshakeError> {
        if local_nonce == 0 {
            return Err(HandshakeError::ZeroNonce);
        }
        Ok((
            Self {
                chain_id,
                local_nonce,
                got_version: false,
                sent_verack: false,
                got_verack: false,
            },
            Message::Version {
                protocol_version: PROTOCOL_VERSION,
                chain_id,
                best_height,
                best_id,
                nonce: local_nonce,
            },
        ))
    }

    /// Processes an incoming message, returning the required response or established message.
    ///
    /// # Errors
    ///
    /// Returns [`HandshakeError`] for a bad chain/version/nonce or invalid message order.
    pub fn receive(&mut self, message: Message) -> Result<HandshakeEvent, HandshakeError> {
        if !self.got_version {
            let Message::Version {
                protocol_version,
                chain_id,
                nonce,
                ..
            } = message
            else {
                return Err(HandshakeError::VersionRequired);
            };
            if protocol_version != PROTOCOL_VERSION {
                return Err(HandshakeError::UnsupportedVersion(protocol_version));
            }
            if chain_id != self.chain_id {
                return Err(HandshakeError::WrongChain);
            }
            if nonce == 0 {
                return Err(HandshakeError::ZeroNonce);
            }
            if nonce == self.local_nonce {
                return Err(HandshakeError::SelfConnection);
            }
            self.got_version = true;
            self.sent_verack = true;
            return Ok(HandshakeEvent::SendVerack);
        }

        if !self.got_verack {
            return match message {
                Message::Verack if self.sent_verack => {
                    self.got_verack = true;
                    Ok(HandshakeEvent::Established)
                }
                Message::Version { .. } => Err(HandshakeError::DuplicateVersion),
                _ => Err(HandshakeError::UnexpectedVerack),
            };
        }

        match message {
            Message::Version { .. } => Err(HandshakeError::DuplicateVersion),
            Message::Verack => Err(HandshakeError::UnexpectedVerack),
            other => Ok(HandshakeEvent::Message(other)),
        }
    }

    /// Whether both handshake messages have been received.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.got_version && self.sent_verack && self.got_verack
    }
}

/// Encodes one message as `YLP2 ‖ payload_len:u32_le ‖ payload`.
///
/// # Errors
///
/// Returns [`FrameError::PayloadTooLarge`] if the encoded message exceeds the frame bound.
pub fn encode_frame(message: &Message) -> Result<Vec<u8>, FrameError> {
    let payload = message.encode();
    if payload.len() > MAX_FRAME_PAYLOAD {
        return Err(FrameError::PayloadTooLarge(payload.len()));
    }
    let length =
        u32::try_from(payload.len()).map_err(|_| FrameError::PayloadTooLarge(payload.len()))?;
    let mut frame = Vec::with_capacity(FRAME_HEADER_SIZE.saturating_add(payload.len()));
    frame.extend_from_slice(&FRAME_MAGIC);
    frame.extend_from_slice(&length.to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Decodes exactly one bounded frame, rejecting trailing bytes.
///
/// # Errors
///
/// Returns a [`FrameError`] if magic, length, size, or message encoding is invalid.
pub fn decode_frame(frame: &[u8]) -> Result<Message, FrameError> {
    if frame.len() < FRAME_HEADER_SIZE {
        return Err(FrameError::Decode(DecodeError::UnexpectedEnd));
    }
    if frame.get(..4) != Some(FRAME_MAGIC.as_slice()) {
        return Err(FrameError::WrongMagic);
    }
    let length_bytes: [u8; 4] = frame
        .get(4..FRAME_HEADER_SIZE)
        .ok_or(FrameError::Decode(DecodeError::UnexpectedEnd))?
        .try_into()
        .map_err(|_| FrameError::Decode(DecodeError::UnexpectedEnd))?;
    let declared = usize::try_from(u32::from_le_bytes(length_bytes))
        .map_err(|_| FrameError::PayloadTooLarge(usize::MAX))?;
    if declared > MAX_FRAME_PAYLOAD {
        return Err(FrameError::PayloadTooLarge(declared));
    }
    let payload = frame
        .get(FRAME_HEADER_SIZE..)
        .ok_or(FrameError::Decode(DecodeError::UnexpectedEnd))?;
    if payload.len() != declared {
        return Err(FrameError::LengthMismatch {
            declared,
            actual: payload.len(),
        });
    }
    decode_exact(payload).map_err(FrameError::Decode)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use yuanlai_codec::BoundedVec;
    use yuanlai_types::{
        Block, BlockHeader, Credit, Hash, Height, PubKey, Signature, Target, Timestamp, Transfer,
        Tx, U256, UserTx,
    };

    use super::{
        FrameError, Handshake, HandshakeError, HandshakeEvent, MAX_FRAME_PAYLOAD, Message,
        PROTOCOL_VERSION, TransportError, decode_frame, encode_frame, read_message, write_message,
    };

    #[test]
    fn every_message_frame_roundtrips() -> Result<(), Box<dyn std::error::Error>> {
        let header = BlockHeader {
            version: 1,
            chain_id: Hash([1; 32]),
            height: Height(0),
            prev_block: Hash::ZERO,
            tx_root: Hash([2; 32]),
            timestamp: Timestamp(1_790_000_000),
            target: Target(U256::from_u64(100)),
            nonce: 3,
        };
        let messages = [
            Message::Version {
                protocol_version: 1,
                chain_id: header.chain_id,
                best_height: header.height,
                best_id: header.id(),
                nonce: 42,
            },
            Message::Verack,
            Message::GetHeaders {
                locator: BoundedVec::empty(),
                stop: Hash::ZERO,
            },
            Message::Headers(BoundedVec::new(vec![header])?),
            Message::GetBlock(header.id()),
            Message::Block(Block {
                header,
                txs: BoundedVec::empty(),
            }),
            Message::Ping(7),
            Message::Pong(8),
            Message::NotFound(Hash([3; 32])),
            Message::Transaction(Box::new(Tx::Transfer(UserTx {
                sender: PubKey([4; 32]),
                nonce: 1,
                body: Transfer {
                    inputs: BoundedVec::empty(),
                    recipient: PubKey([5; 32]),
                    amount: Credit(1),
                },
                signature: Signature::EMPTY,
            }))),
            Message::TransactionAck {
                txid: Hash([6; 32]),
                accepted: true,
            },
        ];
        for message in messages {
            assert_eq!(decode_frame(&encode_frame(&message)?), Ok(message));
        }
        Ok(())
    }

    #[test]
    fn decoder_rejects_wrong_magic_unknown_tags_extra_bytes_and_oversize() {
        let encoded = encode_frame(&Message::Ping(7)).unwrap();
        let mut wrong_magic = encoded.clone();
        wrong_magic[0] = b'X';
        assert_eq!(decode_frame(&wrong_magic), Err(FrameError::WrongMagic));

        let mut unknown_tag = b"YLP2".to_vec();
        unknown_tag.extend_from_slice(&1_u32.to_le_bytes());
        unknown_tag.push(0xff);
        assert!(matches!(
            decode_frame(&unknown_tag),
            Err(FrameError::Decode(yuanlai_codec::DecodeError::UnknownTag(
                0xff
            )))
        ));

        let mut extra = encoded;
        extra.push(0);
        assert!(matches!(
            decode_frame(&extra),
            Err(FrameError::LengthMismatch { .. })
        ));

        let mut oversized = b"YLP2".to_vec();
        let declared = u32::try_from(MAX_FRAME_PAYLOAD + 1).unwrap();
        oversized.extend_from_slice(&declared.to_le_bytes());
        assert!(matches!(
            decode_frame(&oversized),
            Err(FrameError::PayloadTooLarge(_))
        ));
    }

    #[test]
    fn stream_transport_reads_and_writes_one_bounded_frame() {
        let message = Message::Ping(0x1234);
        let mut bytes = Vec::new();
        write_message(&mut bytes, &message).unwrap();
        assert_eq!(read_message(&mut Cursor::new(bytes)).unwrap(), message);

        let mut oversized = b"YLP2".to_vec();
        oversized.extend_from_slice(&u32::try_from(MAX_FRAME_PAYLOAD + 1).unwrap().to_le_bytes());
        assert!(matches!(
            read_message(&mut Cursor::new(oversized)),
            Err(TransportError::Frame(FrameError::PayloadTooLarge(_)))
        ));
    }

    #[test]
    fn handshake_checks_network_nonce_and_message_order() {
        let chain_id = Hash([0x42; 32]);
        let (mut handshake, local_version) =
            Handshake::new(chain_id, Height(1), Hash([1; 32]), 7).unwrap();
        assert!(matches!(
            handshake.receive(Message::Ping(0)),
            Err(HandshakeError::VersionRequired)
        ));

        let remote_version = Message::Version {
            protocol_version: PROTOCOL_VERSION,
            chain_id,
            best_height: Height(2),
            best_id: Hash([2; 32]),
            nonce: 8,
        };
        assert!(matches!(
            handshake.receive(remote_version),
            Ok(HandshakeEvent::SendVerack)
        ));
        assert_eq!(
            handshake.receive(Message::Verack),
            Ok(HandshakeEvent::Established)
        );
        assert!(handshake.is_complete());
        assert_eq!(
            handshake.receive(Message::Ping(9)),
            Ok(HandshakeEvent::Message(Message::Ping(9)))
        );
        assert!(matches!(
            handshake.receive(local_version),
            Err(HandshakeError::DuplicateVersion)
        ));

        let (mut wrong_chain, _) = Handshake::new(chain_id, Height(0), Hash::ZERO, 11).unwrap();
        assert!(matches!(
            wrong_chain.receive(Message::Version {
                protocol_version: PROTOCOL_VERSION,
                chain_id: Hash::ZERO,
                best_height: Height(0),
                best_id: Hash::ZERO,
                nonce: 12,
            }),
            Err(HandshakeError::WrongChain)
        ));
    }
}
