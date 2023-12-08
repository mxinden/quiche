// Copyright (C) 2021, Cloudflare, Inc.
// All rights reserved.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions are
// met:
//
//     * Redistributions of source code must retain the above copyright notice,
//       this list of conditions and the following disclaimer.
//
//     * Redistributions in binary form must reproduce the above copyright
//       notice, this list of conditions and the following disclaimer in the
//       documentation and/or other materials provided with the distribution.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS
// IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO,
// THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR
// PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR
// CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL,
// EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO,
// PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
// PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
// LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
// NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
// SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

use crate::Bytes;
use crate::Token;
use h3::*;
use qpack::*;
use quic::*;

use connectivity::ConnectivityEventType;

use serde::Deserialize;
use serde::Serialize;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(untagged)]
pub enum EventType {
    ConnectivityEventType(ConnectivityEventType),

    TransportEventType(TransportEventType),

    SecurityEventType(SecurityEventType),

    RecoveryEventType(RecoveryEventType),

    Http3EventType(Http3EventType),

    QpackEventType(QpackEventType),

    GenericEventType(GenericEventType),

    #[default]
    None,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub enum TimeFormat {
    Absolute,
    Delta,
    Relative,
}

#[serde_with::skip_serializing_none]
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Event {
    pub time: f32,

    // Strictly, the qlog 02 spec says we should have a name field in the
    // `Event` structure. However, serde's autogenerated Deserialize code
    // struggles to read Events properly because the `EventData` types often
    // alias. In order to work around that, we use can use a trick that will
    // give serde autogen all the information that it needs while also produced
    // a legal qlog. Specifically, strongly linking an EventData enum variant
    // with the wire-format name.
    //
    // The trick is to use Adjacent Tagging
    // (https://serde.rs/enum-representations.html#adjacently-tagged) with
    // Struct flattening (https://serde.rs/attr-flatten.html). At a high level
    // this first creates an `EventData` JSON object:
    //
    // {name: <enum variant name>, data: enum variant data }
    //
    // and then flattens those fields into the `Event` object.
    #[serde(flatten)]
    pub data: EventData,

    pub protocol_type: Option<String>,
    pub group_id: Option<String>,

    pub time_format: Option<TimeFormat>,

    #[serde(skip)]
    ty: EventType,
}

impl Event {
    /// Returns a new `Event` object with the provided time and data.
    pub fn with_time(time: f32, data: EventData) -> Self {
        let ty = EventType::from(&data);
        Event {
            time,
            data,
            protocol_type: Default::default(),
            group_id: Default::default(),
            time_format: Default::default(),
            ty,
        }
    }

