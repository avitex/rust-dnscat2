use std::mem::size_of;
use std::str::{self, Utf8Error};

use bitflags::bitflags;
use bytes::{Buf, BufMut, Bytes, BytesMut};

use crate::util::parse::{self, Needed, NoNullTermError};
use crate::util::{hex, Decode, Encode, StringBytes};

/// A standard supported packet with an undecoded session body.
pub type LazyPacket = Packet<SupportedBody<SessionBodyBytes>>;

///////////////////////////////////////////////////////////////////////////////
// Packet

/// `u16` packet id.
pub type PacketId = u16;

/// A `DNSCAT` packet frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Packet<T = SupportedBody<SupportedSessionBody>> {
    id: PacketId,
    body: T,
}

impl<T> Packet<T>
where
    T: PacketBody,
{
    /// Constructs a new packet given a packet ID and body.
    pub fn new(id: PacketId, body: T) -> Self {
        Self { id, body }
    }

    /// Retrives the packet ID.
    pub fn id(&self) -> PacketId {
        self.id
    }

    /// Retrives a reference to the packet body.
    pub fn body(&self) -> &T {
        &self.body
    }

    /// Returns a mut reference to the packet body.
    pub fn body_mut(&mut self) -> &mut T {
        &mut self.body
    }

    /// Retrives the packet kind.
    pub fn kind(&self) -> PacketKind {
        self.body.packet_kind()
    }

    /// Consumes self into the packet body.
    pub fn into_body(self) -> T {
        self.body
    }

    /// Constant size of the header.
    pub fn header_size() -> usize {
        size_of::<PacketId>() + size_of::<u8>()
    }
}

impl<T> Encode for Packet<T>
where
    T: PacketBody,
{
    fn encode<B: BufMut>(&self, b: &mut B) {
        b.put_u16(self.id());
        b.put_u8(self.kind().into());
        self.body.encode(b);
    }
}

impl<T> Decode for Packet<T>
where
    T: PacketBody,
{
    type Error = PacketDecodeError;

    fn decode(b: &mut Bytes) -> Result<Self, Self::Error> {
        let id = parse::be_u16(b)?;
        let kind = parse::be_u8(b)?.into();
        let body = T::decode_kind(kind, b)?;
        Ok(Self::new(id, body))
    }
}

///////////////////////////////////////////////////////////////////////////////
// Packet Body

pub trait PacketBody: Sized + Encode {
    /// Retrives the packet kind from the body.
    fn packet_kind(&self) -> PacketKind;

    /// Decode a packet kind.
    fn decode_kind(kind: PacketKind, b: &mut Bytes) -> Result<Self, PacketDecodeError>;
}

///////////////////////////////////////////////////////////////////////////////
// Packet Kind

/// Enum of all possible packet kinds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PacketKind {
    /// `SYN` packet kind.
    SYN,
    /// `MSG` packet kind.
    MSG,
    /// `FIN` packet kind.
    FIN,
    /// `ENC` packet kind.
    ENC,
    /// `PING` packet kind.
    PING,
    /// Unsupported packet kind.
    Other(u8),
}

impl PacketKind {
    pub fn is_session_framed(self) -> bool {
        match self {
            Self::SYN | Self::MSG | Self::FIN | Self::ENC => true,
            Self::PING | Self::Other(_) => false,
        }
    }
}

impl From<PacketKind> for u8 {
    fn from(kind: PacketKind) -> u8 {
        match kind {
            PacketKind::SYN => 0x00,
            PacketKind::MSG => 0x01,
            PacketKind::FIN => 0x02,
            PacketKind::ENC => 0x03,
            PacketKind::PING => 0xFF,
            PacketKind::Other(v) => v,
        }
    }
}

impl From<u8> for PacketKind {
    fn from(kind: u8) -> Self {
        match kind {
            0x00 => Self::SYN,
            0x01 => Self::MSG,
            0x02 => Self::FIN,
            0x03 => Self::ENC,
            0xFF => Self::PING,
            v => Self::Other(v),
        }
    }
}

///////////////////////////////////////////////////////////////////////////////
// Packet Flags

