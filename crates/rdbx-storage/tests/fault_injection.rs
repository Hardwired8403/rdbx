//! Property-based and fault-injection tests for the storage layer.

use std::path::PathBuf;

use proptest::prelude::*;
use rdbx_core::{DatabaseOptions, PageId};
use rdbx_storage::{Page, PageManager, PageType};

fn unique_path(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "rdbx-fault-{tag}-{}-{}.rdbx",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    p
}

proptest! {
    /// Any single-byte corruption in the header or payload must be caught by
    /// the page checksum.
    #[test]
    fn single_byte_corruption_detected(
        idx in 0usize..4096,
        flip in 1u8..=255,
    ) {
        let mut page = Page::new(PageId(2), PageType::Data, 4096);
        for i in 0..page.payload.len() {
            page.payload[i] = (i.wrapping_mul(7) & 0xFF) as u8;
        }
        let mut raw = page.encode(4096).unwrap();
        raw[idx] ^= flip;
        let result = Page::decode(&raw, 4096, PageId(2));
        prop_assert!(result.is_err(), "corruption at byte {} not detected", idx);
    }

    /// File header encode/decode round-trips for arbitrary field values.
    ///
    /// We generate the *exponent* of the page size, not the size itself:
    /// powers of two are too sparse within `[4 KiB, 64 KiB]` to be sampled
    /// by raw filtering (`prop_assume!`), which would exhaust proptest's
    /// rejection budget.
    #[test]
    fn file_header_roundtrip(
        page_size_exp in 12u32..=16u32,
        page_count in 2u64..=1_000_000,
        free_head in 0u64..=1_000_000,
        free_count in 0u64..=1_000_000,
    ) {
        let page_size: u32 = 1u32 << page_size_exp;
        let fh = rdbx_storage::FileHeader {
            format_major: 1,
            format_minor: 0,
            page_size,
            file_uuid: [0xAB; 16],
            created_at_unix_nanos: 12345,
            page_count,
            free_list_head: free_head,
            free_page_count: free_count,
            checkpoint_lsn: rdbx_core::Lsn(99),
        };
        let page = fh.to_page().unwrap();
        let fh2 = rdbx_storage::FileHeader::from_page(&page).unwrap();
        prop_assert_eq!(fh.page_size, fh2.page_size);
        prop_assert_eq!(fh.page_count, fh2.page_count);
        prop_assert_eq!(fh.free_list_head, fh2.free_list_head);
        prop_assert_eq!(fh.free_page_count, fh2.free_page_count);
    }
}

#[test]
fn corrupted_magic_rejected() {
    let path = unique_path("magic");
    {
        let mut db = PageManager::create(&path, &DatabaseOptions::default()).unwrap();
        let pid = db.allocate_page(PageType::Data).unwrap();
        let mut page = db.read_page(pid).unwrap();
        page.payload[0..4].copy_from_slice(b"test");
        db.write_page(&page).unwrap();
        db.flush().unwrap();
    }
    let mut bytes = std::fs::read(&path).unwrap();
    let magic_offset = 24; // PAGE_HEADER_SIZE
    bytes[magic_offset] ^= 0xFF;
    std::fs::write(&path, &bytes).unwrap();

    let err = PageManager::open(&path, &DatabaseOptions::default()).err();
    assert!(err.is_some(), "corrupted magic should not open");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn truncated_file_rejected() {
    let path = unique_path("truncate");
    {
        let mut db = PageManager::create(&path, &DatabaseOptions::default()).unwrap();
        db.allocate_page(PageType::Data).unwrap();
        db.flush().unwrap();
    }
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..100]).unwrap();

    let err = PageManager::open(&path, &DatabaseOptions::default()).err();
    assert!(err.is_some(), "truncated file should not open");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn read_only_rejects_writes() {
    let path = unique_path("readonly");
    {
        let mut db = PageManager::create(&path, &DatabaseOptions::default()).unwrap();
        db.allocate_page(PageType::Data).unwrap();
        db.flush().unwrap();
    }
    let opts = DatabaseOptions {
        read_only: true,
        ..Default::default()
    };
    let mut db = PageManager::open(&path, &opts).unwrap();
    assert!(db.is_read_only());
    assert!(db.allocate_page(PageType::Data).is_err());
    assert!(db.free_page(PageId(2)).is_err());
    let mut page = db.read_page(PageId(2)).unwrap();
    page.payload[0] = 42;
    assert!(db.write_page(&page).is_err());
    assert!(db.read_page(PageId(2)).is_ok());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn free_list_well_formed_after_random_ops() {
    let path = unique_path("freelist");
    let mut db = PageManager::create(&path, &DatabaseOptions::default()).unwrap();
    let mut ids = Vec::new();
    for _ in 0..32 {
        ids.push(db.allocate_page(PageType::Data).unwrap());
    }
    for i in (0..ids.len()).step_by(2) {
        db.free_page(ids[i]).unwrap();
    }
    assert!(db.is_free_list_well_formed().unwrap());
    assert_eq!(db.free_page_count(), 16);
    for _ in 0..8 {
        db.allocate_page(PageType::Index).unwrap();
    }
    assert!(db.is_free_list_well_formed().unwrap());
    assert_eq!(db.free_page_count(), 8);
    let _ = std::fs::remove_file(&path);
}
