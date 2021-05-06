#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
use core::convert::{From, TryFrom};
#[cfg(feature = "std")]
use std::convert::{From, TryFrom};

#[cfg(not(feature = "std"))]
use core_io::{self as io, Read, Write};
#[cfg(feature = "std")]
use std::io::{self, Read, Write};

use ::log::{debug, error, info, log, warn};
use crc16;

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

pub type Result<T> = core::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
	Io(io::Error),

	/// The number of communications errors exceeded `max_errors` in a
	/// single transmission.
	ExhaustedRetries,

	/// The transmission was canceled by the other end of the channel.
	Canceled,

	/// Data was received that is not appropriate to the transfer state.
	Invalid,

	/// A packet was received with mismatched sequence numbers.
	SequenceMismatch,

	/// A packet was received with an incorrect checksum or CRC16.
	Checksum,
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

struct XmodemPacket {
	pub seqno: u8,
	data: XmodemBuffer,
}

enum XmodemBuffer {
	Standard([u8; 128]),
	OneK([u8; 1024]),
}

impl XmodemPacket {
	pub fn new(l: BlockLength, pad: u8) -> Self {
		match (l) {
			BlockLength::Standard => XmodemPacket {
				seqno: 0,
				data: XmodemBuffer::Standard([pad; 128]),
			},
			BlockLength::OneK => XmodemPacket {
				seqno: 0,
				data: XmodemBuffer::OneK([pad; 1024]),
			},
		}
	}

	fn recv_next<R: Read>(r: &mut R, c: Checksum) -> Result<Option<Self>> {
		let mut ret = match get_byte(r)? {
			SOH => Self::new(BlockLength::Standard, 0),
			STX => Self::new(BlockLength::OneK, 0),
			EOT => return (Ok(None)),
			_ => return (Err(Error::Invalid)),
		};

		/*
		 * The next two bytes are the packet sequence number mod 256
		 * and the 1's complement of that.  If they don't match we'll
		 * return an error later; we still need to read the packet
		 * to maintain proper transaction state.
		 */
		let recv_seqno = get_byte(r)?;
		let recv_seqno1c = get_byte(r)?;

		r.read_exact(ret.as_mut())?;

		let checksum_ok = match c {
			Checksum::Standard => {
				let recv_checksum = get_byte(r)?;
				calc_checksum(ret.as_ref()) == recv_checksum
			}
			Checksum::CRC16 => {
				calc_crc(ret.as_ref()) ==
					u16::from_be_bytes([
						get_byte(r)?,
						get_byte(r)?,
					])
			}
		};

		if (0xFF - recv_seqno != recv_seqno1c) {
			return (Err(Error::SequenceMismatch));
		}

		if (checksum_ok) {
			ret.seqno = recv_seqno;
			return (Ok(Some(ret)));
		}

		Err(Error::Checksum)
	}

	fn send<W: Write>(&self, w: &mut W, c: Checksum) -> Result<()> {
		let header: [u8; 3] = [
			match (self.data) {
				XmodemBuffer::Standard(_) => SOH,
				XmodemBuffer::OneK(_) => STX,
			},
			self.seqno,
			0xFF - self.seqno,
		];

		debug!("Sending block {}", self.seqno);
		w.write_all(&header)?;
		w.write_all(self.as_ref())?;

		match (c) {
			Checksum::Standard => {
				w.write_all(&[calc_checksum(self.as_ref())])?;
			}
			Checksum::CRC16 => {
				w.write_all(
					&calc_crc(self.as_ref()).to_be_bytes(),
				)?;
			}
		}

		Ok(())
	}
}

impl AsRef<[u8]> for XmodemPacket {
	fn as_ref(&self) -> &[u8] {
		match (self.data) {
			XmodemBuffer::Standard(ref b) => b,
			XmodemBuffer::OneK(ref b) => b,
		}
	}
}

impl AsMut<[u8]> for XmodemPacket {
	fn as_mut(&mut self) -> &mut [u8] {
		match (self.data) {
			XmodemBuffer::Standard(ref mut b) => b,
			XmodemBuffer::OneK(ref mut b) => b,
		}
	}
}

