DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_roles
        WHERE rolname = 'canonical_cloud__quote__migrator'
          AND NOT rolcanlogin
          AND NOT rolsuper
          AND NOT rolcreatedb
          AND NOT rolcreaterole
          AND NOT rolreplication
          AND NOT rolbypassrls
    ) THEN
        RAISE EXCEPTION 'refusing readiness grants because the migrator role is missing or over-privileged';
    END IF;
    IF NOT EXISTS (
        SELECT 1
        FROM pg_roles
        WHERE rolname = 'canonical_cloud__quote__api_rw'
          AND rolcanlogin
          AND NOT rolsuper
          AND NOT rolcreatedb
          AND NOT rolcreaterole
          AND NOT rolreplication
          AND NOT rolbypassrls
    ) THEN
        RAISE EXCEPTION 'refusing readiness grants because the API role is missing or over-privileged';
    END IF;
END;
$$;

ALTER SCHEMA canonical_cloud__readiness
    OWNER TO canonical_cloud__quote__migrator;
ALTER TABLE canonical_cloud__readiness.observation
    OWNER TO canonical_cloud__quote__migrator;

REVOKE ALL ON SCHEMA canonical_cloud__readiness FROM PUBLIC;
REVOKE ALL ON TABLE canonical_cloud__readiness.observation FROM PUBLIC;
REVOKE ALL ON TABLE canonical_cloud__readiness.observation
    FROM canonical_cloud__quote__api_rw;

GRANT USAGE ON SCHEMA canonical_cloud__readiness
    TO canonical_cloud__quote__api_rw;
GRANT SELECT, INSERT ON TABLE canonical_cloud__readiness.observation
    TO canonical_cloud__quote__api_rw;

REVOKE UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER
    ON TABLE canonical_cloud__readiness.observation
    FROM canonical_cloud__quote__api_rw;
