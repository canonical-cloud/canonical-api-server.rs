\set ON_ERROR_STOP on

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_roles
        WHERE rolname = 'canonical_cloud__quote__migrator'
    ) THEN
        CREATE ROLE canonical_cloud__quote__migrator
            LOGIN
            NOSUPERUSER
            NOCREATEDB
            NOCREATEROLE
            NOINHERIT
            NOREPLICATION
            NOBYPASSRLS;
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_roles
        WHERE rolname = 'canonical_cloud__quote__api_rw'
    ) THEN
        CREATE ROLE canonical_cloud__quote__api_rw
            LOGIN
            NOSUPERUSER
            NOCREATEDB
            NOCREATEROLE
            NOINHERIT
            NOREPLICATION
            NOBYPASSRLS;
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_roles
        WHERE rolname = 'canonical_cloud__quote__web_ro'
    ) THEN
        CREATE ROLE canonical_cloud__quote__web_ro
            LOGIN
            NOSUPERUSER
            NOCREATEDB
            NOCREATEROLE
            NOINHERIT
            NOREPLICATION
            NOBYPASSRLS;
    END IF;
END;
$$;

DO $$
DECLARE
    database_name text := current_database();
BEGIN
    EXECUTE format(
        'GRANT CONNECT ON DATABASE %I TO canonical_cloud__quote__migrator',
        database_name
    );
    EXECUTE format(
        'GRANT CONNECT ON DATABASE %I TO canonical_cloud__quote__api_rw',
        database_name
    );
    EXECUTE format(
        'GRANT CONNECT ON DATABASE %I TO canonical_cloud__quote__web_ro',
        database_name
    );

    EXECUTE format(
        'ALTER ROLE canonical_cloud__quote__migrator IN DATABASE %I '
        'SET search_path = pg_catalog, canonical_cloud__quote',
        database_name
    );
    EXECUTE format(
        'ALTER ROLE canonical_cloud__quote__api_rw IN DATABASE %I '
        'SET search_path = pg_catalog, canonical_cloud__quote',
        database_name
    );
    EXECUTE format(
        'ALTER ROLE canonical_cloud__quote__web_ro IN DATABASE %I '
        'SET search_path = pg_catalog, canonical_cloud__quote',
        database_name
    );

    EXECUTE format(
        'ALTER ROLE canonical_cloud__quote__migrator IN DATABASE %I '
        'SET statement_timeout = ''5min''',
        database_name
    );
    EXECUTE format(
        'ALTER ROLE canonical_cloud__quote__migrator IN DATABASE %I '
        'SET lock_timeout = ''15s''',
        database_name
    );
    EXECUTE format(
        'ALTER ROLE canonical_cloud__quote__migrator IN DATABASE %I '
        'SET idle_in_transaction_session_timeout = ''60s''',
        database_name
    );

    EXECUTE format(
        'ALTER ROLE canonical_cloud__quote__api_rw IN DATABASE %I '
        'SET statement_timeout = ''30s''',
        database_name
    );
    EXECUTE format(
        'ALTER ROLE canonical_cloud__quote__api_rw IN DATABASE %I '
        'SET lock_timeout = ''5s''',
        database_name
    );
    EXECUTE format(
        'ALTER ROLE canonical_cloud__quote__api_rw IN DATABASE %I '
        'SET idle_in_transaction_session_timeout = ''30s''',
        database_name
    );

    EXECUTE format(
        'ALTER ROLE canonical_cloud__quote__web_ro IN DATABASE %I '
        'SET default_transaction_read_only = on',
        database_name
    );
    EXECUTE format(
        'ALTER ROLE canonical_cloud__quote__web_ro IN DATABASE %I '
        'SET statement_timeout = ''10s''',
        database_name
    );
    EXECUTE format(
        'ALTER ROLE canonical_cloud__quote__web_ro IN DATABASE %I '
        'SET lock_timeout = ''2s''',
        database_name
    );
    EXECUTE format(
        'ALTER ROLE canonical_cloud__quote__web_ro IN DATABASE %I '
        'SET idle_in_transaction_session_timeout = ''15s''',
        database_name
    );
END;
$$;

REVOKE CREATE ON SCHEMA public FROM PUBLIC;
REVOKE ALL ON SCHEMA public
    FROM canonical_cloud__quote__migrator,
         canonical_cloud__quote__api_rw,
         canonical_cloud__quote__web_ro;

CREATE SCHEMA IF NOT EXISTS canonical_cloud__quote
    AUTHORIZATION canonical_cloud__quote__migrator;

REVOKE ALL ON SCHEMA canonical_cloud__quote FROM PUBLIC;
GRANT USAGE, CREATE ON SCHEMA canonical_cloud__quote
    TO canonical_cloud__quote__migrator;

DO $$
DECLARE
    schema_owner text;
BEGIN
    SELECT pg_get_userbyid(nspowner)
    INTO schema_owner
    FROM pg_namespace
    WHERE nspname = 'canonical_cloud__quote';

    IF schema_owner IS DISTINCT FROM 'canonical_cloud__quote__migrator' THEN
        RAISE EXCEPTION
            'canonical_cloud__quote must be owned by canonical_cloud__quote__migrator';
    END IF;

    IF has_schema_privilege(
        'canonical_cloud__quote__api_rw',
        'public',
        'CREATE'
    ) THEN
        RAISE EXCEPTION
            'canonical_cloud__quote__api_rw must not create objects in public';
    END IF;

    IF has_schema_privilege(
        'canonical_cloud__quote__web_ro',
        'public',
        'CREATE'
    ) THEN
        RAISE EXCEPTION
            'canonical_cloud__quote__web_ro must not create objects in public';
    END IF;
END;
$$;
