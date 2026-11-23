use rusqlite::Connection;

/// Turn a current fixture back into a schema without the v16 sync tables and triggers.
pub fn remove_replica_schema(connection: &Connection) {
    connection
        .execute_batch(
            "DROP TRIGGER replica_clips_INSERT;
         DROP TRIGGER replica_clips_UPDATE;
         DROP TRIGGER replica_clips_DELETE;
         DROP TRIGGER replica_collections_INSERT;
         DROP TRIGGER replica_collections_UPDATE;
         DROP TRIGGER replica_collections_DELETE;
         DROP INDEX clips_replica_order;
         DROP INDEX clips_replica_quick_order;
         DROP TABLE replica_grant_refs;
         DROP TABLE replica_grants;
         DROP TABLE replica_files;
         DROP TABLE replica_names;
         DROP TABLE replica_entities;
         DROP TABLE replica_heads;
         DROP TABLE replica_state;",
        )
        .unwrap();
}
