-- 优化软删级联查询性能：为 parent_id / space_id / deleted_at 补索引

-- 文档树递归查询：parent_id 索引（WHERE d.parent_id = t.id）
CREATE INDEX IF NOT EXISTS idx_documents_parent_id
    ON documents (parent_id)
    WHERE deleted_at IS NULL;

-- 空间级联删除：space_id 索引（WHERE space_id = $1 AND deleted_at IS NULL）
CREATE INDEX IF NOT EXISTS idx_documents_space_id_not_deleted
    ON documents (space_id)
    WHERE deleted_at IS NULL;

COMMENT ON INDEX idx_documents_parent_id IS
'加速文档树递归查询（软删级联 + 恢复祖先链校验），仅索引活跃文档';

COMMENT ON INDEX idx_documents_space_id_not_deleted IS
'加速空间级联删除与空间文档列表查询，仅索引活跃文档';
