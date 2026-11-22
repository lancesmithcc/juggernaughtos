//! Streaming AES-256-GCM (STREAM construction) for data too big for memory:
//! backups of the whole box, large videos. Same chunk format as `FileCipher`:
//! nonce = 8-byte prefix ‖ 32-bit big-endian counter, AAD = id ‖ last-flag.

use crate::envelope::CHUNK_SIZE;
use crate::{CryptoError, Result};
use aws_lc_rs::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use std::io::{Read, Write};

const TAG: usize = 16;

fn nonce(prefix: &[u8; 8], counter: u32) -> Nonce {
    let mut n = [0u8; NONCE_LEN];
    n[..8].copy_from_slice(prefix);
    n[8..].copy_from_slice(&counter.to_be_bytes());
    Nonce::assume_unique_for_key(n)
}

fn aad(id: &[u8], last: bool) -> Aad<Vec<u8>> {
    let mut a = id.to_vec();
    a.push(last as u8);
    Aad::from(a)
}

/// Encrypting writer. Call [`StreamSealer::finish`] to emit the final chunk.
pub struct StreamSealer<W: Write> {
    out: W,
    key: LessSafeKey,
    prefix: [u8; 8],
    id: Vec<u8>,
    counter: u32,
    buf: Vec<u8>,
}

impl<W: Write> StreamSealer<W> {
    /// `prefix` must be unique per (key, stream); store it next to the ciphertext.
    pub fn new(out: W, dek: &[u8; 32], prefix: [u8; 8], id: &[u8]) -> Result<Self> {
        Ok(Self { out, key: LessSafeKey::new(UnboundKey::new(&AES_256_GCM, dek)?), prefix, id: id.to_vec(), counter: 0, buf: Vec::with_capacity(CHUNK_SIZE * 2) })
    }

    fn emit(&mut self, data: Vec<u8>, last: bool) -> Result<()> {
        let mut chunk = data;
        self.key.seal_in_place_append_tag(nonce(&self.prefix, self.counter), aad(&self.id, last), &mut chunk)?;
        self.counter = self.counter.checked_add(1).ok_or(CryptoError::Failed)?;
        self.out.write_all(&chunk).map_err(|e| CryptoError::KeyStore(e.to_string()))
    }

    pub fn finish(mut self) -> Result<W> {
        let rest = std::mem::take(&mut self.buf);
        self.emit(rest, true)?;
        Ok(self.out)
    }
}

impl<W: Write> Write for StreamSealer<W> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(data);
        // Keep at least one byte buffered so the final chunk is never empty-by-surprise.
        while self.buf.len() > CHUNK_SIZE {
            let rest = self.buf.split_off(CHUNK_SIZE);
            let chunk = std::mem::replace(&mut self.buf, rest);
            self.emit(chunk, false).map_err(|e| std::io::Error::other(e.to_string()))?;
        }
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

/// Decrypting reader over a sealed stream. Returns an error (never partial
/// plaintext silently) if any chunk is tampered with, reordered, or the
/// stream is truncated.
pub struct StreamOpener<R: Read> {
    inner: R,
    key: LessSafeKey,
    prefix: [u8; 8],
    id: Vec<u8>,
    counter: u32,
    next: Option<Vec<u8>>,
    plain: Vec<u8>,
    pos: usize,
    done: bool,
}

impl<R: Read> StreamOpener<R> {
    pub fn new(inner: R, dek: &[u8; 32], prefix: [u8; 8], id: &[u8]) -> Result<Self> {
        Ok(Self { inner, key: LessSafeKey::new(UnboundKey::new(&AES_256_GCM, dek)?), prefix, id: id.to_vec(), counter: 0, next: None, plain: vec![], pos: 0, done: false })
    }

    fn read_chunk(&mut self) -> std::io::Result<Vec<u8>> {
        let mut buf = vec![0u8; CHUNK_SIZE + TAG];
        let mut n = 0;
        while n < buf.len() {
            let r = self.inner.read(&mut buf[n..])?;
            if r == 0 {
                break;
            }
            n += r;
        }
        buf.truncate(n);
        Ok(buf)
    }

    fn refill(&mut self) -> std::io::Result<()> {
        let current = match self.next.take() {
            Some(c) => c,
            None => self.read_chunk()?,
        };
        if current.len() < TAG {
            return Err(std::io::Error::other("sealed stream truncated"));
        }
        let lookahead = self.read_chunk()?;
        let last = lookahead.is_empty();
        let mut c = current;
        let plain = self
            .key
            .open_in_place(nonce(&self.prefix, self.counter), aad(&self.id, last), &mut c)
            .map_err(|_| std::io::Error::other("sealed stream failed authentication"))?
            .to_vec();
        self.counter += 1;
        self.plain = plain;
        self.pos = 0;
        if last {
            self.done = true;
        } else {
            self.next = Some(lookahead);
        }
        Ok(())
    }
}

impl<R: Read> Read for StreamOpener<R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.pos >= self.plain.len() {
            if self.done {
                return Ok(0);
            }
            self.refill()?;
        }
        let n = out.len().min(self.plain.len() - self.pos);
        out[..n].copy_from_slice(&self.plain[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(len: usize) {
        let dek = crate::random_bytes::<32>();
        let prefix = crate::random_bytes::<8>();
        let data: Vec<u8> = (0..len).map(|i| (i * 7 % 251) as u8).collect();
        let mut w = StreamSealer::new(Vec::new(), &dek, prefix, b"backup-1").unwrap();
        for part in data.chunks(10_000) {
            w.write_all(part).unwrap();
        }
        let ct = w.finish().unwrap();
        let mut out = Vec::new();
        StreamOpener::new(&ct[..], &dek, prefix, b"backup-1").unwrap().read_to_end(&mut out).unwrap();
        assert_eq!(out, data, "len {len}");
        // truncation at a chunk boundary must fail
        if ct.len() > CHUNK_SIZE + TAG {
            let cut = &ct[..CHUNK_SIZE + TAG];
            assert!(StreamOpener::new(cut, &dek, prefix, b"backup-1").unwrap().read_to_end(&mut Vec::new()).is_err());
        }
        // wrong id must fail
        assert!(StreamOpener::new(&ct[..], &dek, prefix, b"backup-2").unwrap().read_to_end(&mut Vec::new()).is_err());
    }

    #[test]
    fn stream_roundtrips_and_detects_truncation() {
        for len in [0, 1, CHUNK_SIZE - 1, CHUNK_SIZE, CHUNK_SIZE + 1, CHUNK_SIZE * 3 + 17] {
            roundtrip(len);
        }
    }
}
