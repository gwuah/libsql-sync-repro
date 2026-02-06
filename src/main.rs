//! Reproduces the stale WAL bug in libsql's sync loop.
//!
//! The bug: the sync connection (created once, reused forever) calls
//! wal_frame_count() to decide whether to push. But wal_frame_count()
//! reads pWal->hdr.mxFrame — a cached value only refreshed when a read
//! transaction starts (walIndexReadHdr). Since the sync connection never
//! runs a query, the cached value is stale and sync never pushes.

use std::ffi::{c_uint, CString};
use std::ptr;

fn main() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let c_path = CString::new(db_path.to_str().unwrap()).unwrap();

    unsafe {
        // --- Open two connections (writer + sync) ---
        let mut writer: *mut libsql::ffi::sqlite3 = ptr::null_mut();
        let mut sync_conn: *mut libsql::ffi::sqlite3 = ptr::null_mut();

        assert_eq!(
            libsql::ffi::sqlite3_open_v2(
                c_path.as_ptr(),
                &mut writer,
                libsql::ffi::SQLITE_OPEN_READWRITE | libsql::ffi::SQLITE_OPEN_CREATE,
                ptr::null(),
            ),
            libsql::ffi::SQLITE_OK,
        );

        // Enable WAL mode and disable auto-checkpoint (mirrors sync builder)
        exec(writer, "PRAGMA journal_mode = WAL");
        exec(writer, "PRAGMA wal_autocheckpoint = 0");

        // Write some data — this creates WAL frames
        exec(writer, "CREATE TABLE t(x INTEGER)");
        exec(writer, "INSERT INTO t VALUES (1)");
        exec(writer, "INSERT INTO t VALUES (2)");

        // Now open the sync connection (simulates builder.rs:716)
        assert_eq!(
            libsql::ffi::sqlite3_open_v2(
                c_path.as_ptr(),
                &mut sync_conn,
                libsql::ffi::SQLITE_OPEN_READWRITE,
                ptr::null(),
            ),
            libsql::ffi::SQLITE_OK,
        );

        // --- Demonstrate the bug ---
        let writer_frames = wal_frame_count(writer);
        let sync_frames = wal_frame_count(sync_conn);

        println!("=== Stale WAL Bug Reproduction ===\n");
        println!("Writer connection WAL frame count: {}", writer_frames);
        println!("Sync connection WAL frame count:   {}", sync_frames);
        println!();

        if sync_frames == 0 && writer_frames > 0 {
            println!("BUG: sync connection sees 0 frames (stale pWal->hdr.mxFrame)");
            println!(
                "     while writer has {} frames in the WAL.",
                writer_frames
            );
            println!();
            println!(
                "     is_ahead_of_remote() compares wal_frame_count() > durable_frame_num."
            );
            println!("     With stale frame count = 0 and durable_frame_num = 0,");
            println!("     it returns false — sync_offline() skips pushing entirely.");
        }

        // --- Demonstrate the fix ---
        println!();
        println!("--- Applying fix: SELECT 1 FROM sqlite_master LIMIT 1 ---\n");
        exec(sync_conn, "SELECT 1 FROM sqlite_master LIMIT 1");

        let sync_frames_after = wal_frame_count(sync_conn);
        println!("Sync connection WAL frame count:   {}", sync_frames_after);
        println!();

        if sync_frames_after == writer_frames {
            println!(
                "FIXED: after reading sqlite_master, sync connection sees all {} frames.",
                writer_frames
            );
            println!("       is_ahead_of_remote() now correctly returns true.");
            println!("       sync_offline() will call try_push() as expected.");
        }

        libsql::ffi::sqlite3_close(writer);
        libsql::ffi::sqlite3_close(sync_conn);
    }
}

unsafe fn exec(db: *mut libsql::ffi::sqlite3, sql: &str) {
    let c_sql = CString::new(sql).unwrap();
    let rc = libsql::ffi::sqlite3_exec(db, c_sql.as_ptr(), None, ptr::null_mut(), ptr::null_mut());
    assert_eq!(rc, libsql::ffi::SQLITE_OK, "exec failed for: {}", sql);
}

unsafe fn wal_frame_count(db: *mut libsql::ffi::sqlite3) -> u32 {
    let mut count: c_uint = 0;
    let rc = libsql::ffi::libsql_wal_frame_count(db, &mut count);
    if rc != libsql::ffi::SQLITE_OK as i32 {
        return 0;
    }
    count
}