bitflags! {
    /// Packet flags / options.
    pub struct PacketFlags: u16 {
        /// `OPT_NAME`
        ///
        /// Packet contains an additional field called the session name,
        /// which is a free-form field containing user-readable data
        const NAME = 0b0000_0001;
        /// `OPT_TUNNEL`
        #[deprecated]
        const TUNNEL = 0b0000_0010;
        /// `OPT_DATAGRAM`
        #[deprecated]
        const DATAGRAM = 0b0000_0100;
        /// `OPT_DOWNLOAD`
        #[deprecated]
        const DOWNLOAD = 0b0000_1000;
        /// `OPT_CHUNKED_DOWNLOAD`
        #[deprecated]
        const CHUCKED_DOWNLOAD = 0b0001_0000;
        /// `OPT_COMMAND`
        ///
        /// This is a command session, and will be tunneling command messages.
        const COMMAND = 0b0010_0000;
        /// `OPT_ENCRYPTED`
        ///
        /// We're negotiating encryption.
        const ENCRYPTED = 0b0100_0000;
    }
}

impl Default for PacketFlags {
    fn default() -> Self {
        PacketFlags::empty()
    }
}

///////////////////////////////////////////////////////////////////////////////
// Packet Error

/// Enum of all possible errors when decoding packets.
#[derive(Debug, Clone, PartialEq)]
pub enum PacketDecodeError {
    /// No null term error.
    NoNullTerm,
    /// Hex decode error.
    Hex(hex::DecodeError),
    /// UTF8 decode error.
    Utf8(Utf8Error),
    /// Unknown packet kind.
    UnknownKind(u8),
    /// Unknown encryption packet kind.
    UnknownEncKind(u8),
    /// Incomplete input error.
    Incomplete(Needed),
}

impl From<NoNullTermError> for PacketDecodeError {
    fn from(_: NoNullTermError) -> Self {
        Self::NoNullTerm
    }
}

impl From<Utf8Error> for PacketDecodeError {
    fn from(err: Utf8Error) -> Self {
        Self::Utf8(err)
    }
}

impl From<hex::DecodeError> for PacketDecodeError {
    fn from(err: hex::DecodeError) -> Self {
        Self::Hex(err)
    }
}

impl From<Needed> for PacketDecodeError {
    fn from(needed: Needed) -> Self {
        Self::Incomplete(needed)
    }
}

///////////////////////////////////////////////////////////////////////////////
// Generic Packet Body

#[derive(Debug, Clone, PartialEq)]
pub enum SupportedBody<T> {
    Ping(PingBody),
    Session(SessionBodyFrame<T>),
}

impl<T> SupportedBody<T>
where
    T: PacketBody,
{
    pub fn into_session_frame(self) -> Option<SessionBodyFrame<T>> {
        match self {
            Self::Session(frame) => Some(frame),
            _ => None,
        }
    }

    pub fn session_body_mut(&mut self) -> Option<&mut T> {
        match self {
            Self::Session(ref mut frame) => Some(frame.body_mut()),
            _ => None,
        }
    }
}

impl<T> Encode for SupportedBody<T>
where
    T: PacketBody,
{
    fn encode<B: BufMut>(&self, b: &mut B) {
        match self {
            Self::Ping(v) => v.encode(b),
            Self::Session(v) => v.encode(b),
        }
    }
}

impl<T> PacketBody for SupportedBody<T>
where
    T: PacketBody,
{
    fn packet_kind(&self) -> PacketKind {
        match self {
            Self::Ping(_) => PacketKind::PING,
            Self::Session(v) => v.packet_kind(),
        }
    }

    fn decode_kind(kind: PacketKind, b: &mut Bytes) -> Result<Self, PacketDecodeError> {
        match kind {
            PacketKind::PING => PingBody::decode(b).map(Self::Ping),
            kind if kind.is_session_framed() => {
                SessionBodyFrame::decode_kind(kind, b).map(Self::Session)
            }
            kind => Err(PacketDecodeError::UnknownKind(kind.into())),
        }
    }
}

///////////////////////////////////////////////////////////////////////////////
// Session Packet

/// `u16` session id.
pub type SessionId = u16;

#[derive(Debug, Clone, PartialEq)]
pub struct SessionBodyFrame<T> {
    id: SessionId,
    body: T,
}

