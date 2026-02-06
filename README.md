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

## Possible Fixes

### Option 1: Query `sqlite_master` before checking frame count

**Where**: `sync.rs`, in `is_ahead_of_remote()` or at the start of `sync_offline()`.

Execute `SELECT 1 FROM sqlite_master LIMIT 1` on the sync connection before calling `wal_frame_count()`. This forces `walTryBeginRead()` → `walIndexReadHdr()`, refreshing the cached `mxFrame`.

**Pros:**
- Minimal change — one line of SQL
- Works entirely within existing APIs
- No C code changes required

**Cons:**
- Relies on an implicit side effect (query forces WAL header read)
- The intent isn't obvious without a comment explaining why

### Option 2: Expose a WAL header refresh in the FFI layer

**Where**: Add a new C function like `libsql_wal_refresh()` in `wal.c` that calls `walIndexReadHdr()` directly, then call it from `is_ahead_of_remote()`.

**Pros:**
- Precise — does exactly what's needed and nothing more
- No query overhead, no transaction side effects
- Makes the intent explicit in the API

**Cons:**
- Requires changes across 3 layers: C code (`wal.c`), FFI bindings (`libsql-sys`), and Rust wrapper
- `walIndexReadHdr()` is a static function in `wal.c` — you'd need to expose it or write a thin wrapper
- More code to review, more surface area for bugs
- Harder to get merged upstream — touching SQLite internals is sensitive

**Verdict**: The "proper" fix architecturally, but significantly more effort and risk for marginal benefit over Option 1.

## Relevant Source Locations

- **Sync code**: `libsql/libsql/src/sync.rs` (`SyncContext`, `sync_offline`, `try_push`, `try_pull`)
- **Database builder**: `libsql/libsql/src/database/builder.rs` (sync connection opened at ~line 716)
- **WAL C code**: `libsql/libsql-sqlite3/src/wal.c` (`sqlite3WalFrameCount`, `walIndexReadHdr`)
- **Connection**: `libsql/libsql/src/local/connection.rs`

## Running

```
cargo run
```
