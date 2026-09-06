-- 20260801000003_indexes (down)

DROP INDEX IF EXISTS idx_refresh_tokens_expires;
DROP INDEX IF EXISTS idx_refresh_tokens_user;

DROP INDEX IF EXISTS idx_media_deleted_at;
DROP INDEX IF EXISTS idx_media_created_at;
DROP INDEX IF EXISTS idx_media_uploader;

DROP INDEX IF EXISTS idx_doc_permissions_user;

DROP INDEX IF EXISTS idx_space_members_user;
DROP INDEX IF EXISTS uniq_space_members_owner;

DROP INDEX IF EXISTS idx_chunks_content_fts;

DROP INDEX IF EXISTS idx_documents_title_fts;
DROP INDEX IF EXISTS idx_documents_deleted_at;
DROP INDEX IF EXISTS idx_documents_deleted_batch;
DROP INDEX IF EXISTS idx_documents_parent_sort;
DROP INDEX IF EXISTS uniq_documents_space_slug_alive;
DROP INDEX IF EXISTS idx_documents_space_status_sort;

DROP INDEX IF EXISTS idx_spaces_deleted_at;
DROP INDEX IF EXISTS idx_spaces_owner_alive;
DROP INDEX IF EXISTS uniq_spaces_slug_alive;

DROP INDEX IF EXISTS uniq_users_email_alive;
