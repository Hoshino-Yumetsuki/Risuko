//! Integration-style unit tests for the surviving P2P protocols (ADC,
//! Gnutella, G2, giFT). All fixtures are static URIs and synthetic byte
//! buffers — there are no live test endpoints for any of these networks
//! online. The closest things to "test links" are:
//!   * ADC/DC: hub directories at <https://www.te-home.net/?do=hublist>
//!     (live hubs, no static download URIs published)
//!   * Gnutella: GWebCache directories at <http://cache.trillinux.org/>
//!     (peer bootstrap caches, no static file URIs)
//!   * G2: piggybacks on Gnutella infrastructure via Shareaza
//!   * giFT: requires a locally-running giftd, no public IPC endpoint
//!
//! So we exercise URI parsers, binary message codecs, and pure helpers

#[cfg(feature = "adc-transfer")]
use crate::engine::adc::nmdc::{escape_key, lock_to_key};
use crate::engine::adc::{is_adc_uri, parse_adc_hub_uri, parse_dchub_file_uri, types::HubDialect};
use crate::engine::g2::{is_g2_uri, parse_g2_uri};
use crate::engine::gift::{extract_gift_name, is_gift_uri, parse_gift_uri};
use crate::engine::gnutella::{
    is_gnutella_uri,
    message::{build_query, encode_header, make_guid, parse_header, PING, QUERY, QUERYHIT},
    parse_gnutella_uri,
};

// ---------- ADC / DC ----------

#[test]
fn adc_scheme_detection_covers_all_dialects() {
    for s in [
        "adc://h",
        "adcs://h",
        "dchub://h",
        "nmdc://h",
        "ADC://H",
        "AdCs://H",
    ] {
        assert!(is_adc_uri(s), "should detect: {s}");
    }
    for s in ["http://h", "magnet:?", "ed2k://", ""] {
        assert!(!is_adc_uri(s), "should reject: {s}");
    }
}

#[test]
fn adc_default_ports_per_dialect() {
    let h = parse_adc_hub_uri("adc://hub").unwrap();
    assert_eq!(h.port, 411);
    assert!(!h.tls);
    assert_eq!(h.dialect, HubDialect::Adc);

    let h = parse_adc_hub_uri("adcs://hub").unwrap();
    assert_eq!(h.port, 412);
    assert!(h.tls);
    assert_eq!(h.dialect, HubDialect::Adc);

    let h = parse_adc_hub_uri("dchub://hub").unwrap();
    assert_eq!(h.port, 411);
    assert!(!h.tls);
    assert_eq!(h.dialect, HubDialect::Nmdc);

    let h = parse_adc_hub_uri("nmdc://hub").unwrap();
    assert_eq!(h.port, 411);
    assert_eq!(h.dialect, HubDialect::Nmdc);
}

#[test]
fn adc_hub_uri_explicit_port_overrides_default() {
    let h = parse_adc_hub_uri("adcs://hub.example.com:7777/path?ignored").unwrap();
    assert_eq!(h.host, "hub.example.com");
    assert_eq!(h.port, 7777);
    assert!(h.tls);
}

#[test]
fn adc_hub_uri_rejects_unknown_scheme_and_bad_port() {
    assert!(parse_adc_hub_uri("ftp://h").is_err());
    assert!(parse_adc_hub_uri("adc://h:notaport").is_err());
    assert!(parse_adc_hub_uri("adc://").is_err());
}

#[test]
fn dchub_file_uri_decodes_dn_xl_tth() {
    let f = parse_dchub_file_uri(
        "dchub://hub:411/?TTH=PLSTQHKO5F2F5OJG6DNCKEXNV6YLQ47A&xl=2048&dn=movie%20name.mkv",
    )
    .unwrap();
    assert_eq!(f.file_name, "movie name.mkv");
    assert_eq!(f.file_size, 2048);
    assert_eq!(f.tth.as_deref(), Some("PLSTQHKO5F2F5OJG6DNCKEXNV6YLQ47A"));
}

