# XMODEM.rs

The XMODEM protocol in Rust

This is derived from https://github.com/awelkie/xmodem.rs.  It has been modified
primarily to support `no_std` use and for use with the 2018 edition.  All four
permutations of standard 128-byte and 1024-byte block sizes, classic and CRC16
variants are supported for send and receive.  YMODEM and ZMODEM are not
implemented.  In addition, the `send` and `recv` methods return the number of
bytes of data sent or received.

For a `no_std` build, it is necessary to request the `core` feature in addition
to `--no-default-features` or `default-features = false` on account of
cargo#1839.  Additionally, your compiler must be known to `core_io`.  Changes
are welcome to allow the use of alternate crates providing `Read` and `Write`
traits with the same signatures as those in `std::io`; unfortunately this does
not include (most of?) the Embedded HAL crates, which provide slightly different
signatures.

# Testing
The tests require the binaries found in the `lrzsz` package.  There are no tests
for the `no_std` build.