impl<T> SessionBodyFrame<T>
where
    T: PacketBody,
{
    pub fn new(id: SessionId, body: T) -> Self {
        Self { id, body }
    }

    pub fn session_id(&self) -> SessionId {
        self.id
    }

    pub fn into_body(self) -> T {
        self.body
    }

    pub fn body_mut(&mut self) -> &mut T {
        &mut self.body
    }

    /// Constant size of the header.
    pub fn header_size() -> usize {
        size_of::<SessionId>()
    }
}

impl<T> Encode for SessionBodyFrame<T>
where
    T: PacketBody,
{
    fn encode<B: BufMut>(&self, b: &mut B) {
        b.put_u16(self.session_id());
        self.body.encode(b);
    }
}

impl<T> PacketBody for SessionBodyFrame<T>
where
    T: PacketBody,
{
    fn packet_kind(&self) -> PacketKind {
        self.body.packet_kind()
    }

    fn decode_kind(kind: PacketKind, b: &mut Bytes) -> Result<Self, PacketDecodeError> {
        let id = parse::be_u16(b)?;
        let body = T::decode_kind(kind, b)?;
        Ok(Self { id, body })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SupportedSessionBody {
    Syn(SynBody),
    Msg(MsgBody),
    Fin(FinBody),
    Enc(EncBody),
}

impl Encode for SupportedSessionBody {
    fn encode<B: BufMut>(&self, b: &mut B) {
        match self {
            Self::Syn(p) => p.encode(b),
            Self::Msg(p) => p.encode(b),
            Self::Fin(p) => p.encode(b),
            Self::Enc(p) => p.encode(b),
        }
    }
}

impl PacketBody for SupportedSessionBody {
    fn packet_kind(&self) -> PacketKind {
        match self {
            Self::Syn(_) => PacketKind::SYN,
            Self::Msg(_) => PacketKind::MSG,
            Self::Fin(_) => PacketKind::FIN,
            Self::Enc(_) => PacketKind::ENC,
        }
    }

    fn decode_kind(kind: PacketKind, b: &mut Bytes) -> Result<Self, PacketDecodeError> {
        match kind {
            PacketKind::SYN => SynBody::decode(b).map(Self::Syn),
            PacketKind::MSG => MsgBody::decode(b).map(Self::Msg),
            PacketKind::FIN => FinBody::decode(b).map(Self::Fin),
            PacketKind::ENC => EncBody::decode(b).map(Self::Enc),
            other => Err(PacketDecodeError::UnknownKind(other.into())),
        }
    }
}

impl From<SynBody> for SupportedSessionBody {
    fn from(packet: SynBody) -> Self {
        Self::Syn(packet)
    }
}

impl From<MsgBody> for SupportedSessionBody {
    fn from(packet: MsgBody) -> Self {
        Self::Msg(packet)
    }
}

impl From<FinBody> for SupportedSessionBody {
    fn from(packet: FinBody) -> Self {
        Self::Fin(packet)
    }
}

impl From<EncBody> for SupportedSessionBody {
    fn from(packet: EncBody) -> Self {
        Self::Enc(packet)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionBodyBytes {
    kind: PacketKind,
    body: Bytes,
}

impl SessionBodyBytes {
    pub fn new(kind: PacketKind, body: Bytes) -> Self {
        Self { kind, body }
    }

    pub fn into_bytes(self) -> Bytes {
        self.body
    }

    fn from_packet_body<T: PacketBody>(body: T) -> Self {
        let mut body_bytes = BytesMut::new();
        body.encode(&mut body_bytes);
        Self::new(body.packet_kind(), body_bytes.freeze())
    }
}

impl Encode for SessionBodyBytes {
    fn encode<B: BufMut>(&self, b: &mut B) {
        b.put(self.body.clone());
    }
}

impl PacketBody for SessionBodyBytes {
    fn packet_kind(&self) -> PacketKind {
        self.kind
    }

    fn decode_kind(kind: PacketKind, b: &mut Bytes) -> Result<Self, PacketDecodeError> {
        Ok(Self::new(kind, b.to_bytes()))
    }
}

impl From<SynBody> for SessionBodyBytes {
    fn from(packet: SynBody) -> Self {
        Self::from_packet_body(packet)
    }
}

impl From<MsgBody> for SessionBodyBytes {
    fn from(packet: MsgBody) -> Self {
        Self::from_packet_body(packet)
    }
}

impl From<FinBody> for SessionBodyBytes {
    fn from(packet: FinBody) -> Self {
        Self::from_packet_body(packet)
    }
}

impl From<EncBody> for SessionBodyBytes {
    fn from(packet: EncBody) -> Self {
        Self::from_packet_body(packet)
    }
}

///////////////////////////////////////////////////////////////////////////////
// SYN Packet

/// A `SYN` packet.
#[derive(Debug, Clone, PartialEq)]
pub struct SynBody {
    init_seq: Sequence,
    flags: PacketFlags,
    sess_name: StringBytes,
}

impl SynBody {
    /// Contructs a new `SYN` packet.
    pub fn new(init_seq: Sequence, command: bool, encrypted: bool) -> Self {
        let mut flags = PacketFlags::empty();
        if command {
            flags.insert(PacketFlags::COMMAND);
        }
        if encrypted {
            flags.insert(PacketFlags::ENCRYPTED);
        }
        Self {
            init_seq,
            flags,
            sess_name: StringBytes::new(),
        }
    }

    /// Retrives the initial sequence.
    pub fn initial_sequence(&self) -> Sequence {
        self.init_seq
    }

    /// Retrives the packet flags.
    pub fn flags(&self) -> PacketFlags {
        self.flags
    }

    /// Retrives the session name.
    pub fn session_name(&self) -> Option<&str> {
        if self.has_session_name() {
            Some(self.sess_name.as_ref())
        } else {
            None
        }
    }

    /// Returns `true` if the `NAME` packet flag is set, `false` if not.
    pub fn has_session_name(&self) -> bool {
        self.flags.contains(PacketFlags::NAME)
    }

    /// Sets the session name field and flag.
    ///
    /// # Panics
    ///
    /// Panics if session name is empty.
    ///
    /// Returns the size added to the packet.
    pub fn set_session_name<S>(&mut self, sess_name: S) -> usize
    where
        S: Into<StringBytes>,
    {
        let sess_name = sess_name.into();
        assert!(!sess_name.is_empty(), "session name is empty");
        self.flags.insert(PacketFlags::NAME);
        self.sess_name = sess_name;
        self.sess_name.len() + 1
    }

    /// Constant size of the header.
    pub fn header_size() -> usize {
        size_of::<Sequence>() * 2
    }
}

impl Encode for SynBody {
    fn encode<B: BufMut>(&self, b: &mut B) {
        b.put_u16(self.init_seq);
        b.put_u16(self.flags.bits());
        if self.has_session_name() {
            b.put_slice(self.sess_name.as_bytes());
            b.put_u8(0);
        }
    }
}

impl Decode for SynBody {
    type Error = PacketDecodeError;

    fn decode(b: &mut Bytes) -> Result<Self, Self::Error> {
        let init_seq = parse::be_u16(b)?;
        let flags_raw = parse::be_u16(b)?;
        let flags = PacketFlags::from_bits_truncate(flags_raw);
        let sess_name = if flags.contains(PacketFlags::NAME) {
            parse::nt_string::<PacketDecodeError>(b)?
        } else {
            StringBytes::new()
        };
        Ok(Self {
            init_seq,
            flags,
            sess_name,
        })
    }
}

impl PacketBody for SynBody {
    fn packet_kind(&self) -> PacketKind {
        PacketKind::SYN
    }

    fn decode_kind(kind: PacketKind, b: &mut Bytes) -> Result<Self, PacketDecodeError> {
        match kind {
            PacketKind::SYN => Self::decode(b),
            other => Err(PacketDecodeError::UnknownKind(other.into())),
        }
    }
}

///////////////////////////////////////////////////////////////////////////////
// MSG Packet

/// `u16` sequence value.
pub type Sequence = u16;

/// A `MSG` packet.
#[derive(Debug, Clone, PartialEq)]
pub struct MsgBody {
    seq: Sequence,
    ack: Sequence,
    data: Bytes,
}

impl MsgBody {
    /// Constructs a new empty `MSG` packet.
    pub fn new(seq: Sequence, ack: Sequence) -> Self {
        Self {
            seq,
            ack,
            data: Bytes::new(),
        }
    }

    /// Retrieves the seq number.
    pub fn seq(&self) -> Sequence {
        self.seq
    }

    /// Retrieves the ack number.
    pub fn ack(&self) -> Sequence {
        self.ack
    }

    /// Sets the seq number.
    pub fn set_seq(&mut self, seq: Sequence) {
        self.seq = seq;
    }

    /// Sets the ack number.
    pub fn set_ack(&mut self, ack: Sequence) {
        self.ack = ack;
    }

    /// Set the message data.
    ///
    /// Returns the size added to the message.
    pub fn set_data(&mut self, data: Bytes) -> usize {
        self.data = data;
        self.data.len()
    }

    /// Retrieves the message data.
    pub fn data(&self) -> &Bytes {
        &self.data
    }

    /// Consumes self into the message data.
    pub fn into_data(self) -> Bytes {
        self.data
    }

    /// Constant size of the header.
    pub fn header_size() -> usize {
        size_of::<Sequence>() * 2
    }
}

impl Encode for MsgBody {
    fn encode<B: BufMut>(&self, b: &mut B) {
        b.put_u16(self.seq);
        b.put_u16(self.ack);
        b.put(self.data.clone());
    }
}

impl Decode for MsgBody {
    type Error = PacketDecodeError;

    fn decode(b: &mut Bytes) -> Result<Self, Self::Error> {
        let seq = parse::be_u16(b)?;
        let ack = parse::be_u16(b)?;
        Ok(Self {
            seq,
            ack,
            data: b.to_bytes(),
        })
    }
}

impl PacketBody for MsgBody {
    fn packet_kind(&self) -> PacketKind {
        PacketKind::MSG
    }

    fn decode_kind(kind: PacketKind, b: &mut Bytes) -> Result<Self, PacketDecodeError> {
        match kind {
            PacketKind::MSG => Self::decode(b),
            other => Err(PacketDecodeError::UnknownKind(other.into())),
        }
    }
}

///////////////////////////////////////////////////////////////////////////////
// FIN Packet

/// A `FIN` packet.
#[derive(Debug, Clone, PartialEq)]
pub struct FinBody {
    reason: StringBytes,
}

impl FinBody {
    /// Constructs a new `FIN` packet.
    pub fn new<S>(reason: S) -> Self
    where
        S: Into<StringBytes>,
    {
        Self {
            reason: reason.into(),
        }
    }

    /// Retrives the reason for disconnect.
    pub fn reason(&self) -> &str {
        self.reason.as_ref()
    }
}

impl Encode for FinBody {
    fn encode<B: BufMut>(&self, b: &mut B) {
        b.put_slice(self.reason.as_bytes());
        b.put_u8(0);
    }
}

impl Decode for FinBody {
    type Error = PacketDecodeError;

    fn decode(b: &mut Bytes) -> Result<Self, Self::Error> {
        let reason = parse::nt_string::<PacketDecodeError>(b)?;
        Ok(Self::new(reason))
    }
}

impl PacketBody for FinBody {
    fn packet_kind(&self) -> PacketKind {
        PacketKind::FIN
    }

    fn decode_kind(kind: PacketKind, b: &mut Bytes) -> Result<Self, PacketDecodeError> {
        match kind {
            PacketKind::FIN => Self::decode(b),
            other => Err(PacketDecodeError::UnknownKind(other.into())),
        }
    }
}

///////////////////////////////////////////////////////////////////////////////
// ENC Packet

/// `u16` crypto flags.
pub type CryptoFlags = u16;

/// A `ENC` packet.
#[derive(Debug, Clone, PartialEq)]
pub struct EncBody {
    cryp_flags: CryptoFlags,
    body: EncBodyVariant,
}

impl EncBody {
    /// Constructs a new `ENC` packet.
    pub fn new(cryp_flags: CryptoFlags, body: EncBodyVariant) -> Self {
        Self { cryp_flags, body }
    }

    /// Retrives the crypto flags.
    ///
    /// # Notes
    ///
    /// This field is currently not used in the original specification.
    pub fn crypto_flags(&self) -> CryptoFlags {
        self.cryp_flags
    }

    /// Retrives the encryption packet kind.
    pub fn kind(&self) -> EncBodyKind {
        self.body.kind()
    }

    /// Retrives a reference to the encryption packet body.
    pub fn body(&self) -> &EncBodyVariant {
        &self.body
    }

    /// Constant size of the header.
    pub fn header_size() -> usize {
        size_of::<CryptoFlags>() + size_of::<u8>()
    }
}

impl Encode for EncBody {
    fn encode<B: BufMut>(&self, b: &mut B) {
        b.put_u8(self.body.kind() as u8);
        b.put_u16(self.cryp_flags);
        self.body.encode(b);
    }
}

impl Decode for EncBody {
    type Error = PacketDecodeError;

    fn decode(b: &mut Bytes) -> Result<Self, Self::Error> {
        let enc_kind = parse::be_u8(b)?;
        let enc_kind = EncBodyKind::from_u8(enc_kind)
            .ok_or_else(|| PacketDecodeError::UnknownEncKind(enc_kind))?;
        let cryp_flags = parse::be_u16(b)?;
        let body = EncBodyVariant::decode_kind(enc_kind, b)?;
        Ok(Self::new(cryp_flags, body))
    }
}

impl PacketBody for EncBody {
    fn packet_kind(&self) -> PacketKind {
        PacketKind::ENC
    }

    fn decode_kind(kind: PacketKind, b: &mut Bytes) -> Result<Self, PacketDecodeError> {
        match kind {
            PacketKind::ENC => Self::decode(b),
            other => Err(PacketDecodeError::UnknownKind(other.into())),
        }
    }
}

/// Enum of all supported encryption packet bodies.
#[derive(Debug, Clone, PartialEq)]
pub enum EncBodyVariant {
    /// `INIT` encyption packet body.
    Init {
        /// `X` component of public key.
        public_key_x: Bytes,
        /// `Y` component of public key.
        public_key_y: Bytes,
    },
    /// `AUTH` encyption packet body.
    Auth {
        /// Authenticator value.
        authenticator: Bytes,
    },
}

impl EncBodyVariant {
    const PART_SIZE: usize = 32;

    /// Retrives the encryption packet kind.
    pub fn kind(&self) -> EncBodyKind {
        match self {
            Self::Init { .. } => EncBodyKind::INIT,
            Self::Auth { .. } => EncBodyKind::AUTH,
        }
    }

    /// Decodes a encryption packet body given the encryption packet kind.
    ///
    /// Returns a tuple of the remaining buffer not used and the decoded encryption
    /// packet body on success or a packet decode error on failure.
    pub fn decode_kind(kind: EncBodyKind, b: &mut Bytes) -> Result<Self, PacketDecodeError> {
        match kind {
            EncBodyKind::INIT => Ok(Self::Init {
                public_key_x: Self::decode_part(b)?,
                public_key_y: Self::decode_part(b)?,
            }),
            EncBodyKind::AUTH => Ok(Self::Auth {
                authenticator: Self::decode_part(b)?,
            }),
        }
    }

    fn decode_part(b: &mut Bytes) -> Result<Bytes, PacketDecodeError> {
        parse::np_hex_string::<PacketDecodeError>(b, Self::PART_SIZE)
    }

    fn encode_part<B: BufMut>(b: &mut B, part: &[u8]) {
        let mut hex = Vec::with_capacity(Self::PART_SIZE);
        hex::encode_into_buf(&mut hex, part);
        hex.resize_with(Self::PART_SIZE, || 0);
        b.put(hex.as_ref())
    }
}

impl Encode for EncBodyVariant {
    fn encode<B: BufMut>(&self, b: &mut B) {
        match self {
            Self::Init {
                ref public_key_x,
                ref public_key_y,
            } => {
                Self::encode_part(b, &public_key_x[..]);
                Self::encode_part(b, &public_key_y[..]);
            }
            Self::Auth { ref authenticator } => {
                Self::encode_part(b, &authenticator[..]);
            }
        }
    }
}

/// Enum of all supported encryption packet kinds.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum EncBodyKind {
    /// `INIT` encryption packet kind.
    INIT = 0x00,
    /// `AUTH` encryption packet kind.
    AUTH = 0x01,
}

impl EncBodyKind {
    /// Converts a encryption packet kind value to a supported variant.
    pub fn from_u8(kind: u8) -> Option<Self> {
        match kind {
            0x00 => Some(Self::INIT),
            0x01 => Some(Self::AUTH),
            _ => None,
        }
    }
}

///////////////////////////////////////////////////////////////////////////////
// PING Packet

/// `u16` ping id.
pub type PingId = u16;

/// A `PING` packet body.
#[derive(Debug, Clone, PartialEq)]
pub struct PingBody {
    ping_id: PingId,
    data: StringBytes,
}

impl PingBody {
    /// Constructs a new `PING` packet.
    pub fn new<S>(ping_id: PingId, data: S) -> Self
    where
        S: Into<StringBytes>,
    {
        Self {
            ping_id,
            data: data.into(),
        }
    }

    /// Retrives the ping ID.
    pub fn ping_id(&self) -> PingId {
        self.ping_id
    }

    /// Retrives the ping data.
    pub fn data(&self) -> &str {
        self.data.as_ref()
    }

    /// Constant size of the header.
    pub fn header_size() -> usize {
        size_of::<PingId>()
    }
}

impl Encode for PingBody {
    fn encode<B: BufMut>(&self, b: &mut B) {
        b.put_u16(self.ping_id);
        b.put_slice(self.data.as_bytes());
        b.put_u8(0);
    }
}

impl Decode for PingBody {
    type Error = PacketDecodeError;

    fn decode(b: &mut Bytes) -> Result<Self, Self::Error> {
        let ping_id = parse::be_u16(b)?;
        let data = parse::nt_string::<PacketDecodeError>(b)?;
        Ok(Self::new(ping_id, data))
    }
}

impl PacketBody for PingBody {
    fn packet_kind(&self) -> PacketKind {
        PacketKind::PING
    }

    fn decode_kind(kind: PacketKind, b: &mut Bytes) -> Result<Self, PacketDecodeError> {
        match kind {
            PacketKind::PING => Self::decode(b),
            other => Err(PacketDecodeError::UnknownKind(other.into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_pkt_encdec_works(packet_in: &'static [u8], valid: Packet) {
        let mut bytes = Bytes::from_static(packet_in);
        let decoded = match Packet::decode(&mut bytes) {
            Ok(decoded) if bytes.is_empty() => decoded,
            Ok(_) => panic!("bytes remaining after decode: {:?}", bytes),
            Err(err) => panic!("error decoding packet: {:?}", err),
        };
        let mut packet_out = Vec::new();
        assert_eq!(valid, decoded, "valid = decoded");
        valid.encode(&mut packet_out);
        assert_eq!(
            packet_in,
            &packet_out[..],
            "packet = encoded (len {} vs {} )",
            packet_in.len(),
            packet_out.len()
        )
    }

    fn new_session_packet<B: Into<SupportedSessionBody>>(
        packet_id: PacketId,
        session_id: SessionId,
        body: B,
    ) -> Packet {
        let packet_body = SupportedBody::Session(SessionBodyFrame::new(session_id, body.into()));
        Packet::new(packet_id, packet_body)
    }

    #[test]
    #[rustfmt::skip]
    fn test_parse_pkt_syn() {
        assert_pkt_encdec_works(
            &[
                0x00, 0x01, // Packet ID
                0x00, // Packet kind
                0x00, 0x01, // Session ID
                0x00, 0x01, // Init sequence
                0x00, 0x01, // Flags (has name)
                b'h', b'e', b'l', b'l', b'o', 0x00, // Session name
            ],
            new_session_packet(1, 1, SynBody {
                init_seq: 1,
                flags: PacketFlags::NAME,
                sess_name: "hello".into(),
            })
        );
    }

    #[test]
    #[rustfmt::skip]
    fn test_parse_pkt_msg() {
        assert_pkt_encdec_works(
            &[
                0x00, 0x01, // Packet ID
                0x01, // Packet kind
                0x00, 0x01, // Session ID
                0x00, 0x02, // SEQ
                0x00, 0x03, // ACK
                b'h', b'e', b'l', b'l', b'o', // Data
            ],
            new_session_packet(1, 1, MsgBody {
                seq: 2,
                ack: 3,
                data: Bytes::from_static(b"hello"),
            }),
        );
    }

    #[test]
    #[rustfmt::skip]
    fn test_parse_pkt_fin() {
        assert_pkt_encdec_works(
            &[
                0x00, 0x01, // Packet ID
                0x02, // Packet kind
                0x00, 0x01, // Session ID
                b'd', b'r', b'a', b'g', b'o', b'n', b's', 0x00, // Reason
            ],
            new_session_packet(1, 1, FinBody {
                reason: "dragons".into(),
            }),
        );
    }

    #[test]
    #[rustfmt::skip]
    fn test_parse_pkt_enc_init() {
        fn truncate_arr(mut arr: Bytes, new_len: usize) -> Bytes {
            arr.truncate(new_len);
            arr
        }
        assert_pkt_encdec_works(
            &[
                0x00, 0x01, // Packet ID
                0x03, // Packet kind
                0x00, 0x01, // Session ID
                EncBodyKind::INIT as u8, // Encryption kind
                0x00, 0x02, // Crypto flags
                0x30, 0x33, 0x30, 0x33, 0x30, 0x33, 0x30, 0x33, 0x30, 0x33, 0x30, 0x33, 0x30, 0x33, 0x30, 0x33, // Pubkey X (1)
                0x30, 0x33, 0x30, 0x33, 0x30, 0x33, 0x30, 0x33, 0x30, 0x33, 0x30, 0x33, 0x30, 0x33, 0x00, 0x00, // Pubkey X (2)
                0x30, 0x34, 0x30, 0x34, 0x30, 0x34, 0x30, 0x34, 0x30, 0x34, 0x30, 0x34, 0x30, 0x34, 0x30, 0x34, // Pubkey Y (1)
                0x30, 0x34, 0x30, 0x34, 0x30, 0x34, 0x30, 0x34, 0x30, 0x34, 0x30, 0x34, 0x30, 0x34, 0x30, 0x34, // Pubkey Y (2)
            ],
            new_session_packet(1, 1, EncBody {
                cryp_flags: 2,
                body: EncBodyVariant::Init {
                    public_key_x: truncate_arr(Bytes::from(&[3u8; 16][..]), 15),
                    public_key_y: truncate_arr(Bytes::from(&[4u8; 16][..]), 16),
                },
            }),
        );
    }

    #[test]
    #[rustfmt::skip]
    fn test_parse_pkt_enc_auth() {
        assert_pkt_encdec_works(
            &[
                0x00, 0x01, // Packet ID
                0x03, // Packet kind
                0x00, 0x01, // Session ID
                EncBodyKind::AUTH as u8, // Encryption kind
                0x00, 0x02, // Crypto flags
                0x30, 0x33, 0x30, 0x33, 0x30, 0x33, 0x30, 0x33, 0x30, 0x33, 0x30, 0x33, 0x30, 0x33, 0x30, 0x33, // Auth (1)
                0x30, 0x33, 0x30, 0x33, 0x30, 0x33, 0x30, 0x33, 0x30, 0x33, 0x30, 0x33, 0x30, 0x33, 0x30, 0x33, // Auth (2)
            ],
            new_session_packet(1, 1, EncBody {
                cryp_flags: 2,
                body: EncBodyVariant::Auth {
                    authenticator: Bytes::from(&[3u8; 16][..]),
                },
            }),
        );
    }

    #[test]
    #[rustfmt::skip]
    fn test_parse_pkt_ping() {
        assert_pkt_encdec_works(
            &[
                0x00, 0x01, // Packet ID
                0xFF, // Packet kind
                0x00, 0x02, // Ping ID
                b'd', b'r', b'a', b'g', b'o', b'n', b's', 0x00, // Data
            ],
            Packet {
                id: 1,
                body: SupportedBody::Ping(PingBody {
                    ping_id: 2,
                    data: "dragons".into(),
                }),
            },
        );
    }
}
