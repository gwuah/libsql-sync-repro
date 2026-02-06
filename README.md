# Stale WAL Bug in libsql's Sync Loop

This program reproduces a bug in libsql where offline writes are never pushed to the remote server.

## The Bug

libsql's sync loop uses a dedicated "sync connection" (opened once, reused forever) to read WAL frames and push them to the remote. Before pushing, it calls `wal_frame_count()` to check if there are local frames ahead of the remote's `durable_frame_num`.

The problem: `wal_frame_count()` reads `pWal->hdr.mxFrame`, a **cached** value inside SQLite's WAL implementation. This cache is only refreshed via `walIndexReadHdr()`, which is called during `walTryBeginRead()` — i.e., when a **read transaction** starts.

Since the sync connection never executes a table-accessing query, it never starts a read transaction. The cached `mxFrame` stays at 0 (its initial value), and `is_ahead_of_remote()` always returns `false`. The sync loop skips pushing entirely.

## Call Chain

```
sync_offline()
  → is_ahead_of_remote()
    → wal_frame_count()           // reads cached pWal->hdr.mxFrame
      → sqlite3WalFrameCount()    // returns stale value (0)
  → 0 > 0 == false
  → skips try_push()
```

## Why Some Queries Don't Fix It

- `SELECT 1` — evaluated as a constant expression; no database pages are read, so no read transaction is started.
- `PRAGMA page_size` — returns a cached config value; also doesn't start a read transaction.

## The Fix

Execute a table-accessing query on the sync connection before checking the frame count:

```sql
SELECT 1 FROM sqlite_master LIMIT 1
```

This forces `walTryBeginRead()` → `walIndexReadHdr()`, which refreshes `pWal->hdr.mxFrame` from shared memory. After that, `wal_frame_count()` returns the correct value.

## Relevant Source Locations

- **Sync code**: `libsql/libsql/src/sync.rs` (`SyncContext`, `sync_offline`, `try_push`, `try_pull`)
- **Database builder**: `libsql/libsql/src/database/builder.rs` (sync connection opened at ~line 716)
- **WAL C code**: `libsql/libsql-sqlite3/src/wal.c` (`sqlite3WalFrameCount`, `walIndexReadHdr`)
- **Connection**: `libsql/libsql/src/local/connection.rs`

## Running

```
cargo run
```

## Expected Output

```
=== Stale WAL Bug Reproduction ===

Writer connection WAL frame count: 4
Sync connection WAL frame count:   0

BUG: sync connection sees 0 frames (stale pWal->hdr.mxFrame)
     while writer has 4 frames in the WAL.

--- Applying fix: SELECT 1 FROM sqlite_master LIMIT 1 ---

Sync connection WAL frame count:   4

FIXED: after reading sqlite_master, sync connection sees all 4 frames.
```
