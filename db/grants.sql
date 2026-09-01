\set ON_ERROR_STOP on

DO $$
DECLARE
    invalid_roles text;
BEGIN
    SELECT string_agg(rolname, ', ' ORDER BY rolname)
    INTO invalid_roles
    FROM pg_roles
    WHERE rolname IN (
        'canonical_cloud__quote__migrator',
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote__web_ro'
    )
      AND (
          NOT rolcanlogin
          OR rolsuper
          OR rolcreatedb
          OR rolcreaterole
          OR rolinherit
          OR rolreplication
          OR rolbypassrls
      );

    IF invalid_roles IS NOT NULL THEN
        RAISE EXCEPTION
            'refusing grants because Canonical quote roles have forbidden attributes: %',
            invalid_roles;
    END IF;

    IF pg_has_role(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote__migrator',
        'member'
    ) OR pg_has_role(
        'canonical_cloud__quote__web_ro',
        'canonical_cloud__quote__migrator',
        'member'
    ) THEN
        RAISE EXCEPTION
            'runtime roles must not hold migrator membership';
    END IF;
END;
$$;

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
    ON TABLE canonical_cloud__quote.canonical_quote_operation
    TO canonical_cloud__quote__api_rw;
GRANT SELECT, INSERT
    ON TABLE canonical_cloud__quote.canonical_quote_event
    TO canonical_cloud__quote__api_rw;
GRANT SELECT, INSERT, UPDATE
    ON TABLE canonical_cloud__quote.canonical_model_attempt
    TO canonical_cloud__quote__api_rw;
GRANT SELECT, INSERT, UPDATE
    ON TABLE canonical_cloud__quote.canonical_pre_interest_registration
    TO canonical_cloud__quote__api_rw;
GRANT SELECT, INSERT
    ON TABLE canonical_cloud__quote.canonical_pre_interest_consent
    TO canonical_cloud__quote__api_rw;
GRANT SELECT, INSERT, UPDATE
    ON TABLE canonical_cloud__quote.canonical_pre_interest_outbox
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
    ) OR has_schema_privilege(
        'canonical_cloud__quote__api_rw',
        'public',
        'CREATE'
    ) THEN
        RAISE EXCEPTION
            'the API role must not create schema objects';
    END IF;

    IF NOT has_schema_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote',
        'USAGE'
    ) THEN
        RAISE EXCEPTION
            'the API role must have namespace usage';
    END IF;

    IF NOT has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_context',
        'SELECT'
    ) OR NOT has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_context',
        'INSERT'
    ) OR NOT has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_context',
        'UPDATE'
    ) OR has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_context',
        'DELETE'
    ) OR has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_context',
        'TRUNCATE'
    ) THEN
        RAISE EXCEPTION
            'canonical_context API privilege contract is not exact';
    END IF;

    IF NOT has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_quote',
        'SELECT'
    ) OR NOT has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_quote',
        'INSERT'
    ) OR NOT has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_quote',
        'UPDATE'
    ) OR has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_quote',
        'DELETE'
    ) OR has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_quote',
        'TRUNCATE'
    ) THEN
        RAISE EXCEPTION
            'canonical_quote API privilege contract is not exact';
    END IF;

    IF NOT has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_quote_operation',
        'SELECT'
    ) OR NOT has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_quote_operation',
        'INSERT'
    ) OR has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_quote_operation',
        'UPDATE'
    ) OR has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_quote_operation',
        'DELETE'
    ) OR has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_quote_operation',
        'TRUNCATE'
    ) THEN
        RAISE EXCEPTION
            'canonical_quote_operation API privilege contract is append-only';
    END IF;

    IF NOT has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_quote_event',
        'SELECT'
    ) OR NOT has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_quote_event',
        'INSERT'
    ) OR has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_quote_event',
        'UPDATE'
    ) OR has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_quote_event',
        'DELETE'
    ) OR has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_quote_event',
        'TRUNCATE'
    ) THEN
        RAISE EXCEPTION
            'canonical_quote_event API privilege contract is not append-only';
    END IF;

    IF NOT has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_model_attempt',
        'SELECT'
    ) OR NOT has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_model_attempt',
        'INSERT'
    ) OR NOT has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_model_attempt',
        'UPDATE'
    ) OR has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_model_attempt',
        'DELETE'
    ) OR has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_model_attempt',
        'TRUNCATE'
    ) THEN
        RAISE EXCEPTION
            'canonical_model_attempt API privilege contract is not exact';
    END IF;

    IF NOT has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_pre_interest_registration',
        'SELECT'
    ) OR NOT has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_pre_interest_registration',
        'INSERT'
    ) OR NOT has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_pre_interest_registration',
        'UPDATE'
    ) OR has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_pre_interest_registration',
        'DELETE'
    ) OR has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_pre_interest_registration',
        'TRUNCATE'
    ) THEN
        RAISE EXCEPTION
            'canonical_pre_interest_registration API privilege contract is not exact';
    END IF;

    IF NOT has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_pre_interest_consent',
        'SELECT'
    ) OR NOT has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_pre_interest_consent',
        'INSERT'
    ) OR has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_pre_interest_consent',
        'UPDATE'
    ) OR has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_pre_interest_consent',
        'DELETE'
    ) OR has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_pre_interest_consent',
        'TRUNCATE'
    ) THEN
        RAISE EXCEPTION
            'canonical_pre_interest_consent API privilege contract is append-only';
    END IF;

    IF NOT has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_pre_interest_outbox',
        'SELECT'
    ) OR NOT has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_pre_interest_outbox',
        'INSERT'
    ) OR NOT has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_pre_interest_outbox',
        'UPDATE'
    ) OR has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_pre_interest_outbox',
        'DELETE'
    ) OR has_table_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_pre_interest_outbox',
        'TRUNCATE'
    ) THEN
        RAISE EXCEPTION
            'canonical_pre_interest_outbox API privilege contract is not exact';
    END IF;

    IF NOT has_sequence_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_quote_event_sequence_id_seq',
        'USAGE'
    ) OR NOT has_function_privilege(
        'canonical_cloud__quote__api_rw',
        'canonical_cloud__quote.canonical_set_updated_at()',
        'EXECUTE'
    ) THEN
        RAISE EXCEPTION
            'the API role is missing required sequence or function privileges';
    END IF;

    IF has_schema_privilege(
        'canonical_cloud__quote__web_ro',
        'canonical_cloud__quote',
        'USAGE'
    ) OR has_table_privilege(
        'canonical_cloud__quote__web_ro',
        'canonical_cloud__quote.canonical_quote',
        'SELECT'
    ) OR has_table_privilege(
        'canonical_cloud__quote__web_ro',
        'canonical_cloud__quote.canonical_quote_operation',
        'SELECT'
    ) OR has_table_privilege(
        'canonical_cloud__quote__web_ro',
        'canonical_cloud__quote.canonical_pre_interest_registration',
        'SELECT'
    ) OR has_table_privilege(
        'canonical_cloud__quote__web_ro',
        'canonical_cloud__quote.canonical_pre_interest_consent',
        'SELECT'
    ) OR has_table_privilege(
        'canonical_cloud__quote__web_ro',
        'canonical_cloud__quote.canonical_pre_interest_outbox',
        'SELECT'
    ) THEN
        RAISE EXCEPTION
            'the web role has a forbidden direct Canonical database surface';
    END IF;
END;
$$;
