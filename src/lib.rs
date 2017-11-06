#[macro_use]
extern crate log;
extern crate crc16;

use std::io::{self, Read, Write};
use std::convert::From;

// TODO: Send CAN byte after too many errors
// TODO: Handle CAN bytes while sending
// TODO: Implement Error for Error

const SOH: u8 = 0x01;
const STX: u8 = 0x02;
const EOT: u8 = 0x04;
const ACK: u8 = 0x06;
const NAK: u8 = 0x15;
const CAN: u8 = 0x18;
const CRC: u8 = 0x43;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),

    /// The number of communications errors exceeded `max_errors` in a single
    /// transmission.
    ExhaustedRetries,

    /// The transmission was canceled by the other end of the channel.
    Canceled,
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::Io(err)
    }
}

#[derive(Copy, Clone, Debug)]
pub enum Checksum {
    Standard,
    CRC16,
}

#[derive(Copy, Clone, Debug)]
pub enum BlockLength {
    Standard = 128,
    OneK = 1024,
}

/// Configuration for the XMODEM transfer.
#[derive(Copy, Clone, Debug)]
pub struct Xmodem {
    /// The number of errors that can occur before the communication is
    /// considered a failure. Errors include unexpected bytes and timeouts waiting for bytes.
    pub max_errors: u32,

    /// The byte used to pad the last block. XMODEM can only send blocks of a certain size,
    /// so if the message is not a multiple of that size the last block needs to be padded.
    pub pad_byte: u8,

    /// The length of each block. There are only two options: 128-byte blocks (standard
    ///  XMODEM) or 1024-byte blocks (XMODEM-1k).
    pub block_length: BlockLength,

    /// The checksum mode used by XMODEM. This is determined by the receiver.
    checksum_mode: Checksum,
    errors: u32,
}

impl Xmodem {
    /// Creates the XMODEM config with default parameters.
    pub fn new() -> Self {
        Xmodem {
            max_errors: 16,
            pad_byte: 0x1a,
            block_length: BlockLength::Standard,
            checksum_mode: Checksum::Standard,
            errors: 0,
        }
    }

    /// Starts the XMODEM transmission.
    ///
    /// `dev` should be the serial communication channel (e.g. the serial device).
    /// `stream` should be the message to send (e.g. a file).
    ///
    /// # Timeouts
    /// This method has no way of setting the timeout of `dev`, so it's up to the caller
    /// to set the timeout of the device before calling this method. Timeouts on receiving
    /// bytes will be counted against `max_errors`, but timeouts on transmitting bytes
    /// will be considered a fatal error.
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

    /// Receive an XMODEM transmission.
    ///
    /// `dev` should be the serial communication channel (e.g. the serial device).
    /// The received data will be written to `outstream`.
    /// `checksum` indicates which checksum mode should be used; Checksum::Standard is
    /// a reasonable default.
    ///
    /// # Timeouts
    /// This method has no way of setting the timeout of `dev`, so it's up to the caller
    /// to set the timeout of the device before calling this method. Timeouts on receiving
    /// bytes will be counted against `max_errors`, but timeouts on transmitting bytes
    /// will be considered a fatal error.
    pub fn recv<D: Read + Write, W: Write>(&mut self,
                                           dev: &mut D,
                                           outstream: &mut W,
                                           checksum : Checksum) -> Result<()> {
        self.errors = 0;
        self.checksum_mode = checksum;
        debug!("Starting XMODEM receive");
        try!(dev.write(&[match self.checksum_mode {
            Checksum::Standard => NAK,
            Checksum::CRC16 => CRC }]));
        debug!("NCG sent. Receiving stream.");
        let mut packet_num : u8 = 1;
        loop {
            match try!(get_byte_timeout(dev)) {
                Some(SOH) => { // Handle next packet
                    let pnum = try!(get_byte(dev)); // specified packet number
                    let pnum_1c = try!(get_byte(dev)); // same, 1's complemented
                    // We'll respond with cancel later if the packet number is wrong
                    let cancel_packet = packet_num != pnum ||  (255-pnum) != pnum_1c;
                    let mut data : [u8; 128] = [0; 128];
                    try!(dev.read_exact(&mut data));
                    let success = match self.checksum_mode {
                        Checksum::Standard => {
                            let recv_checksum = try!(get_byte(dev));
                            calc_checksum(&data) == recv_checksum },
                        Checksum::CRC16 => {
                            let recv_checksum =
                                ( (try!(get_byte(dev)) as u16) << 8) +
                                try!(get_byte(dev)) as u16;
                            calc_crc(&data) == recv_checksum },
                    };

                    if cancel_packet {
                        try!(dev.write(&[CAN]));
                            try!(dev.write(&[CAN]));
                        return Err(Error::Canceled)
                    }
                    if success {
                        packet_num = packet_num.wrapping_add(1);
                        try!(dev.write(&[ACK]));
                        try!(outstream.write_all(&data));
                    } else {
                        try!(dev.write(&[NAK]));
                        self.errors += 1;
                    }
                },
                Some(EOT) => { // End of file
                    try!(dev.write(&[ACK]));
                    break
                },
                Some(_) => {
                    warn!("Unrecognized symbol!");
                },
                None => { self.errors += 1; warn!("Timeout!") },
            }
            if self.errors >= self.max_errors {
                error!("Exhausted max retries ({}) while waiting for ACK for EOT", self.max_errors);
                return Err(Error::ExhaustedRetries);
            }
        }
        Ok(())
    }
          
    fn start_send<D: Read + Write>(&mut self, dev: &mut D) -> Result<()> {
        let mut cancels = 0u32;
        loop {
            match try!(get_byte_timeout(dev)) {
                Some(c) => {
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
                None => warn!("Timed out waiting for start of XMODEM transfer."),
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
                    let checksum = calc_checksum(&buff[3..]);
                    buff.push(checksum);
                },
                Checksum::CRC16 => {
                    let crc = calc_crc(&buff[3..]);
                    buff.push(((crc >> 8) & 0xFF) as u8);
                    buff.push((crc & 0xFF) as u8);
                }
            }

            debug!("Sending block {}", block_num);
            try!(dev.write_all(&buff));

            match try!(get_byte_timeout(dev)) {
                Some(c) => {
                    if c == ACK {
                        debug!("Received ACK for block {}", block_num);
                        continue
                    } else {
                        warn!("Expected ACK, got {}", c);
                    }
                    // TODO handle CAN bytes
                },
                None => warn!("Timeout waiting for ACK for block {}", block_num),
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

            match try!(get_byte_timeout(dev)) {
                Some(c) => {
                    if c == ACK {
                        info!("XMODEM transmission successful");
                        return Ok(());
                    } else {
                        warn!("Expected ACK, got {}", c);
                    }
                },
                None => warn!("Timeout waiting for ACK for EOT"),
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
    data.iter().fold(0, |x, &y| x.wrapping_add(y))
}

fn calc_crc(data: &[u8]) -> u16 {
    crc16::State::<crc16::XMODEM>::calculate(data)
}

fn get_byte<R: Read>(reader: &mut R) -> std::io::Result<u8> {
    let mut buff = [0];
    try!(reader.read_exact(&mut buff));
    Ok(buff[0])
}

/// Turns timeout errors into `Ok(None)`
fn get_byte_timeout<R: Read>(reader: &mut R) -> std::io::Result<Option<u8>> {
    match get_byte(reader) {
        Ok(c) => Ok(Some(c)),
        Err(err) => {
            if err.kind() == io::ErrorKind::TimedOut {
                Ok(None)
            } else {
                Err(err)
            }
        }
    }
}