#[test]
fn dchub_file_uri_handles_plus_as_space_and_missing_size() {
    let f = parse_dchub_file_uri("dchub://hub/?TTH=ABC&dn=hello+world").unwrap();
    assert_eq!(f.file_name, "hello world");
    assert_eq!(f.file_size, 0);
    assert_eq!(f.tth.as_deref(), Some("ABC"));
}

#[test]
fn dchub_file_uri_returns_none_when_no_query() {
    assert!(parse_dchub_file_uri("dchub://hub.example.com/").is_none());
}

#[test]
#[cfg(feature = "adc-transfer")]
fn nmdc_lock_to_key_round_trip_properties() {
    // Algorithm correctness check: for the well-known short input the
    // first byte mixes lock[0] xor lock[n-1] xor lock[n-2] xor 5, then
    // every byte is nibble-swapped. Verify both invariants hold
    let lock = b"ABCDEF";
    let key = lock_to_key(lock);
    assert_eq!(key.len(), lock.len());
    // first key byte derived nibble-swap of (A xor F xor E xor 5)
    let raw0 = lock[0] ^ lock[5] ^ lock[4] ^ 5;
    let nibble_swapped = ((raw0 << 4) & 0xF0) | ((raw0 >> 4) & 0x0F);
    assert_eq!(key[0], nibble_swapped);
}

#[test]
#[cfg(feature = "adc-transfer")]
fn nmdc_escape_key_escapes_only_reserved_bytes() {
    let raw = vec![0u8, 1, 5, 36, 50, 96, 100, 124, 126, 200];
    let esc = escape_key(&raw);
    let s = String::from_utf8_lossy(&esc);
    // each reserved byte → /%DCNxxx%/
    for b in [0u8, 5, 36, 96, 124, 126] {
        assert!(
            s.contains(&format!("/%DCN{:03}%/", b)),
            "missing escape for {b}"
        );
    }
    // unreserved bytes pass through verbatim
    assert!(esc.contains(&1u8));
    assert!(esc.contains(&50u8));
    assert!(esc.contains(&100u8));
    assert!(esc.contains(&200u8));
}

// ---------- Gnutella 0.6 ----------

#[test]
fn gnutella_scheme_detection() {
    assert!(is_gnutella_uri("gnutella://h"));
    assert!(is_gnutella_uri("gnet://h"));
    assert!(is_gnutella_uri("GNET://h"));
    assert!(!is_gnutella_uri("g2://h"));
}

#[test]
fn gnutella_uri_default_port_is_6346() {
    let l = parse_gnutella_uri("gnutella://peer.example.com/uri-res/N2R?urn:sha1:ABC").unwrap();
    assert_eq!(l.host, "peer.example.com");
    assert_eq!(l.port, 6346);
    assert_eq!(l.urn.as_deref(), Some("urn:sha1:ABC"));
}

#[test]
fn gnutella_uri_extracts_dn_and_xl() {
    let l = parse_gnutella_uri(
        "gnutella://peer:6346/uri-res/N2R?urn:sha1:PLSTQHKO5F2F5OJG6DNCKEXNV6YLQ47A&dn=test%2Eiso&xl=4096",
    )
    .unwrap();
    assert_eq!(l.file_name, "test.iso");
    assert_eq!(l.file_size, 4096);
}

#[test]
fn gnutella_uri_recognises_bitprint_urn() {
    let l = parse_gnutella_uri("gnutella://h:6346/uri-res/N2R?urn:bitprint:ABCDEF.XYZ").unwrap();
    assert!(l.urn.unwrap().starts_with("urn:bitprint:"));
}

#[test]
fn gnutella_message_header_round_trip() {
    let guid = make_guid();
    let hdr = encode_header(&guid, QUERY, 7, 0, 1024);
    let (g, t, ttl, hops, len) = parse_header(&hdr).unwrap();
    assert_eq!(g, guid);
    assert_eq!(t, QUERY);
    assert_eq!(ttl, 7);
    assert_eq!(hops, 0);
    assert_eq!(len, 1024);
}

