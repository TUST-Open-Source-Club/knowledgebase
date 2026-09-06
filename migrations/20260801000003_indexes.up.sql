-- 20260801000003_indexes
--
-- 每个索引标注支撑的查询模式。刻意不建的索引在文末列出并说明原因。

-- ============================================================
-- users
-- ============================================================

-- 邮箱唯一性只在未删除行之间成立：软删用户 30 天保留期内不占用邮箱槽位。
-- 恢复时若邮箱已被新账号占用，本索引抛 23505，应用层映射为 RestoreConflict。
CREATE UNIQUE INDEX uniq_users_email_alive ON users(email)
    WHERE deleted_at IS NULL;

-- ============================================================
-- spaces
-- ============================================================

CREATE UNIQUE INDEX uniq_spaces_slug_alive ON spaces(slug)
    WHERE deleted_at IS NULL;

-- 列出某用户拥有的空间；也用于用户注销前置校验（是否仍持有活跃空间）。
CREATE INDEX idx_spaces_owner_alive ON spaces(owner_id)
    WHERE deleted_at IS NULL;

-- 定时硬删：找出软删超过 30 天的空间。
CREATE INDEX idx_spaces_deleted_at ON spaces(deleted_at)
    WHERE deleted_at IS NOT NULL;

-- ============================================================
-- documents
-- ============================================================

-- 主力列表查询：某空间下按状态过滤、按 sort_order 排序（目录树 / 草稿箱 / 已发布）。
-- 三列复合 + 部分条件：
--   最左前缀 (space_id) 覆盖"列出空间全部文档"，故不再单建 idx_documents_space_id；
--   (space_id, status) 覆盖状态过滤，故不再单建低基数的 idx_documents_status；
--   尾列 sort_order 让排序走索引顺序，免 sort 节点。
CREATE INDEX idx_documents_space_status_sort
    ON documents(space_id, status, sort_order)
    WHERE deleted_at IS NULL;

CREATE UNIQUE INDEX uniq_documents_space_slug_alive
    ON documents(space_id, slug)
    WHERE deleted_at IS NULL AND slug IS NOT NULL;

-- 目录树：取某节点的直接子文档，按 sort_order 排序。
CREATE INDEX idx_documents_parent_sort ON documents(parent_id, sort_order)
    WHERE deleted_at IS NULL AND parent_id IS NOT NULL;

-- 批次恢复：空间恢复时取"跟着该空间被连带软删"的文档。
CREATE INDEX idx_documents_deleted_batch ON documents(deleted_batch_id)
    WHERE deleted_batch_id IS NOT NULL;

-- 定时硬删：找出软删超过 30 天的文档。
CREATE INDEX idx_documents_deleted_at ON documents(deleted_at)
    WHERE deleted_at IS NOT NULL;

-- 标题全文检索（zhparser 分词），与 chunk 正文检索两路 UNION。
CREATE INDEX idx_documents_title_fts ON documents USING GIN (title_tsv);

-- ============================================================
-- document_chunks
-- ============================================================

-- (document_id, position) 索引由 uniq_chunks_doc_position 唯一约束自带，
-- 已覆盖"取某文档全部分片并按 position 排序"与单列 document_id 查询（最左前缀），
-- 故此处不再建任何 document_id 相关索引。

-- 正文全文检索（zhparser 分词）。
CREATE INDEX idx_chunks_content_fts ON document_chunks USING GIN (content_tsv);

-- ============================================================
-- space_members
-- ============================================================

-- 每空间至多一个 owner。应用层的创建/转移事务负责保证"至少一个"。
CREATE UNIQUE INDEX uniq_space_members_owner ON space_members(space_id)
    WHERE role = 'owner';

-- 反向查询：某用户加入了哪些空间。
-- 正向 (space_id, ...) 已由主键覆盖。
CREATE INDEX idx_space_members_user ON space_members(user_id);

-- ============================================================
-- document_permissions
-- ============================================================

-- 反向查询：某用户被显式授权了哪些文档。
-- 权限判定走 (document_id, user_id)，已由主键覆盖。
CREATE INDEX idx_doc_permissions_user ON document_permissions(user_id);

-- ============================================================
-- media_assets
-- ============================================================

CREATE INDEX idx_media_uploader ON media_assets(uploader_id);

-- 孤儿清理：按上传时间找出候选，以及找出已标记待确删的资源。
CREATE INDEX idx_media_created_at ON media_assets(created_at)
    WHERE deleted_at IS NULL;
CREATE INDEX idx_media_deleted_at ON media_assets(deleted_at)
    WHERE deleted_at IS NOT NULL;

-- ============================================================
-- refresh_tokens
-- ============================================================

-- 列出某用户的在线设备 / 踢下线。
CREATE INDEX idx_refresh_tokens_user ON refresh_tokens(user_id);

-- 定时清理：物理删除已过期或已注销的令牌。
CREATE INDEX idx_refresh_tokens_expires ON refresh_tokens(expires_at);

-- ============================================================
-- 刻意不建的索引
-- ============================================================
--
-- documents(id) WHERE deleted_at IS NULL / spaces(id) WHERE deleted_at IS NULL
--   主键已能单行定位，规划器不会选这类索引，纯写入开销。
--
-- document_chunks USING GIN (content jsonb_path_ops)
--   仅能加速 @> 包含查询，对 tsvector 全文检索无用。
--   孤儿媒体清理需扫 content->asset_id，一期接受全表扫
--   （每日一次、预估 < 10 万行）。表增长到百万行级时再补此索引。
--
-- documents(status) 单列
--   仅 3 个取值，低基数 B-tree 会被规划器跳过，已并入
--   idx_documents_space_status_sort。
