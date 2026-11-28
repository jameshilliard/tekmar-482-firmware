//! UART framing shared by vendor tRPC and the binary raw bridge.
pub mod capture;
pub mod raw;
pub mod trace;
use crate::diagnostics::errors::HostError;

const MAX_PAYLOAD: usize = u8::MAX as usize;
pub const SOF: u8 = 0xca;
const EOF: u8 = 0x35;
const ESC: u8 = 0x2f;
pub const TN4: u8 = 1;
pub const TRPC: u8 = 6;
/// Custom diagnostic records; never a Send or a Normal-mode application packet.
pub const NORMAL_TRACE: u8 = 0xd0;

#[derive(Debug, PartialEq, Eq)]
pub struct Packet<'a> {
    pub kind: u8,
    pub data: &'a [u8],
}

const LENGTH_OFFSET: u16 = 1;
const KIND_OFFSET: u16 = 2;
const PAYLOAD_OFFSET: u16 = 3;
const ENCODED: u16 = u16::MAX;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Header {
    #[default]
    Escaped,
    Literal,
}

/// Resume a single TPCK frame without allocating its escaped representation.
pub struct Encoder {
    header: Header,
    offset: u16,
    sum: u8,
    escaped: Option<u8>,
}

impl Encoder {
    pub const EMPTY: Self = Self {
        header: Header::Escaped,
        offset: 0,
        sum: 0,
        escaped: None,
    };

    pub fn set_header(&mut self, header: Header) {
        *self = Self {
            header,
            ..Self::EMPTY
        };
    }

    pub fn reset(&mut self) {
        self.set_header(self.header);
    }

    pub fn started(&self) -> bool {
        self.offset != 0
    }

    pub fn finished(&self) -> bool {
        self.offset == ENCODED
    }

    /// Write at most `room` wire bytes. Keep kind and payload fixed until finished.
    pub fn write(
        &mut self,
        kind: u8,
        data: &[u8],
        room: usize,
        mut put: impl FnMut(u8),
    ) -> Result<(), TooLong> {
        let len = u8::try_from(data.len()).map_err(|_| TooLong)?;
        let checksum = PAYLOAD_OFFSET + u16::from(len);
        for _ in 0..room {
            if let Some(byte) = self.escaped.take() {
                put(byte);
                continue;
            }
            let offset = self.offset;
            let byte = match offset {
                ENCODED => break,
                0 => {
                    self.offset = LENGTH_OFFSET;
                    put(SOF);
                    continue;
                }
                LENGTH_OFFSET => len,
                KIND_OFFSET => kind,
                i if i < checksum => data[usize::from(i - PAYLOAD_OFFSET)],
                i if i == checksum => self.sum,
                _ => {
                    self.offset = ENCODED;
                    put(EOF);
                    break;
                }
            };
            if offset < checksum {
                self.sum = self.sum.wrapping_add(byte);
            }
            self.offset += 1;
            if (offset >= PAYLOAD_OFFSET || self.header == Header::Escaped)
                && matches!(byte, SOF | EOF | ESC)
            {
                self.escaped = Some(byte);
                put(ESC);
            } else {
                put(byte);
            }
        }
        Ok(())
    }
}

/// Emits nothing if the payload cannot be represented by the one-byte length.
pub fn encode(kind: u8, data: &[u8], put: impl FnMut(u8)) -> Result<(), TooLong> {
    let mut encoder = Encoder::EMPTY;
    encoder.write(kind, data, usize::MAX, put)
}

#[derive(Debug, PartialEq, Eq)]
pub struct TooLong;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Length,
    Type,
    Payload,
    Checksum,
    End,
}

pub struct Decoder {
    header: Header,
    state: State,
    escaped: bool,
    len: u8,
    used: u8,
    kind: u8,
    sum: u8,
    bytes: [u8; MAX_PAYLOAD],
}

impl Default for Decoder {
    fn default() -> Self {
        Self {
            header: Header::Escaped,
            state: State::Idle,
            escaped: false,
            len: 0,
            used: 0,
            kind: 0,
            sum: 0,
            bytes: [0; MAX_PAYLOAD],
        }
    }
}

impl Decoder {
    pub fn set_header(&mut self, header: Header) {
        self.reset();
        self.header = header;
    }

    pub fn reset(&mut self) {
        self.state = State::Idle;
        self.escaped = false;
    }