/// Configuration for the XMODEM transfer.
#[derive(Copy, Clone, Debug)]
pub struct Xmodem {
	/// The number of errors that can occur before the communication is
	/// considered a failure. Errors include unexpected bytes and timeouts
	/// waiting for bytes.
	pub max_errors: u32,

	/// The byte used to pad the last block. XMODEM can only send blocks of
	/// a certain size, so if the message is not a multiple of that size
	/// the last block needs to be padded.
	pub pad_byte: u8,

	/// The length of each block. There are only two options: 128-byte
	/// blocks (standard  XMODEM) or 1024-byte blocks (XMODEM-1k).
	pub block_length: BlockLength,

	/// The checksum mode used by XMODEM. This is determined by the
	/// receiver.
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
	/// `dev` should be the serial communication channel (e.g. the serial
	/// device). `stream` should be the message to send (e.g. a file).
	///
	/// # Timeouts
	/// This method has no way of setting the timeout of `dev`, so it's up
	/// to the caller to set the timeout of the device before calling this
	/// method. Timeouts on receiving bytes will be counted against
	/// `max_errors`, but timeouts on transmitting bytes will be considered
	/// a fatal error.
	pub fn send<D: Read + Write, R: Read>(
		&mut self,
		dev: &mut D,
		stream: &mut R,
	) -> Result<()> {
		self.errors = 0;

		debug!("Starting XMODEM transfer");
		self.start_send(dev)?;
		debug!("First byte received. Sending stream.");
		self.send_stream(dev, stream)?;
		debug!("Sending EOT");
		self.finish_send(dev)?;

		Ok(())
	}

	/// Receive an XMODEM transmission.
	///
	/// `dev` should be the serial communication channel (e.g. the serial
	/// device). The received data will be written to `outstream`.
	/// `checksum` indicates which checksum mode should be used;
	/// Checksum::Standard is the original wrapping 8-bit checksum; you
	/// probably want CRC16 if the remote supports it (which, in 2021, it's
	/// all but certain to do).
	///
	/// # Timeouts
	/// This method has no way of setting the timeout of `dev`, so it's up
	/// to the caller to set the timeout of the device before calling this
	/// method. Timeouts on receiving bytes will be counted against
	/// `max_errors`, but timeouts on transmitting bytes will be considered
	/// a fatal error.
	pub fn recv<D: Read + Write, W: Write>(
		&mut self,
		dev: &mut D,
		outstream: &mut W,
		checksum: Checksum,
	) -> Result<()> {
		self.errors = 0;
		self.checksum_mode = checksum;
		debug!("Starting XMODEM receive");
		dev.write(&[match self.checksum_mode {
			Checksum::Standard => NAK,
			Checksum::CRC16 => CRC,
		}])?;
		debug!("NCG sent. Receiving stream.");
		let mut seqno = 1;
		loop {
			if (self.errors >= self.max_errors) {
				error!(
					"Exhausted max retries ({}) while \
					 waiting for data packet {}",
					self.max_errors, seqno
				);
				return Err(Error::ExhaustedRetries);
			}

			let packet = match (XmodemPacket::recv_next(
				dev,
				self.checksum_mode,
			)) {
				Ok(Some(x)) => {
					if (x.seqno != (seqno & 0xFF)) {
						dev.write(&[CAN])?;
						dev.write(&[CAN])?;
						return (Err(Error::Canceled));
					}
					x
				}
				Ok(None) => {
					dev.write(&[ACK])?;
					break;
				}
				Err(Error::Io(e)) => match (e.kind()) {
					io::ErrorKind::TimedOut => {
						self.errors += 1;
						warn!("Timeout!");
						continue;
					}
					_ => return (Err(Error::Io(e))),
				},
				Err(Error::Checksum) => {
					dev.write(&[NAK])?;
					self.errors += 1;
					continue;
				}
				Err(Error::SequenceMismatch) => {
					dev.write(&[CAN])?;
					dev.write(&[CAN])?;

					/* XXX Is this the right code? */
					return (Err(Error::Canceled));
				}
				Err(Error::Invalid) => {
					warn!("Unrecognized symbol!");
					continue;
				}
				Err(e) => return (Err(e)),
			};

			dev.write(&[ACK])?;
			outstream.write_all(packet.as_ref())?;
			seqno = seqno.wrapping_add(1);
		}

		Ok(())
	}

