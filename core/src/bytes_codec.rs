//! 变长头长度前缀帧编解码器
//! 对齐 RustDesk BytesCodec：底 2 bit 编码头长度（1-4 字节），有效载荷长度 = 头部值 >> 2

use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::io;
use tokio_util::codec::{Decoder, Encoder};

const MAX_FRAME_LEN: usize = 4 * 1024 * 1024; // 4MB

#[derive(Debug, Clone, Copy)]
enum DecodeState {
    Head,
    Data(usize),
}

#[derive(Debug, Clone, Copy)]
pub struct BytesCodec {
    state: DecodeState,
}

impl BytesCodec {
    pub fn new() -> Self {
        Self {
            state: DecodeState::Head,
        }
    }
}

impl Default for BytesCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl Encoder<Bytes> for BytesCodec {
    type Error = io::Error;

    fn encode(&mut self, data: Bytes, buf: &mut BytesMut) -> Result<(), io::Error> {
        let len = data.len();
        if len <= 0x3F {
            buf.put_u8((len << 2) as u8);
        } else if len <= 0x3FFF {
            buf.put_u16_le((len << 2) as u16 | 0x1);
        } else if len <= 0x3FFFFF {
            let h = (len << 2) as u32 | 0x2;
            buf.put_u16_le((h & 0xFFFF) as u16);
            buf.put_u8((h >> 16) as u8);
        } else if len <= 0x3FFFFFFF {
            buf.put_u32_le((len << 2) as u32 | 0x3);
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "frame too large",
            ));
        }
        buf.extend_from_slice(&data);
        Ok(())
    }
}

impl Decoder for BytesCodec {
    type Item = BytesMut;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<BytesMut>, io::Error> {
        let n = match self.state {
            DecodeState::Head => match decode_head(src)? {
                Some(n) => {
                    self.state = DecodeState::Data(n);
                    n
                }
                None => return Ok(None),
            },
            DecodeState::Data(n) => n,
        };

        if src.len() < n {
            return Ok(None);
        }

        let data = src.split_to(n);
        self.state = DecodeState::Head;
        Ok(Some(data))
    }
}

fn decode_head(src: &mut BytesMut) -> io::Result<Option<usize>> {
    if src.is_empty() {
        return Ok(None);
    }
    let head_len = ((src[0] & 0x3) + 1) as usize;
    if src.len() < head_len {
        return Ok(None);
    }
    let mut n = src[0] as usize;
    if head_len > 1 {
        n |= (src[1] as usize) << 8;
    }
    if head_len > 2 {
        n |= (src[2] as usize) << 16;
    }
    if head_len > 3 {
        n |= (src[3] as usize) << 24;
    }
    n >>= 2;
    if n > MAX_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame exceeds max size",
        ));
    }
    src.advance(head_len);
    src.reserve(n);
    Ok(Some(n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;

    fn roundtrip(data: &[u8]) {
        let mut codec = BytesCodec::new();
        let mut buf = BytesMut::new();
        codec
            .encode(Bytes::copy_from_slice(data), &mut buf)
            .unwrap();

        let mut decoded = BytesMut::new();
        decoded.extend_from_slice(&buf);
        let result = codec.decode(&mut decoded).unwrap().unwrap();
        assert_eq!(&result[..], data);
        assert!(decoded.is_empty(), "should have consumed all bytes");
    }

    #[test]
    fn tiny_frame() {
        roundtrip(b"hello"); // 5 bytes → 1-byte header
    }

    #[test]
    fn medium_frame() {
        let data = vec![0xABu8; 200]; // 200 bytes → 2-byte header
        roundtrip(&data);
    }

    #[test]
    fn large_frame() {
        let data = vec![0xCDu8; 100_000]; // → 3-byte header
        roundtrip(&data);
    }

    #[test]
    fn empty_frame() {
        roundtrip(b""); // 0 bytes
    }

    #[test]
    fn partial_header_then_data() {
        let mut codec = BytesCodec::new();
        let mut buf = BytesMut::new();
        codec.encode(Bytes::from_static(b"hi"), &mut buf).unwrap();

        // 只给 1 字节（header 可能 > 1）
        let mut partial = BytesMut::from(&buf[..1]);
        assert!(codec.decode(&mut partial).unwrap().is_none());

        // 补全剩余
        partial.extend_from_slice(&buf[1..]);
        let result = codec.decode(&mut partial).unwrap().unwrap();
        assert_eq!(&result[..], b"hi");
    }
}