    pub fn push(&mut self, byte: u8, mut fault: impl FnMut(HostError)) -> Option<Packet<'_>> {
        let literal =
            self.header == Header::Literal && matches!(self.state, State::Length | State::Type);
        if !self.escaped && !literal {
            match byte {
                SOF => {
                    if !matches!(self.state, State::Idle | State::Length) {
                        fault(HostError::TpckResync);
                    }
                    self.state = State::Length;
                    return None;
                }
                _ if self.state == State::Idle => return None,
                EOF => {
                    let valid = self.state == State::End;
                    if !valid {
                        fault(HostError::TpckFraming);
                    }
                    self.reset();
                    return valid.then(|| Packet {
                        kind: self.kind,
                        data: &self.bytes[..usize::from(self.len)],
                    });
                }
                ESC if self.state != State::End => {
                    self.escaped = true;
                    return None;
                }
                _ => {}
            }
        }
        self.escaped = false;
        self.state = match self.state {
            State::Length => {
                self.len = byte;
                self.used = 0;
                self.sum = byte;
                State::Type
            }
            State::Type => {
                self.kind = byte;
                self.sum = self.sum.wrapping_add(byte);
                if self.len == 0 {
                    State::Checksum
                } else {
                    State::Payload
                }
            }
            State::Payload => {
                self.bytes[usize::from(self.used)] = byte;
                self.used += 1;
                self.sum = self.sum.wrapping_add(byte);
                if self.used == self.len {
                    State::Checksum
                } else {
                    State::Payload
                }
            }
            State::Checksum if byte == self.sum => State::End,
            State::Checksum => {
                fault(HostError::TpckChecksum);
                State::Idle
            }
            State::End => {
                fault(HostError::TpckFraming);
                State::Idle
            }
            State::Idle => State::Idle,
        };
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_sized_pulls_preserve_escaping_across_every_boundary() {
        let fixtures = [
            (0x35, &[][..], &[0xca, 0, 0x2f, 0x35, 0x2f, 0x35, 0x35][..]),
            (
                0x2f,
                &[0xca, 0x35, 0x2f],
                &[
                    0xca, 3, 0x2f, 0x2f, 0x2f, 0xca, 0x2f, 0x35, 0x2f, 0x2f, 0x60, 0x35,
                ],
            ),
            (6, &[0xc3], &[0xca, 1, 6, 0xc3, 0x2f, 0xca, 0x35]),
            (
                6,
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xca],
                &[
                    0xca, 13, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x2f, 0xca, 0xdd, 0x35,
                ],
            ),
        ];
        for (kind, body, golden) in fixtures {
            for budget in 1..=16 {
                let mut output = heapless::Vec::<_, 24>::new();
                let mut cursor = Encoder::EMPTY;
                cursor
                    .write(kind, body, 0, |_| panic!("no FIFO space"))
                    .unwrap();
                assert!(!cursor.started() && !cursor.finished());
                for _ in 0..golden.len().div_ceil(budget) {
                    let before = output.len();
                    cursor
                        .write(kind, body, budget, |b| output.push(b).unwrap())
                        .unwrap();
                    assert!(cursor.started());
                    assert_eq!(output.len() - before, budget.min(golden.len() - before));
                }
                assert!(cursor.finished());
                assert_eq!(output.as_slice(), golden);
                cursor
                    .write(kind, body, budget, |_| panic!("frame already done"))
                    .unwrap();
            }
        }
    }

    #[test]
    fn all_lengths_round_trip_including_escaped_lengths_and_types() {
        let payload = core::array::from_fn::<_, 255, _>(|i| [SOF, EOF, ESC][i % 3]);
        for header in [Header::Escaped, Header::Literal] {
            let mut decoder = Decoder::default();
            let mut encoder = Encoder::EMPTY;
            decoder.set_header(header);
            encoder.set_header(header);
            for n in 0..=255 {
                let mut packets = 0;
                encoder
                    .write(ESC, &payload[..n], usize::MAX, |byte| {
                        if let Some(p) = decoder.push(byte, |_| panic!("valid packet")) {
                            assert_eq!(p.kind, ESC);
                            assert_eq!(p.data, &payload[..n]);
                            packets += 1;
                        }
                    })
                    .unwrap();
                assert_eq!(packets, 1);
                encoder.reset();
            }
        }
        assert_eq!(encode(1, &[0; 256], |_| panic!()), Err(TooLong));
    }

    #[test]
    fn raw_header_is_literal_but_checksum_is_stuffed() {
        let mut decoder = Decoder::default();
        decoder.set_header(Header::Literal);
        let mut encoder = Encoder::EMPTY;
        encoder.set_header(Header::Literal);
        let mut bytes = heapless::Vec::<_, 6>::new();
        encoder
            .write(EOF, &[], 6, |byte| bytes.push(byte).unwrap())
            .unwrap();
        assert_eq!(&bytes[..], &[SOF, 0, EOF, ESC, EOF, EOF]);
        for &b in &bytes[..5] {
            assert_eq!(decoder.push(b, |_| panic!("valid packet")), None);
        }
        assert_eq!(
            decoder.push(EOF, |_| panic!("valid packet")),
            Some(Packet {
                kind: EOF,
                data: &[]
            }),
        );
    }

    #[test]
    fn faults_discard_one_packet_and_training_is_not_a_fault() {
        let mut decoder = Decoder::default();
        let mut faults = heapless::Vec::<u8, 4>::new();
        let mut good = 0;
        for b in [
            SOF, SOF, SOF, 0, 1, 2, EOF, SOF, 9, EOF, SOF, 0, 0, 0, 1, EOF, SOF, 2, 1, SOF, 0, 0,
            0, EOF,
        ] {
            if decoder
                .push(b, |code| faults.push(code as u8).unwrap())
                .is_some()
            {
                good += 1;
            }
        }
        assert_eq!(good, 1);
        assert_eq!(&faults[..], [0x84, 0x85, 0x85, 0x86]);
    }

    #[test]
    fn smoke_vectors_do_not_depend_on_the_encoder() {
        let mut d = Decoder::default();
        for data in [
            &[SOF, 1, 1, 9, 0, EOF][..],
            &[SOF, 1, 1, 9, 11, 42, EOF],
            &[SOF, 1, 1, EOF],
            &[SOF, 0, 0, 0, ESC, EOF],
            &[SOF, 9, 8],
        ] {
            for &b in data {
                assert!(d.push(b, |_| {}).is_none());
            }
        }
        for b in [SOF, 2, ESC, ESC, ESC, SOF, ESC, EOF, 0x30] {
            assert!(d.push(b, |_| {}).is_none());
        }
        assert_eq!(
            d.push(EOF, |_| panic!()),
            Some(Packet {
                kind: ESC,
                data: &[SOF, EOF]
            })
        );
        for b in [SOF, 1, 1, 0x33, ESC, EOF] {
            assert!(d.push(b, |_| {}).is_none());
        }
        assert_eq!(
            d.push(EOF, |_| panic!()),
            Some(Packet {
                kind: 1,
                data: &[0x33]
            })
        );
    }
}
