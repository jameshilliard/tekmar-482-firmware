//! UART framing shared by vendor tRPC and the binary raw bridge.

pub const SOF: u8 = 0xca;
const EOF: u8 = 0x35;
const ESC: u8 = 0x2f;
pub const TN4: u8 = 1;
pub const TRPC: u8 = 6;

#[derive(Debug, PartialEq, Eq)]
pub struct Packet<'a> {
    pub kind: u8,
    pub data: &'a [u8],
}

/// Emits nothing if the payload cannot be represented by the one-byte length.
pub fn encode(kind: u8, data: &[u8], mut put: impl FnMut(u8)) -> Result<(), TooLong> {
    let len = u8::try_from(data.len()).map_err(|_| TooLong)?;
    let sum = data
        .iter()
        .copied()
        .fold(len.wrapping_add(kind), u8::wrapping_add);
    put(SOF);
    for b in [len, kind]
        .into_iter()
        .chain(data.iter().copied())
        .chain([sum])
    {
        if matches!(b, SOF | EOF | ESC) {
            put(ESC);
        }
        put(b);
    }
    put(EOF);
    Ok(())
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
    state: State,
    escaped: bool,
    len: u8,
    used: u8,
    kind: u8,
    sum: u8,
    bytes: [u8; 255],
}

impl Default for Decoder {
    fn default() -> Self {
        Self {
            state: State::Idle,
            escaped: false,
            len: 0,
            used: 0,
            kind: 0,
            sum: 0,
            bytes: [0; 255],
        }
    }
}

impl Decoder {
    pub fn reset(&mut self) {
        self.state = State::Idle;
        self.escaped = false;
    }

    pub fn push(&mut self, byte: u8) -> Option<Packet<'_>> {
        if !self.escaped {
            match byte {
                SOF => {
                    self.state = State::Length;
                    return None;
                }
                _ if self.state == State::Idle => return None,
                EOF => {
                    let valid = self.state == State::End;
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
            _ => State::Idle,
        };
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_lengths_round_trip_including_escaped_lengths_and_types() {
        for n in 0..=255 {
            let payload = core::array::from_fn::<_, 255, _>(|i| [SOF, EOF, ESC][i % 3]);
            let mut decoder = Decoder::default();
            let mut packets = 0;
            encode(ESC, &payload[..n], |byte| {
                if let Some(p) = decoder.push(byte) {
                    assert_eq!(p.kind, ESC);
                    assert_eq!(p.data, &payload[..n]);
                    packets += 1;
                }
            })
            .unwrap();
            assert_eq!(packets, 1);
        }
        assert_eq!(encode(1, &[0; 256], |_| panic!()), Err(TooLong));
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
                assert!(d.push(b).is_none());
            }
        }
        for b in [SOF, 2, ESC, ESC, ESC, SOF, ESC, EOF, 0x30] {
            assert!(d.push(b).is_none());
        }
        assert_eq!(
            d.push(EOF),
            Some(Packet {
                kind: ESC,
                data: &[SOF, EOF]
            })
        );
        for b in [SOF, 1, 1, 0x33, ESC, EOF] {
            assert!(d.push(b).is_none());
        }
        assert_eq!(
            d.push(EOF),
            Some(Packet {
                kind: 1,
                data: &[0x33]
            })
        );
    }
}
