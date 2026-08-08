\set ON_ERROR_STOP on

REVOKE ALL ON SCHEMA canonical_cloud__quote FROM PUBLIC;
REVOKE ALL ON ALL TABLES IN SCHEMA canonical_cloud__quote FROM PUBLIC;
REVOKE ALL ON ALL SEQUENCES IN SCHEMA canonical_cloud__quote FROM PUBLIC;
REVOKE ALL ON ALL FUNCTIONS IN SCHEMA canonical_cloud__quote FROM PUBLIC;

REVOKE ALL ON SCHEMA canonical_cloud__quote
    FROM canonical_cloud__quote__api_rw,
         canonical_cloud__quote__web_ro;
REVOKE ALL ON ALL TABLES IN SCHEMA canonical_cloud__quote
    FROM canonical_cloud__quote__api_rw,
         canonical_cloud__quote__web_ro;
REVOKE ALL ON ALL SEQUENCES IN SCHEMA canonical_cloud__quote
    FROM canonical_cloud__quote__api_rw,
         canonical_cloud__quote__web_ro;
REVOKE ALL ON ALL FUNCTIONS IN SCHEMA canonical_cloud__quote
    FROM canonical_cloud__quote__api_rw,
         canonical_cloud__quote__web_ro;

GRANT USAGE ON SCHEMA canonical_cloud__quote
    TO canonical_cloud__quote__api_rw;

GRANT SELECT, INSERT, UPDATE
    ON TABLE canonical_cloud__quote.canonical_context
    TO canonical_cloud__quote__api_rw;
GRANT SELECT, INSERT, UPDATE
    ON TABLE canonical_cloud__quote.canonical_quote
    TO canonical_cloud__quote__api_rw;
GRANT SELECT, INSERT
    ON TABLE canonical_cloud__quote.canonical_quote_event
    TO canonical_cloud__quote__api_rw;
GRANT SELECT, INSERT, UPDATE
    ON TABLE canonical_cloud__quote.canonical_model_attempt
    TO canonical_cloud__quote__api_rw;

GRANT USAGE, SELECT
    ON SEQUENCE canonical_cloud__quote.canonical_quote_event_sequence_id_seq
    TO canonical_cloud__quote__api_rw;

GRANT EXECUTE
    ON FUNCTION canonical_cloud__quote.canonical_set_updated_at()
    TO canonical_cloud__quote__api_rw;

DO $$
DECLARE
    object_owner_count integer;
BEGIN
    SELECT count(*)
    INTO object_owner_count
    FROM pg_class AS relation
    JOIN pg_namespace AS namespace
      ON namespace.oid = relation.relnamespace
    WHERE namespace.nspname = 'canonical_cloud__quote'
      AND relation.relkind IN ('r', 'p', 'S')
      AND pg_get_userbyid(relation.relowner)
          <> 'canonical_cloud__quote__migrator';

    IF object_owner_count <> 0 THEN
        RAISE EXCEPTION
            'all Canonical quote tables and sequences must be owned by the migrator';
    END IF;

    IF has_schema_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote',
        'CREATE'
    ) THEN
        RAISE EXCEPTION
            'the API role must not create schema objects';
    END IF;

    IF has_schema_privilege(
        'canonical_cloud__quote__web_ro',
        'canonical_cloud__quote',
        'USAGE'
    ) THEN
        RAISE EXCEPTION
            'the web role has no direct Canonical quote database surface';
    END IF;

    IF has_table_privilege(
        'canonical_cloud__quote__web_ro',
        'canonical_cloud__quote.canonical_quote',
        'SELECT'
    ) THEN
        RAISE EXCEPTION
            'the web role must not read quote rows directly';
    END IF;
END;
$$;
