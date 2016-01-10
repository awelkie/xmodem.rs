#[macro_use]
extern crate log;
extern crate serial;

use std::io::{self, Read};
use std::ops::Add;
use std::convert::From;
use serial::SerialDevice;

const SOH: u8 = 0x01;
const STX: u8 = 0x02;
const EOT: u8 = 0x04;
const ACK: u8 = 0x06;
const DLE: u8 = 0x10;
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

enum Checksum {
    Standard,
    CRC16,
}

pub fn xmodem_send<D: SerialDevice, R: io::Read>(dev: &mut D, stream: &mut R, max_retries: u32) -> Result<()> {
    debug!("Starting XMODEM transfer");
    let checksum_mode = try!(xmodem_send_start(dev, max_retries));
    debug!("First byte received. Sending stream.");
    try!(xmodem_send_stream(dev, stream, max_retries));
    debug!("Sending EOT");
    try!(xmodem_finish_send(dev, max_retries));

    Ok(())
}

fn xmodem_send_start<D: SerialDevice>(dev: &mut D, max_retries: u32) -> Result<Checksum> {
    let mut errors = 0u32;
    let mut cancels = 0u32;
    loop {
        match dev.bytes().next() {
            Some(Ok(c)) => {
                match c {
                    NAK => {
                        debug!("Standard checksum requested");
                        return Ok(Checksum::Standard);
                    }
                    CRC => {
                        debug!("16-bit CRC requested");
                        return Ok(Checksum::CRC16);
                    }
                    CAN => {
                        warn!("Cancel (CAN) byte received");
                        cancels += 1;
                    },
                    c => warn!("Unknown byte received at start of XMODEM transfer: {}", c),
                }
            },
            Some(Err(err)) => {
                if err.kind() == io::ErrorKind::TimedOut {
                    warn!("Timed out waiting for start of XMODEM transfer.");
                } else {
                    return Err(From::from(err));
                }
            },
            None => warn!("No bytes available waiting for start of XMODEM transfer"),
        }

        errors += 1;

        if cancels >= 2 {
            error!("Transmission canceled: received two cancel (CAN) bytes \
                    at start of XMODEM transfer");
            return Err(Error::Canceled);
        }

        if errors >= max_retries {
            error!("Exhausted max retries ({}) at start of XMODEM transfer.", max_retries);
            // TODO send CAN in this case.
            return Err(Error::ExhaustedRetries);
        }
    }
}

fn xmodem_send_stream<D: SerialDevice, R: io::Read>(dev: &mut D, stream: &mut R, max_retries: u32) -> Result<()> {
    let mut errors = 0u32;
    let mut block_num = 0u32;
    let pad = 0x1a;
    let block_size = 128;
    loop {
        let mut buff = vec![pad; block_size + 3];
        let n = try!(stream.read(&mut buff[3..]));
        if n == 0 {
            debug!("Reached EOF");
            return Ok(());
        }

        block_num += 1;
        buff[0] = SOH;
        buff[1] = (block_num & 0xFF) as u8;
        buff[2] = 0xFF - buff[1];
        // TODO support 16-bit CRC.
        let checksum = calc_checksum(&buff);
        buff.push(checksum);

        debug!("Sending block {}", block_num);
        try!(dev.write_all(&buff));

        match dev.bytes().next() {
            Some(Ok(c)) => {
                if c == ACK {
                    debug!("Received ACK for block {}", block_num);
                    continue
                } else {
                    warn!("Expected ACK, got {}", c);
                }
            },
            Some(Err(err)) => {
                if err.kind() == io::ErrorKind::TimedOut {
                    warn!("Timeout waiting for ACK for block {}", block_num);
                } else {
                    return Err(From::from(err));
                }
            },
            None => warn!("No bytes available waiting for ACK"),
        }

        errors += 1;

        if errors >= max_retries {
            error!("Exhausted max retries ({}) while sending block {} in XMODEM transfer",
                   max_retries, block_num);
            return Err(Error::ExhaustedRetries);
        }
    }
}

fn xmodem_finish_send<D: SerialDevice>(dev: &mut D, max_retries: u32) -> Result<()> {
    let mut errors = 0u32;
    loop {
        try!(dev.write_all(&[EOT]));

        match dev.bytes().next() {
            Some(Ok(c)) => {
                if c == ACK {
                    info!("XMODEM transmission successful");
                    return Ok(());
                } else {
                    warn!("Expected ACK, got {}", c);
                }
            },
            Some(Err(err)) => {
                if err.kind() == io::ErrorKind::TimedOut {
                    warn!("Timeout waiting for ACK for EOT");
                } else {
                    return Err(From::from(err));
                }
            },
            None => warn!("No bytes available waiting for ACK"),
        }

        errors += 1;

        if errors >= max_retries {
            error!("Exhausted max retries ({}) while waiting for ACK for EOT", max_retries);
            return Err(Error::ExhaustedRetries);
        }
    }
}

fn calc_checksum(data: &[u8]) -> u8 {
    data.iter().fold(0, Add::add)
}