	fn start_send<D: Read + Write>(&mut self, dev: &mut D) -> Result<()> {
		let mut cancels = 0;
		loop {
			match get_byte_timeout(dev)? {
				Some(NAK) => {
					debug!("Standard checksum requested");
					self.checksum_mode = Checksum::Standard;
					return Ok(());
				}
				Some(CRC) => {
					debug!("16-bit CRC requested");
					self.checksum_mode = Checksum::CRC16;
					return Ok(());
				}
				Some(CAN) => {
					warn!("Cancel (CAN) byte received");
					cancels += 1;
				}
				Some(c) => warn!(
					"Unknown byte received at start of \
					 XMODEM transfer: {}",
					c
				),
				None => warn!("Timed out waiting for start \
				               of XMODEM transfer."),
			}

			self.errors += 1;

			if cancels >= 2 {
				error!("Transmission canceled: received two \
				        cancel (CAN) bytes at start of \
				        XMODEM transfer");
				return Err(Error::Canceled);
			}

			if self.errors >= self.max_errors {
				error!(
					"Exhausted max retries ({}) at start \
					 of XMODEM transfer.",
					self.max_errors
				);
				if let Err(err) = dev.write_all(&[CAN]) {
					warn!(
						"Error sending CAN byte: {}",
						err
					);
				}
				return Err(Error::ExhaustedRetries);
			}
		}
	}

	fn send_stream<D: Read + Write, R: Read>(
		&mut self,
		dev: &mut D,
		stream: &mut R,
	) -> Result<()> {
		let mut seqno = 0;
		loop {
			let mut packet = XmodemPacket::new(
				self.block_length,
				self.pad_byte,
			);

			let n = stream.read(packet.as_mut())?;
			if (n == 0) {
				debug!("Reached EOF");
				return Ok(());
			}

			seqno += 1;
			packet.seqno = u8::try_from(seqno & 0xFF).unwrap();
			packet.send(dev, self.checksum_mode)?;

			match (get_byte_timeout(dev)?) {
				Some(ACK) => {
					debug!(
						"Received ACK for block {}",
						seqno
					);
					continue;
				}
				// TODO handle CAN bytes
				Some(b) => {
					warn!("Expected ACK, got {}", b);
				}
				None => warn!(
					"Timeout waiting for ACK for block {}",
					seqno
				),
			}

			self.errors += 1;

			if self.errors >= self.max_errors {
				error!(
					"Exhausted max retries ({}) while \
					 sending block {} in XMODEM transfer",
					self.max_errors, seqno
				);
				return Err(Error::ExhaustedRetries);
			}
		}
	}

	fn finish_send<D: Read + Write>(&mut self, dev: &mut D) -> Result<()> {
		loop {
			dev.write_all(&[EOT])?;

			match get_byte_timeout(dev)? {
				Some(ACK) => {
					info!("XMODEM transmission successful");
					return Ok(());
				}
				Some(b) => {
					warn!("Expected ACK, got {}", b);
				}
				None => {
					warn!("Timeout waiting for ACK for EOT")
				}
			}

			self.errors += 1;

			if (self.errors >= self.max_errors) {
				error!(
					"Exhausted max retries ({}) while \
					 waiting for ACK for EOT",
					self.max_errors
				);
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

fn get_byte<R: Read>(reader: &mut R) -> io::Result<u8> {
	let mut buff = [0];
	reader.read_exact(&mut buff)?;
	Ok(buff[0])
}

/// Turns timeout errors into `Ok(None)`
fn get_byte_timeout<R: Read>(reader: &mut R) -> io::Result<Option<u8>> {
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
