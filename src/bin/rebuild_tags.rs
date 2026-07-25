use nostr_rs_relay::config;
use nostr_rs_relay::error::{Error, Result};
use nostr_rs_relay::event::{single_char_tagname, Event};
use nostr_rs_relay::repo::sqlite::{build_pool, PooledConnection};
use nostr_rs_relay::repo::sqlite_migration::{curr_db_version, DB_VERSION};
use rusqlite::params;
use rusqlite::OpenFlags;
use std::path::Path;
use tracing::info;

/// Rebuild the tag table from the canonical event JSON in the database
/// specified in config.toml (or ./nostr.db as a default).
///
/// Tag rows are derived data: they exist only to index the tags inside
/// each event's stored JSON.  Any drift — orphaned rows left behind by a
/// writer that did not enforce foreign keys, or rows attached to the
/// wrong event — is repaired by deleting every row and re-deriving the
/// index exactly as the event writer (`persist_event`) would have.
///
/// Run offline: stop the relay, run this tool against the same config,
/// then restart the relay.  The rebuild is idempotent.
pub fn main() -> Result<()> {
    let _trace_sub = tracing_subscriber::fmt::try_init();
    println!("Nostr-rs-relay Tag Index Rebuilder");
    let settings = config::Settings::new(&None)?;
    if !Path::new(&settings.database.data_directory).is_dir() {
        info!("Database directory does not exist");
        return Err(Error::DatabaseDirError);
    }
    let pool = build_pool(
        "tag-rebuilder",
        &settings,
        OpenFlags::SQLITE_OPEN_READ_WRITE,
        1,
        2,
        false,
    );
    let mut conn: PooledConnection = pool.get()?;
    // ensure the schema version is current; the insert below must match
    // what persist_event writes for this schema.
    let version = curr_db_version(&mut conn)?;
    info!("current version is: {:?}", version);
    if version != DB_VERSION {
        panic!("cannot rebuild tags for schema other than v{DB_VERSION}");
    }
    let tx = conn.transaction()?;
    {
        let deleted = tx.execute("DELETE FROM tag;", [])?;
        info!("deleted {} existing tag rows", deleted);
        let mut stmt =
            tx.prepare("SELECT id, kind, created_at, content FROM event ORDER BY id;")?;
        let mut rows = stmt.query([])?;
        let mut events: u64 = 0;
        let mut tags: u64 = 0;
        while let Some(row) = rows.next()? {
            events += 1;
            if events.is_multiple_of(10_000) {
                info!("processed {} events...", events);
            }
            let event_id: u64 = row.get(0)?;
            let kind: u64 = row.get(1)?;
            let created_at: u64 = row.get(2)?;
            let event_json: String = row.get(3)?;
            let event: Event = serde_json::from_str(&event_json)?;
            // look at each event, and each tag, creating new tag entries if appropriate.
            for t in event.tags.iter().filter(|x| x.len() > 1) {
                let tagname = t.first().unwrap();
                let tagnamechar_opt = single_char_tagname(tagname);
                if tagnamechar_opt.is_none() {
                    continue;
                }
                // safe because len was > 1
                let tagval = t.get(1).unwrap();
                // matches persist_event: single-char tag names only,
                // text value column (value_hex is not written on this
                // schema), INSERT OR IGNORE for duplicate tags.
                tags += tx.execute(
                    "INSERT OR IGNORE INTO tag (event_id, name, value, kind, created_at) VALUES (?1, ?2, ?3, ?4, ?5);",
                    params![event_id, tagname, &tagval, kind, created_at],
                )? as u64;
            }
        }
        info!("re-derived {} tag rows from {} events", tags, events);
    }
    tx.commit()?;
    conn.execute_batch("pragma wal_checkpoint(truncate)")?;
    info!("tag index rebuild complete");
    Ok(())
}
