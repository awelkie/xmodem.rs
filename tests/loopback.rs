//! Test against own implementation
extern crate tempfile;
extern crate rand;
extern crate xmodem;

use std::io::{self, Read, Write, ErrorKind};
use xmodem::{Xmodem,Checksum,BlockLength};
use std::sync::mpsc::{channel,Sender,Receiver};
use rand::{Rng, thread_rng};

struct Pipe {
    pin : Receiver<u8>,
    pout : Sender<u8>,
}

impl Read for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        for idx in 0..buf.len() {
            buf[idx] = match self.pin.recv() {
                Ok(v) => v,
                Err(e) => return Err(std::io::Error::new(ErrorKind::BrokenPipe,e)),
            }
        }
        Ok(buf.len())
    }
}

impl Write for Pipe {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        for v in buf { self.pout.send(*v).unwrap(); }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn loopback() -> (Pipe, Pipe) {
    let (s1,r1) = channel();
    let (s2,r2) = channel();
    (Pipe{ pin : r1, pout : s2 }, Pipe{ pin : r2, pout : s1 })
}

#[cfg(test)]
fn xmodem_loopback(checksum_mode:Checksum,block_length:BlockLength, data_len : usize) {
    let mut data_out = vec![0; data_len];
    // We don't really need the rng here
    for idx in 0..data_len { data_out[idx] = ((idx+7) * 13) as u8; }
    let (mut p1, mut p2) = loopback();
    let handle = std::thread::spawn(move || {
        let mut xmodem = Xmodem::new();
        xmodem.block_length = block_length;
        xmodem.send(&mut p1, &mut &data_out[..]).unwrap();
        data_out
    });
    let handle2 = std::thread::spawn(move || {
        let mut xmodem = Xmodem::new();
        let mut data_in= vec![0; 0];
        xmodem.recv(&mut p2, &mut data_in, checksum_mode).unwrap();
        data_in
    });
    
    let mut dato = handle.join().unwrap();
    // Pad output data to multiple of block length for comparison
    let bl = block_length as usize;
    for _ in 0..(bl - data_len % bl) { dato.push(0x1a); }
    let dati = handle2.join().unwrap();
    assert_eq!(dato.len(),dati.len());
    assert_eq!(dato,dati);
}

#[test]
fn xmodem_loopback_standard() {
    xmodem_loopback(Checksum::Standard,BlockLength::Standard,2000);
}

#[test]
fn xmodem_loopback_onek() {
    xmodem_loopback(Checksum::Standard,BlockLength::OneK,2200);
}

#[test]
fn xmodem_loopback_crc() {
    xmodem_loopback(Checksum::CRC16,BlockLength::Standard,2000);
}

#[test]
fn xmodem_loopback_long_crc() {
    // make sure we wrap block counter
    xmodem_loopback(Checksum::CRC16,BlockLength::Standard,50000);
}

#[test]
fn xmodem_initial_noise_test() {
    let data_len = 2000;
    let mut data_out = vec![0; data_len];
    // We don't really need the rng here
    for idx in 0..data_len { data_out[idx] = ((idx+7) * 13) as u8; }
    let (mut p1, mut p2) = loopback();
    let handle = std::thread::spawn(move || {
        let mut xmodem = Xmodem::new();
        let mut noise = vec![0; 10];
        thread_rng().fill_bytes(&mut noise);
        p1.write(&noise).unwrap();
        xmodem.send(&mut p1, &mut &data_out[..]).unwrap();
        data_out
    });
    let handle2 = std::thread::spawn(move || {
        let mut xmodem = Xmodem::new();
        let mut data_in= vec![0; 0];
        xmodem.recv(&mut p2, &mut data_in, Checksum::Standard).unwrap();
        data_in
    });
    
    let mut dato = handle.join().unwrap();
    // Pad output data to multiple of block length for comparison
    let bl = 128;
    for _ in 0..(bl - data_len % bl) { dato.push(0x1a); }
    let dati = handle2.join().unwrap();
    assert_eq!(dato.len(),dati.len());
    assert_eq!(dato,dati);
}