    pub fn importance(&self) -> EventImportance {
        self.ty.into()
    }
}

impl PartialEq for Event {
    // custom comparison to skip over the `ty` field
    fn eq(&self, other: &Event) -> bool {
        self.time == other.time &&
            self.data == other.data &&
            self.protocol_type == other.protocol_type &&
            self.group_id == other.group_id &&
            self.time_format == other.time_format
    }
}

#[derive(Clone)]
pub enum EventImportance {
    Core,
    Base,
    Extra,
}

impl EventImportance {
    /// Returns true if this importance level is included by `other`.
    pub fn is_contained_in(&self, other: &EventImportance) -> bool {
        match (other, self) {
            (EventImportance::Core, EventImportance::Core) => true,

            (EventImportance::Base, EventImportance::Core) |
            (EventImportance::Base, EventImportance::Base) => true,

            (EventImportance::Extra, EventImportance::Core) |
            (EventImportance::Extra, EventImportance::Base) |
            (EventImportance::Extra, EventImportance::Extra) => true,

            (..) => false,
        }
    }
}

impl From<EventType> for EventImportance {
    fn from(ty: EventType) -> Self {
        match ty {
            EventType::ConnectivityEventType(
                ConnectivityEventType::ServerListening,
            ) => EventImportance::Extra,
            EventType::ConnectivityEventType(
                ConnectivityEventType::ConnectionStarted,
            ) => EventImportance::Base,
            EventType::ConnectivityEventType(
                ConnectivityEventType::ConnectionIdUpdated,
            ) => EventImportance::Base,
            EventType::ConnectivityEventType(
                ConnectivityEventType::SpinBitUpdated,
            ) => EventImportance::Base,
            EventType::ConnectivityEventType(
                ConnectivityEventType::ConnectionStateUpdated,
            ) => EventImportance::Base,
            EventType::ConnectivityEventType(
                ConnectivityEventType::MtuUpdated,
            ) => EventImportance::Extra,

            EventType::SecurityEventType(SecurityEventType::KeyUpdated) =>
                EventImportance::Base,
            EventType::SecurityEventType(SecurityEventType::KeyDiscarded) =>
                EventImportance::Base,

            EventType::TransportEventType(TransportEventType::ParametersSet) =>
                EventImportance::Core,
            EventType::TransportEventType(
                TransportEventType::DatagramsReceived,
            ) => EventImportance::Extra,
            EventType::TransportEventType(TransportEventType::DatagramsSent) =>
                EventImportance::Extra,
            EventType::TransportEventType(
                TransportEventType::DatagramDropped,
            ) => EventImportance::Extra,
            EventType::TransportEventType(TransportEventType::PacketReceived) =>
                EventImportance::Core,
            EventType::TransportEventType(TransportEventType::PacketSent) =>
                EventImportance::Core,
            EventType::TransportEventType(TransportEventType::PacketDropped) =>
                EventImportance::Base,
            EventType::TransportEventType(TransportEventType::PacketBuffered) =>
                EventImportance::Base,
            EventType::TransportEventType(
                TransportEventType::StreamStateUpdated,
            ) => EventImportance::Base,
            EventType::TransportEventType(
                TransportEventType::FramesProcessed,
            ) => EventImportance::Extra,
            EventType::TransportEventType(TransportEventType::DataMoved) =>
                EventImportance::Base,

            EventType::RecoveryEventType(RecoveryEventType::ParametersSet) =>
                EventImportance::Base,
            EventType::RecoveryEventType(RecoveryEventType::MetricsUpdated) =>
                EventImportance::Core,
            EventType::RecoveryEventType(
                RecoveryEventType::CongestionStateUpdated,
            ) => EventImportance::Base,
            EventType::RecoveryEventType(RecoveryEventType::LossTimerUpdated) =>
                EventImportance::Extra,
            EventType::RecoveryEventType(RecoveryEventType::PacketLost) =>
                EventImportance::Core,
            EventType::RecoveryEventType(
                RecoveryEventType::MarkedForRetransmit,
            ) => EventImportance::Extra,

            EventType::Http3EventType(Http3EventType::ParametersSet) =>
                EventImportance::Base,
            EventType::Http3EventType(Http3EventType::StreamTypeSet) =>
                EventImportance::Base,
            EventType::Http3EventType(Http3EventType::FrameCreated) =>
                EventImportance::Core,
            EventType::Http3EventType(Http3EventType::FrameParsed) =>
                EventImportance::Core,
            EventType::Http3EventType(Http3EventType::PushResolved) =>
                EventImportance::Extra,

            EventType::QpackEventType(QpackEventType::StateUpdated) =>
                EventImportance::Base,
            EventType::QpackEventType(QpackEventType::StreamStateUpdated) =>
                EventImportance::Base,
            EventType::QpackEventType(QpackEventType::DynamicTableUpdated) =>
                EventImportance::Extra,
            EventType::QpackEventType(QpackEventType::HeadersEncoded) =>
                EventImportance::Base,
            EventType::QpackEventType(QpackEventType::HeadersDecoded) =>
                EventImportance::Base,
            EventType::QpackEventType(QpackEventType::InstructionCreated) =>
                EventImportance::Base,
            EventType::QpackEventType(QpackEventType::InstructionParsed) =>
                EventImportance::Base,

            _ => unimplemented!(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub enum EventCategory {
    Connectivity,
    Security,
    Transport,
    Recovery,
    Http,
    Qpack,

    Error,
    Warning,
    Info,
    Debug,
    Verbose,
    Simulation,
}

impl std::fmt::Display for EventCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let v = match self {
            EventCategory::Connectivity => "connectivity",
            EventCategory::Security => "security",
            EventCategory::Transport => "transport",
            EventCategory::Recovery => "recovery",
            EventCategory::Http => "http",
            EventCategory::Qpack => "qpack",
            EventCategory::Error => "error",
            EventCategory::Warning => "warning",
            EventCategory::Info => "info",
            EventCategory::Debug => "debug",
            EventCategory::Verbose => "verbose",
            EventCategory::Simulation => "simulation",
        };

        write!(f, "{v}",)
    }
}

impl From<EventType> for EventCategory {
    fn from(ty: EventType) -> Self {
        match ty {
            EventType::ConnectivityEventType(_) => EventCategory::Connectivity,
            EventType::SecurityEventType(_) => EventCategory::Security,
            EventType::TransportEventType(_) => EventCategory::Transport,
            EventType::RecoveryEventType(_) => EventCategory::Recovery,
            EventType::Http3EventType(_) => EventCategory::Http,
            EventType::QpackEventType(_) => EventCategory::Qpack,

            _ => unimplemented!(),
        }
    }
}

impl From<&EventData> for EventType {
    fn from(event_data: &EventData) -> Self {
        match event_data {
            EventData::ServerListening { .. } =>
                EventType::ConnectivityEventType(
                    ConnectivityEventType::ServerListening,
                ),
            EventData::ConnectionStarted { .. } =>
                EventType::ConnectivityEventType(
                    ConnectivityEventType::ConnectionStarted,
                ),
            EventData::ConnectionClosed { .. } =>
                EventType::ConnectivityEventType(
                    ConnectivityEventType::ConnectionClosed,
                ),
            EventData::ConnectionIdUpdated { .. } =>
                EventType::ConnectivityEventType(
                    ConnectivityEventType::ConnectionIdUpdated,
                ),
            EventData::SpinBitUpdated { .. } => EventType::ConnectivityEventType(
                ConnectivityEventType::SpinBitUpdated,
            ),
            EventData::ConnectionStateUpdated { .. } =>
                EventType::ConnectivityEventType(
                    ConnectivityEventType::ConnectionStateUpdated,
                ),
            EventData::MtuUpdated { .. } => EventType::ConnectivityEventType(
                ConnectivityEventType::MtuUpdated,
            ),

            EventData::KeyUpdated { .. } =>
                EventType::SecurityEventType(SecurityEventType::KeyUpdated),
            EventData::KeyDiscarded { .. } =>
                EventType::SecurityEventType(SecurityEventType::KeyDiscarded),

            EventData::VersionInformation { .. } =>
                EventType::TransportEventType(
                    TransportEventType::VersionInformation,
                ),
            EventData::AlpnInformation { .. } =>
                EventType::TransportEventType(TransportEventType::AlpnInformation),
            EventData::TransportParametersSet { .. } =>
                EventType::TransportEventType(TransportEventType::ParametersSet),
            EventData::TransportParametersRestored { .. } =>
                EventType::TransportEventType(
                    TransportEventType::ParametersRestored,
                ),
            EventData::DatagramsReceived { .. } => EventType::TransportEventType(
                TransportEventType::DatagramsReceived,
            ),
            EventData::DatagramsSent { .. } =>
                EventType::TransportEventType(TransportEventType::DatagramsSent),
            EventData::DatagramDropped { .. } =>
                EventType::TransportEventType(TransportEventType::DatagramDropped),
            EventData::PacketReceived { .. } =>
                EventType::TransportEventType(TransportEventType::PacketReceived),
            EventData::PacketSent { .. } =>
                EventType::TransportEventType(TransportEventType::PacketSent),
            EventData::PacketDropped { .. } =>
                EventType::TransportEventType(TransportEventType::PacketDropped),
            EventData::PacketBuffered { .. } =>
                EventType::TransportEventType(TransportEventType::PacketBuffered),
            EventData::PacketsAcked { .. } =>
                EventType::TransportEventType(TransportEventType::PacketsAcked),
            EventData::StreamStateUpdated { .. } =>
                EventType::TransportEventType(
                    TransportEventType::StreamStateUpdated,
                ),
            EventData::FramesProcessed { .. } =>
                EventType::TransportEventType(TransportEventType::FramesProcessed),
            EventData::DataMoved { .. } =>
                EventType::TransportEventType(TransportEventType::DataMoved),

            EventData::RecoveryParametersSet { .. } =>
                EventType::RecoveryEventType(RecoveryEventType::ParametersSet),
            EventData::MetricsUpdated { .. } =>
                EventType::RecoveryEventType(RecoveryEventType::MetricsUpdated),
            EventData::CongestionStateUpdated { .. } =>
                EventType::RecoveryEventType(
                    RecoveryEventType::CongestionStateUpdated,
                ),
            EventData::LossTimerUpdated { .. } =>
                EventType::RecoveryEventType(RecoveryEventType::LossTimerUpdated),
            EventData::PacketLost { .. } =>
                EventType::RecoveryEventType(RecoveryEventType::PacketLost),
            EventData::MarkedForRetransmit { .. } =>
                EventType::RecoveryEventType(
                    RecoveryEventType::MarkedForRetransmit,
                ),

            EventData::H3ParametersSet { .. } =>
                EventType::Http3EventType(Http3EventType::ParametersSet),
            EventData::H3ParametersRestored { .. } =>
                EventType::Http3EventType(Http3EventType::ParametersRestored),
            EventData::H3StreamTypeSet { .. } =>
                EventType::Http3EventType(Http3EventType::StreamTypeSet),
            EventData::H3FrameCreated { .. } =>
                EventType::Http3EventType(Http3EventType::FrameCreated),
            EventData::H3FrameParsed { .. } =>
                EventType::Http3EventType(Http3EventType::FrameParsed),
            EventData::H3PushResolved { .. } =>
                EventType::Http3EventType(Http3EventType::PushResolved),

            EventData::QpackStateUpdated { .. } =>
                EventType::QpackEventType(QpackEventType::StateUpdated),
            EventData::QpackStreamStateUpdated { .. } =>
                EventType::QpackEventType(QpackEventType::StreamStateUpdated),
            EventData::QpackDynamicTableUpdated { .. } =>
                EventType::QpackEventType(QpackEventType::DynamicTableUpdated),
            EventData::QpackHeadersEncoded { .. } =>
                EventType::QpackEventType(QpackEventType::HeadersEncoded),
            EventData::QpackHeadersDecoded { .. } =>
                EventType::QpackEventType(QpackEventType::HeadersDecoded),
            EventData::QpackInstructionCreated { .. } =>
                EventType::QpackEventType(QpackEventType::InstructionCreated),
            EventData::QpackInstructionParsed { .. } =>
                EventType::QpackEventType(QpackEventType::InstructionParsed),

            EventData::ConnectionError { .. } =>
                EventType::GenericEventType(GenericEventType::ConnectionError),
            EventData::ApplicationError { .. } =>
                EventType::GenericEventType(GenericEventType::ApplicationError),
            EventData::InternalError { .. } =>
                EventType::GenericEventType(GenericEventType::InternalError),
            EventData::InternalWarning { .. } =>
                EventType::GenericEventType(GenericEventType::InternalError),
            EventData::Message { .. } =>
                EventType::GenericEventType(GenericEventType::Message),
            EventData::Marker { .. } =>
                EventType::GenericEventType(GenericEventType::Marker),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum DataRecipient {
    User,
    Application,
    Transport,
    Network,
    Dropped,
}

#[serde_with::skip_serializing_none]
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct RawInfo {
    pub length: Option<u64>,
    pub payload_length: Option<u64>,

    pub data: Option<Bytes>,
}

#[serde_with::skip_serializing_none]
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "name", content = "data")]
#[allow(clippy::large_enum_variant)]
pub enum EventData {
    // Connectivity
    #[serde(rename = "connectivity:server_listening")]
    ServerListening(connectivity::ServerListening),

    #[serde(rename = "connectivity:connection_started")]
    ConnectionStarted(connectivity::ConnectionStarted),

    #[serde(rename = "connectivity:connection_closed")]
    ConnectionClosed(connectivity::ConnectionClosed),

    #[serde(rename = "connectivity:connection_id_updated")]
    ConnectionIdUpdated(connectivity::ConnectionIdUpdated),

    #[serde(rename = "connectivity:spin_bit_updated")]
    SpinBitUpdated(connectivity::SpinBitUpdated),

    #[serde(rename = "connectivity:connection_state_updated")]
    ConnectionStateUpdated(connectivity::ConnectionStateUpdated),

    #[serde(rename = "connectivity:mtu_updated")]
    MtuUpdated(connectivity::MtuUpdated),

    // Security
    #[serde(rename = "security:key_updated")]
    KeyUpdated(security::KeyUpdated),

    #[serde(rename = "security:key_retired")]
    KeyDiscarded(security::KeyDiscarded),

    // Transport
    #[serde(rename = "transport:version_information")]
    VersionInformation(quic::VersionInformation),

    #[serde(rename = "transport:alpn_information")]
    AlpnInformation(quic::AlpnInformation),

    #[serde(rename = "transport:parameters_set")]
    TransportParametersSet(quic::TransportParametersSet),

    #[serde(rename = "transport:parameters_restored")]
    TransportParametersRestored(quic::TransportParametersRestored),

    #[serde(rename = "transport:datagrams_received")]
    DatagramsReceived(quic::DatagramsReceived),

    #[serde(rename = "transport:datagrams_sent")]
    DatagramsSent(quic::DatagramsSent),

    #[serde(rename = "transport:datagram_dropped")]
    DatagramDropped(quic::DatagramDropped),

    #[serde(rename = "transport:packet_received")]
    PacketReceived(quic::PacketReceived),

    #[serde(rename = "transport:packet_sent")]
    PacketSent(quic::PacketSent),

    #[serde(rename = "transport:packet_dropped")]
    PacketDropped(quic::PacketDropped),

    #[serde(rename = "transport:packet_buffered")]
    PacketBuffered(quic::PacketBuffered),

    #[serde(rename = "transport:packets_acked")]
    PacketsAcked(quic::PacketsAcked),

    #[serde(rename = "transport:stream_state_updated")]
    StreamStateUpdated(quic::StreamStateUpdated),

    #[serde(rename = "transport:frames_processed")]
    FramesProcessed(quic::FramesProcessed),

    #[serde(rename = "transport:data_moved")]
    DataMoved(quic::DataMoved),

    // Recovery
    #[serde(rename = "recovery:parameters_set")]
    RecoveryParametersSet(quic::RecoveryParametersSet),

    #[serde(rename = "recovery:metrics_updated")]
    MetricsUpdated(quic::MetricsUpdated),

    #[serde(rename = "recovery:congestion_state_updated")]
    CongestionStateUpdated(quic::CongestionStateUpdated),

    #[serde(rename = "recovery:loss_timer_updated")]
    LossTimerUpdated(quic::LossTimerUpdated),

    #[serde(rename = "recovery:packet_lost")]
    PacketLost(quic::PacketLost),

    #[serde(rename = "recovery:marked_for_retransmit")]
    MarkedForRetransmit(quic::MarkedForRetransmit),

    // HTTP/3
    #[serde(rename = "http:parameters_set")]
    H3ParametersSet(h3::H3ParametersSet),

    #[serde(rename = "http:parameters_restored")]
    H3ParametersRestored(h3::H3ParametersRestored),

    #[serde(rename = "http:stream_type_set")]
    H3StreamTypeSet(h3::H3StreamTypeSet),

    #[serde(rename = "http:frame_created")]
    H3FrameCreated(h3::H3FrameCreated),

    #[serde(rename = "http:frame_parsed")]
    H3FrameParsed(h3::H3FrameParsed),

    #[serde(rename = "http:push_resolved")]
    H3PushResolved(h3::H3PushResolved),

    // QPACK
    #[serde(rename = "qpack:state_updated")]
    QpackStateUpdated(qpack::QpackStateUpdated),

    #[serde(rename = "qpack:stream_state_updated")]
    QpackStreamStateUpdated(qpack::QpackStreamStateUpdated),

    #[serde(rename = "qpack:dynamic_table_updated")]
    QpackDynamicTableUpdated(qpack::QpackDynamicTableUpdated),

    #[serde(rename = "qpack:headers_encoded")]
    QpackHeadersEncoded(qpack::QpackHeadersEncoded),

    #[serde(rename = "qpack:headers_decoded")]
    QpackHeadersDecoded(qpack::QpackHeadersDecoded),

    #[serde(rename = "qpack:instruction_created")]
    QpackInstructionCreated(qpack::QpackInstructionCreated),

    #[serde(rename = "qpack:instruction_parsed")]
    QpackInstructionParsed(qpack::QpackInstructionParsed),

    // Generic
    #[serde(rename = "generic:connection_error")]
    ConnectionError {
        code: Option<ConnectionErrorCode>,
        description: Option<String>,
    },

    #[serde(rename = "generic:application_error")]
    ApplicationError {
        code: Option<ApplicationErrorCode>,
        description: Option<String>,
    },

    #[serde(rename = "generic:internal_error")]
    InternalError {
        code: Option<u64>,
        description: Option<String>,
    },

    #[serde(rename = "generic:internal_warning")]
    InternalWarning {
        code: Option<u64>,
        description: Option<String>,
    },

    #[serde(rename = "generic:message")]
    Message { message: String },

    #[serde(rename = "generic:marker")]
    Marker {
        marker_type: String,
        message: Option<String>,
    },
}

impl EventData {
    /// Returns size of `EventData` array of `QuicFrame`s if it exists.
    pub fn contains_quic_frames(&self) -> Option<usize> {
        // For some EventData variants, the frame array is optional
        // but for others it is mandatory.
        match self {
            EventData::PacketSent(pkt) => pkt.frames.as_ref().map(|f| f.len()),

            EventData::PacketReceived(pkt) =>
                pkt.frames.as_ref().map(|f| f.len()),

            EventData::PacketLost(pkt) => pkt.frames.as_ref().map(|f| f.len()),

            EventData::MarkedForRetransmit(ev) => Some(ev.frames.len()),
            EventData::FramesProcessed(ev) => Some(ev.frames.len()),

            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum GenericEventType {
    ConnectionError,
    ApplicationError,
    InternalError,
    InternalWarning,

    Message,
    Marker,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(untagged)]
pub enum ConnectionErrorCode {
    TransportError(TransportError),
    CryptoError(CryptoError),
    Value(u64),
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(untagged)]
pub enum ApplicationErrorCode {
    ApplicationError(ApplicationError),
    Value(u64),
}

// TODO
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum CryptoError {
    Prefix,
}

pub mod quic;

pub mod connectivity;
pub mod h3;
pub mod qpack;
pub mod security;
