-- Exact privilege contract for the Canonical quote API runtime login.
--
-- The role is intentionally provisioned outside this repository. This script
-- fails closed when the expected role is missing or has privileged attributes.
-- Run it with the reviewed migration identity after db/schema.sql converges.

\set ON_ERROR_STOP on

BEGIN;

DO $$
DECLARE
    runtime_role pg_roles%ROWTYPE;
BEGIN
    SELECT *
      INTO runtime_role
      FROM pg_roles
     WHERE rolname = 'canonical_api_server';

    IF NOT FOUND THEN
        RAISE EXCEPTION 'required runtime role canonical_api_server does not exist';
    END IF;

    IF runtime_role.rolsuper
       OR runtime_role.rolcreaterole
       OR runtime_role.rolcreatedb
       OR runtime_role.rolreplication
       OR runtime_role.rolbypassrls THEN
        RAISE EXCEPTION 'canonical_api_server has forbidden privileged attributes';
    END IF;
END;
$$;

REVOKE ALL ON TABLE canonical_context FROM PUBLIC, canonical_api_server;
REVOKE ALL ON TABLE canonical_quote FROM PUBLIC, canonical_api_server;
REVOKE ALL ON TABLE canonical_quote_event FROM PUBLIC, canonical_api_server;
REVOKE ALL ON TABLE canonical_model_attempt FROM PUBLIC, canonical_api_server;
REVOKE ALL ON SEQUENCE canonical_quote_event_sequence_id_seq
    FROM PUBLIC, canonical_api_server;

GRANT USAGE ON SCHEMA public TO canonical_api_server;
GRANT SELECT, INSERT, UPDATE ON canonical_context TO canonical_api_server;
GRANT SELECT, INSERT, UPDATE ON canonical_quote TO canonical_api_server;
GRANT SELECT, INSERT ON canonical_quote_event TO canonical_api_server;
GRANT SELECT, INSERT, UPDATE ON canonical_model_attempt TO canonical_api_server;
GRANT USAGE, SELECT ON SEQUENCE canonical_quote_event_sequence_id_seq
    TO canonical_api_server;

COMMIT;
