-- 20260801000001_extensions (down)

DO $$
BEGIN
    EXECUTE format(
        'ALTER DATABASE %I RESET zhparser.multi_short',
        current_database()
    );
END
$$;

DROP TEXT SEARCH CONFIGURATION IF EXISTS zhparser_cfg;

DROP EXTENSION IF EXISTS zhparser;
