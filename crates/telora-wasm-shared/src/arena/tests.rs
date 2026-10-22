use super::{Error, content::*};

#[test]
fn utf8_inline_boundary_and_raw_relative_offsets_round_trip() {
    let utf8 = "中文中文中".as_bytes();
    let data: [u8; 15] = utf8.try_into().unwrap();
    let value = Bytes::Inline { data, len: 15 };
    let bytes = value.encode().unwrap();
    assert_eq!(&bytes[..15], utf8);
    assert_eq!(Bytes::decode(bytes).unwrap(), value);
    for (start, end) in [(0, 16), (100, 200), (u32::MAX - 16, u32::MAX)] {
        let value = Bytes::Slice(Slice {
            raw: 4096,
            start,
            end,
        });
        assert_eq!(Bytes::decode(value.encode().unwrap()).unwrap(), value);
    }
}

#[test]
fn invalid_raw_views_are_rejected() {
    for (raw, start, end) in [(0, 0, 16), (4096, 10, 9), (4096, 0, 15)] {
        assert_eq!(
            Bytes::Slice(Slice { raw, start, end }).encode(),
            Err(Error::Bounds)
        );
    }
    let mut bytes = Bytes::Slice(Slice {
        raw: 4096,
        start: 0,
        end: 16,
    })
    .encode()
    .unwrap();
    bytes[12] = 1;
    assert_eq!(Bytes::decode(bytes), Err(Error::Bounds));
    assert_eq!(Bytes::decode([255; 16]), Err(Error::Bounds));
}
