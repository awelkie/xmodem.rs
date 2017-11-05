//! Test against the `sx` program.

extern crate tempfile;
extern crate rand;
extern crate xmodem;

use std::process::{Command, Stdio, ChildStdin, ChildStdout};
use std::io::{self, Read, Write, Seek};
use tempfile::NamedTempFile;
use rand::{Rng, thread_rng};
use xmodem::{Xmodem,Checksum};

struct ChildStdInOut {
    stdin: ChildStdin,
    stdout: ChildStdout,
}

impl Read for ChildStdInOut {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stdout.read(buf)
    }
}

impl Write for ChildStdInOut {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stdin.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stdin.flush()
    }
}

#[cfg(test)]
fn xmodem_recv(checksum_mode:Checksum) {
    let data_len = 2000;
    let mut data = vec![0; data_len];
    thread_rng().fill_bytes(&mut data);

    let mut send_file = NamedTempFile::new().unwrap();
    send_file.write_all(&data).unwrap();

    let send = Command::new("sb")
        .arg("--xmodem")
        .arg(send_file.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn().unwrap();
    
    let tx_stream = send.stdin.unwrap();
    let rx_stream = send.stdout.unwrap();
    let mut serial_dev = ChildStdInOut { stdin: tx_stream, stdout: rx_stream };

    let mut xmodem = Xmodem::new();
    let mut recv_data = Vec::new();
    xmodem.recv(&mut serial_dev,&mut recv_data, checksum_mode).unwrap();

    let mut sent_data = Vec::new();
    send_file.seek(std::io::SeekFrom::Start(0)).unwrap();
    send_file.read_to_end(&mut sent_data).unwrap();
    let mut padded_data = sent_data.clone();
    for _ in 0..(128 - sent_data.len() % 128) {
         padded_data.push(0x1a);
    }
    assert_eq!(padded_data, recv_data);
}

#[test]
fn xmodem_recv_standard() {
    xmodem_recv(Checksum::Standard);
}

#[test]
fn xmodem_recv_crc() {
    xmodem_recv(Checksum::CRC16);
}

#[test]
fn xmodem_send_standard() {
    let data_len = 2000;
    let mut data = vec![0; data_len];
    thread_rng().fill_bytes(&mut data);

    let mut recv_file = NamedTempFile::new().unwrap();
    let recv = Command::new("rb")
                       .arg("--xmodem")
                       .arg(recv_file.path())
                       .stdin(Stdio::piped())
                       .stdout(Stdio::piped())
                       .stderr(Stdio::null())
                       .spawn().unwrap();

    let tx_stream = recv.stdin.unwrap();
    let rx_stream = recv.stdout.unwrap();
    let mut serial_dev = ChildStdInOut { stdin: tx_stream, stdout: rx_stream };

    let mut xmodem = Xmodem::new();
    xmodem.send(&mut serial_dev, &mut &data[..]).unwrap();

    let mut received_data = Vec::new();
    recv_file.read_to_end(&mut received_data).unwrap();
    let mut padded_data = data.clone();
    for _ in 0..(128 - data.len() % 128) {
        padded_data.push(0x1a);
    }
    assert_eq!(received_data, padded_data);
}

#[test]
fn xmodem_send_crc() {
    let data_len = 2000;
    let mut data = vec![0; data_len];
    thread_rng().fill_bytes(&mut data);

    let mut recv_file = NamedTempFile::new().unwrap();
    let recv = Command::new("rb")
                       .arg("--xmodem")
                       .arg("--with-crc")
                       .arg(recv_file.path())
                       .stdin(Stdio::piped())
                       .stdout(Stdio::piped())
                       .stderr(Stdio::null())
                       .spawn().unwrap();

    let tx_stream = recv.stdin.unwrap();
    let rx_stream = recv.stdout.unwrap();
    let mut serial_dev = ChildStdInOut { stdin: tx_stream, stdout: rx_stream };

    let mut xmodem = Xmodem::new();
    xmodem.send(&mut serial_dev, &mut &data[..]).unwrap();

    let mut received_data = Vec::new();
    recv_file.read_to_end(&mut received_data).unwrap();
    let mut padded_data = data.clone();
    for _ in 0..(128 - data.len() % 128) {
        padded_data.push(0x1a);
    }
    assert_eq!(received_data, padded_data);
}
