#![no_std]

pub mod tn4;

pub const IDENTITY: &[u8] = b"TEKMAR-482-DEV\r\n";
pub const ISP_ACK: &[u8] = b"OK ISP\r\n";

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Identify,
    Panel,
    LedTest,
    RxBegin,
    RxEnd,
    Stream(Stream),
    BadSend,
    EnterIsp,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Stream {
    Passive,
    Diagnostic,
    Ack(u8),
    Send(tn4::tx::Request),
}

/// Accept whole lines only. A damaged or oversized line is ignored until LF.
pub struct Decoder {
    bytes: [u8; 145],
    len: usize,
    discard: bool,
}

impl Default for Decoder {
    fn default() -> Self {
        Self {
            bytes: [0; 145],
            len: 0,
            discard: false,
        }
    }
}

impl Decoder {
    pub fn push(&mut self, byte: u8) -> Option<Command> {
        if byte == b'\n' {
            let line = &self.bytes[..self.len];
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let command = match (self.discard, line) {
                (false, b"ID?") => Some(Command::Identify),
                (false, b"PANEL?") => Some(Command::Panel),
                (false, b"LED-TEST") => Some(Command::LedTest),
                (false, b"RX-BEGIN") => Some(Command::RxBegin),
                (false, b"RX-END") => Some(Command::RxEnd),
                (false, b"RX-STREAM") => Some(Command::Stream(Stream::Passive)),
                (false, b"RX-DIAG") => Some(Command::Stream(Stream::Diagnostic)),
                (false, [b'R', b'X', b'-', b'A', b'C', b'K', b' ', n @ b'0'..=b'4']) => {
                    Some(Command::Stream(Stream::Ack(n - b'0')))
                }
                (false, b"ENTER-ISP") => Some(Command::EnterIsp),
                (false, [b'T', b'X', b' ', request @ ..]) => Some(
                    tn4::tx::Request::parse(request)
                        .map_or(Command::BadSend, |r| Command::Stream(Stream::Send(r))),
                ),
                _ => None,
            };
            self.len = 0;
            self.discard = false;
            return command;
        }
        if let Some(slot) = self.bytes.get_mut(self.len) {
            *slot = byte;
            self.len += 1;
        } else {
            self.discard = true;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(decoder: &mut Decoder, bytes: &[u8]) -> Option<Command> {
        let mut command = None;
        for &byte in bytes {
            if let Some(next) = decoder.push(byte) {
                assert!(command.replace(next).is_none());
            }
        }
        command
    }

    #[test]
    fn accepts_only_complete_commands() {
        let mut d = Decoder::default();
        assert_eq!(feed(&mut d, b"ID?\n"), Some(Command::Identify));
        assert_eq!(feed(&mut d, b"RX-BEGIN\r\n"), Some(Command::RxBegin));
        assert_eq!(feed(&mut d, b"RX-END\n"), Some(Command::RxEnd));
        assert_eq!(feed(&mut d, b"ENTER-ISP\r"), None);
        assert_eq!(feed(&mut d, b"\n"), Some(Command::EnterIsp));
        assert_eq!(feed(&mut d, b"ENTER-ISP\n"), Some(Command::EnterIsp));
    }

    #[test]
    fn rejects_noise_and_recovers_at_next_line() {
        let mut d = Decoder::default();
        for line in [
            b"prefixENTER-ISP\n".as_slice(),
            b"ENTER-ISP!\n",
            b"\xffID?\n",
            b"\n",
        ] {
            assert_eq!(feed(&mut d, line), None);
        }
        assert_eq!(feed(&mut d, b"ID?\r\n"), Some(Command::Identify));
    }
}
