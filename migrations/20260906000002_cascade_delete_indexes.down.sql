-- 回滚软删级联性能优化索引
DROP INDEX IF EXISTS idx_documents_parent_id;
DROP INDEX IF EXISTS idx_documents_space_id_not_deleted;
