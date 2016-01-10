#[macro_use]
extern crate log;
extern crate crc16;

use std::io::{self, Read, Write};
use std::ops::Add;
use std::convert::From;

// TODO: Send CAN byte after too many errors
// TODO: Handle CAN bytes while sending

const SOH: u8 = 0x01;
const STX: u8 = 0x02;
const EOT: u8 = 0x04;
const ACK: u8 = 0x06;
const NAK: u8 = 0x15;
const CAN: u8 = 0x18;
const CRC: u8 = 0x67;

pub type Result<T> = std::result::Result<T, Error>;

pub enum Error {
    Io(io::Error),
    ExhaustedRetries,
    Canceled,
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::Io(err)
    }
}

#[derive(Copy, Clone, Debug)]
enum Checksum {
    Standard,
    CRC16,
}

#[derive(Copy, Clone, Debug)]
pub enum BlockLength {
    Standard = 128,
    OneK = 1024,
}

pub struct Xmodem {
    max_errors: u32,
    errors: u32,
    pad_byte: u8,
    block_length: BlockLength,
    checksum_mode: Checksum,
}

impl Xmodem {
    pub fn new() -> Self {
        Xmodem {
            max_errors: 16,
            errors: 0,
            pad_byte: 0x1a,
            block_length: BlockLength::Standard,
            checksum_mode: Checksum::Standard,
        }
    }

    pub fn block_length<'a>(&'a mut self, block_length: BlockLength) -> &'a mut Self {
        self.block_length = block_length;
        self
    }

    pub fn max_errors<'a>(&'a mut self, max_errors: u32) -> &'a mut Self {
        self.max_errors = max_errors;
        self
    }

    pub fn pad_byte<'a>(&'a mut self, pad_byte: u8) -> &'a mut Self {
        self.pad_byte = pad_byte;
        self
    }

    pub fn send<D: Read + Write, R: Read>(&mut self, dev: &mut D, stream: &mut R) -> Result<()> {
        self.errors = 0;

        debug!("Starting XMODEM transfer");
        try!(self.start_send(dev));
        debug!("First byte received. Sending stream.");
        try!(self.send_stream(dev, stream));
        debug!("Sending EOT");
        try!(self.finish_send(dev));

        Ok(())
    }

    pub fn start_send<D: Read + Write>(&mut self, dev: &mut D) -> Result<()> {
        let mut cancels = 0u32;
        loop {
            match get_byte(dev) {
                Ok(c) => {
                    match c {
                        NAK => {
                            debug!("Standard checksum requested");
                            self.checksum_mode = Checksum::Standard;
                            return Ok(());
                        }
                        CRC => {
                            debug!("16-bit CRC requested");
                            self.checksum_mode = Checksum::CRC16;
                            return Ok(());
                        }
                        CAN => {
                            warn!("Cancel (CAN) byte received");
                            cancels += 1;
                        },
                        c => warn!("Unknown byte received at start of XMODEM transfer: {}", c),
                    }
                },
                Err(err) => {
                    if err.kind() == io::ErrorKind::TimedOut {
                        warn!("Timed out waiting for start of XMODEM transfer.");
                    } else {
                        return Err(From::from(err));
                    }
                },
            }

            self.errors += 1;

            if cancels >= 2 {
                error!("Transmission canceled: received two cancel (CAN) bytes \
                        at start of XMODEM transfer");
                return Err(Error::Canceled);
            }

            if self.errors >= self.max_errors {
                error!("Exhausted max retries ({}) at start of XMODEM transfer.", self.max_errors);
                if let Err(err) = dev.write_all(&[CAN]) {
                    warn!("Error sending CAN byte: {}", err);
                }
                return Err(Error::ExhaustedRetries);
            }
        }
    }

    fn send_stream<D: Read + Write, R: Read>(&mut self, dev: &mut D, stream: &mut R) -> Result<()> {
        let mut block_num = 0u32;
        loop {
            let mut buff = vec![self.pad_byte; self.block_length as usize + 3];
            let n = try!(stream.read(&mut buff[3..]));
            if n == 0 {
                debug!("Reached EOF");
                return Ok(());
            }

            block_num += 1;
            buff[0] = match self.block_length {
                BlockLength::Standard => SOH,
                BlockLength::OneK => STX,
            };
            buff[1] = (block_num & 0xFF) as u8;
            buff[2] = 0xFF - buff[1];

            match self.checksum_mode {
                Checksum::Standard => {
                    let checksum = calc_checksum(&buff);
                    buff.push(checksum);
                },
                Checksum::CRC16 => {
                    let crc = calc_crc(&buff);
                    buff.push(((crc >> 8) & 0xFF) as u8);
                    buff.push((crc & 0xFF) as u8);
                }
            }

            debug!("Sending block {}", block_num);
            try!(dev.write_all(&buff));

            match get_byte(dev) {
                Ok(c) => {
                    if c == ACK {
                        debug!("Received ACK for block {}", block_num);
                        continue
                    } else {
                        warn!("Expected ACK, got {}", c);
                    }
                    // TODO handle CAN bytes
                },
                Err(err) => {
                    if err.kind() == io::ErrorKind::TimedOut {
                        warn!("Timeout waiting for ACK for block {}", block_num);
                    } else {
                        return Err(From::from(err));
                    }
                },
            }

            self.errors += 1;

            if self.errors >= self.max_errors {
                error!("Exhausted max retries ({}) while sending block {} in XMODEM transfer",
                       self.max_errors, block_num);
                return Err(Error::ExhaustedRetries);
            }
        }
    }

    fn finish_send<D: Read + Write>(&mut self, dev: &mut D) -> Result<()> {
        loop {
            try!(dev.write_all(&[EOT]));

            match get_byte(dev) {
                Ok(c) => {
                    if c == ACK {
                        info!("XMODEM transmission successful");
                        return Ok(());
                    } else {
                        warn!("Expected ACK, got {}", c);
                    }
                },
                Err(err) => {
                    if err.kind() == io::ErrorKind::TimedOut {
                        warn!("Timeout waiting for ACK for EOT");
                    } else {
                        return Err(From::from(err));
                    }
                },
            }

            self.errors += 1;

            if self.errors >= self.max_errors {
                error!("Exhausted max retries ({}) while waiting for ACK for EOT", self.max_errors);
                return Err(Error::ExhaustedRetries);
            }
        }
    }
}

fn calc_checksum(data: &[u8]) -> u8 {
    data.iter().fold(0, Add::add)
}

fn calc_crc(data: &[u8]) -> u16 {
    crc16::State::<crc16::XMODEM>::calculate(data)
}

fn get_byte<R: Read>(reader: &mut R) -> std::io::Result<u8> {
    let mut buff = [0];
    try!(reader.read_exact(&mut buff));
    Ok(buff[0])
}
