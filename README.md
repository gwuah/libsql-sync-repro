# Broken replication in libsql

A reproduction for libsql bug where offline writes (on a seperate connection) are never replicated to the remote instance.

## Setup

```rust
let db = Builder::new_synced_database(&path, url, token)
               .sync_interval(sync_interval)
               .build()
               .await?

// you'd expect remote replication to work automatically
// however it only replicates from remote -> local.
```

## The Bug

libsql's sync loop uses a dedicated "sync connection" (opened once, reused forever) to read WAL frames and push them to the remote. Before pushing, it calls `wal_frame_count()` to check if there are local frames ahead of the remote's `durable_frame_num`.

`Turns out wal_frame_count()` reads `pWal->hdr.mxFrame`, a **cached** value inside SQLite's WAL implementation. This cache is only refreshed via `walIndexReadHdr()`, which is called during `walTryBeginRead()` — i.e., when a **read transaction** starts.

Since the sync connection never executes a table-accessing query, it never starts a read transaction. The cached `mxFrame` stays at 0 (its initial value), and `is_ahead_of_remote()` always returns `false`.

As a result, the [sync loop](https://github.com/tursodatabase/libsql/blob/b5dab26b005c51ac8a67a868f8eaa1f9674877a9/libsql/src/sync.rs#L839) never pushes to remote.

## Call Chain

```
sync_offline()
  → is_ahead_of_remote()
    → wal_frame_count()           // reads cached pWal->hdr.mxFrame
      → sqlite3WalFrameCount()    // returns stale value (0)
  → 0 > 0 == false
  → skips try_push()
```

## Relevant Source Locations

- **Sync code**: [libsql/src/sync.rs](https://github.com/tursodatabase/libsql/blob/main/libsql/src/sync.rs)(`SyncContext`, `sync_offline`, `try_push`, `try_pull`)
- **Database builder**: [libsql/src/database/builder.rs](https://github.com/tursodatabase/libsql/blob/main/libsql/src/database/builder.rs) (sync connection opened at ~line 716)
- **WAL C code**: [libsql-sqlite3/src/wal.c](https://github.com/tursodatabase/libsql/blob/main/libsql-sqlite3/src/wal.c) (`sqlite3WalFrameCount`, `walIndexReadHdr`)
- **Connection**: [libsql/src/local/connection.rs](https://github.com/tursodatabase/libsql/blob/main/libsql/src/local/connection.rs)

## Running

```
cargo run
```

