-- 20260801000002_init_schema (down)
--
-- 逆依赖顺序删除。CASCADE 不需要：按此顺序时外键已无引用者。

DROP TABLE IF EXISTS refresh_tokens;
DROP TABLE IF EXISTS document_permissions;
DROP TABLE IF EXISTS space_members;
DROP TABLE IF EXISTS document_chunks;
DROP TABLE IF EXISTS documents;
DROP TABLE IF EXISTS media_assets;
DROP TABLE IF EXISTS spaces;
DROP TABLE IF EXISTS users;

DROP FUNCTION IF EXISTS trg_set_updated_at();
DROP FUNCTION IF EXISTS chunk_cells_text(jsonb);
