//! Basic Phase 1 demo: create a DB, write a page, reopen, read it back.

use rdbx::{Database, DatabaseOptions, PageType};

/// Demo payload. Length is derived from the slice itself so that edits to
/// the message cannot silently desynchronize the write and read paths.
const MSG: &[u8] = b"Hello, RDBX!";

fn main() -> rdbx::Result<()> {
    let path = std::env::temp_dir().join("rdbx-example.rdbx");
    let _ = std::fs::remove_file(&path);

    let db = Database::open(&path, DatabaseOptions::default())?;
    println!(
        "Created {} (page_size={}, page_count={})",
        path.display(),
        db.page_size(),
        db.page_count()
    );

    let pid = db.page_manager().allocate_page(PageType::Data)?;
    let mut page = db.page_manager().read_page(pid)?;
    page.payload[..MSG.len()].copy_from_slice(MSG);
    db.page_manager().write_page(&page)?;
    db.flush()?;
    println!("Wrote page {pid} ({} bytes)", MSG.len());
    db.close()?;

    let db = Database::open(&path, DatabaseOptions::default())?;
    let page = db.page_manager().read_page(pid)?;
    let msg = std::str::from_utf8(&page.payload[..MSG.len()])
        .map_err(|e| rdbx::Error::Internal(format!("payload is not UTF-8: {e}")))?;
    println!("Read back: {msg}");
    assert_eq!(msg.as_bytes(), MSG, "roundtrip payload mismatch");
    Ok(())
}