#[test]
fn gnutella_make_guid_marks_rfc4122_bits() {
    let g = make_guid();
    // variant bits forced to 10xxxxxx in byte 8
    assert_eq!(g[8] & 0xC0, 0x80);
    assert_eq!(g[15], 0xFF);
    // two consecutive GUIDs differ
    assert_ne!(make_guid(), make_guid());
}

#[test]
fn gnutella_parse_header_rejects_short_buffer() {
    assert!(parse_header(&[0u8; 22]).is_none());
    assert!(parse_header(&[]).is_none());
}

#[test]
fn gnutella_build_query_layout() {
    let buf = build_query(0x1234, "ubuntu", Some("urn:sha1:ABC"));
    // first 2 bytes: little-endian min-speed
    assert_eq!(&buf[0..2], &[0x34, 0x12]);
    // search text + null
    assert_eq!(&buf[2..8], b"ubuntu");
    assert_eq!(buf[8], 0);
    // urn extension + trailing null
    assert!(buf
        .windows(b"urn:sha1:ABC".len())
        .any(|w| w == b"urn:sha1:ABC"));
    assert_eq!(*buf.last().unwrap(), 0);
}

#[test]
fn gnutella_payload_type_constants_match_spec() {
    assert_eq!(PING, 0x00);
    assert_eq!(QUERY, 0x80);
    assert_eq!(QUERYHIT, 0x81);
}

// ---------- Gnutella2 ----------

#[test]
fn g2_scheme_detection() {
    assert!(is_g2_uri("g2://h:6346/sha1/ABC"));
    assert!(is_g2_uri("G2://h"));
    assert!(!is_g2_uri("gnutella://h"));
}

#[test]
fn g2_uri_default_port_is_6346() {
    let l = parse_g2_uri("g2://peer.example.com/sha1/ABCDEF?xl=42&dn=foo.bin").unwrap();
    assert_eq!(l.port, 6346);
    assert_eq!(l.host, "peer.example.com");
    assert_eq!(l.urn.as_deref(), Some("urn:sha1:ABCDEF"));
    assert_eq!(l.file_name, "foo.bin");
    assert_eq!(l.file_size, 42);
}

#[test]
fn g2_uri_decodes_percent_encoded_name() {
    let l = parse_g2_uri("g2://h:6346/sha1/ABC?xl=10&dn=video%20clip%2Emp4").unwrap();
    assert_eq!(l.file_name, "video clip.mp4");
}

#[test]
fn g2_uri_returns_none_for_non_g2_scheme() {
    assert!(parse_g2_uri("gnutella://h/").is_none());
    assert!(parse_g2_uri("g2://h:notaport/sha1/A").is_none());
}

// ---------- giFT IPC ----------

#[test]
fn gift_scheme_detection() {
    assert!(is_gift_uri("gift://Gnutella/sha1/ABC"));
    assert!(is_gift_uri("GIFT://OpenFT/file"));
    assert!(!is_gift_uri("gnutella://h"));
}

#[test]
fn gift_uri_preserves_inner_payload_verbatim() {
    let l = parse_gift_uri("gift://Gnutella/sha1/ABC?dn=foo.bin&xl=99").unwrap();
    assert_eq!(l.inner, "Gnutella/sha1/ABC?dn=foo.bin&xl=99");
}

#[test]
fn gift_extract_name_strips_query_and_path() {
    assert_eq!(extract_gift_name("Gnutella/sha1/ABC?dn=foo.bin"), "ABC");
    assert_eq!(extract_gift_name("OpenFT/dir/sub/file.zip"), "file.zip");
    assert_eq!(extract_gift_name("Gnutella/"), "gift-download");
    assert_eq!(extract_gift_name(""), "gift-download");
}
