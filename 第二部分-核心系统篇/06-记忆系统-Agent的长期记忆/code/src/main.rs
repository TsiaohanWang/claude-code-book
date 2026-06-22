//! # 第六章：记忆系统 —— Claude Code 的持久化记忆
//!
//! 本模块演示 Claude Code 的记忆系统架构，忠实映射书中第 6 章内容。
//!
//! 对应 claude-code-book 第 6 章（记忆系统 —— 跨越会话的智慧）。
//!
//! 核心概念：
//! - 四种记忆类型：user / feedback / project / reference
//! - YAML frontmatter：每条记忆的元数据格式
//! - MEMORY.md 索引：记忆文件的索引和组织
//! - BM25 搜索：基于相关度的记忆检索
//!
//! Claude Code 的记忆系统位于：
//!   src/memory/: 记忆管理核心
//!   ~/.claude/projects/<slug>/memory/: 记忆文件存储
//!
//! 运行方式：
//! ```bash
//! cargo run -p ch06-memory
//! ```

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// 第一部分：记忆类型定义
//
// 对应 Claude Code 的四种记忆类型。
//
// Claude Code 将记忆分为四种类型：
//   - user: 用户级偏好（跨项目持久）
//   - feedback: 用户反馈（纠错、补充）
//   - project: 项目级知识（架构决策、规则）
//   - reference: 参考资料（API 文档、代码片段）
//
// 每种类型存储在不同的目录中，使用不同的检索策略。
// ============================================================================

/// 记忆类型
///
/// 对应 Claude Code 的记忆类型分类。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
enum MemoryType {
    /// 用户级偏好 —— 跨项目持久
    User,
    /// 用户反馈 —— 纠错、补充、偏好调整
    Feedback,
    /// 项目级知识 —— 架构决策、规则、上下文
    Project,
    /// 参考资料 —— API 文档、代码片段、最佳实践
    Reference,
}

impl MemoryType {
    fn as_str(&self) -> &str {
        match self {
            Self::User => "user",
            Self::Feedback => "feedback",
            Self::Project => "project",
            Self::Reference => "reference",
        }
    }

    fn description(&self) -> &str {
        match self {
            Self::User => "用户级偏好（跨项目持久）",
            Self::Feedback => "用户反馈（纠错、补充）",
            Self::Project => "项目级知识（架构决策、规则）",
            Self::Reference => "参考资料（API 文档、代码片段）",
        }
    }

#[allow(dead_code)]
    fn storage_path(&self, project_slug: &str) -> String {
        match self {
            Self::User => "~/.claude/memory/user/".to_string(),
            Self::Feedback => format!("~/.claude/projects/{}/memory/feedback/", project_slug),
            Self::Project => format!("~/.claude/projects/{}/memory/project/", project_slug),
            Self::Reference => format!("~/.claude/projects/{}/memory/reference/", project_slug),
        }
    }
}

// ============================================================================
// 第二部分：YAML Frontmatter 记忆条目
//
// 对应 Claude Code 的记忆文件格式。
//
// 每条记忆是一个 markdown 文件，带有 YAML frontmatter：
//   ---
//   type: project
//   created: 2025-01-15T10:30:00Z
//   updated: 2025-01-15T10:30:00Z
//   tags: [architecture, rust]
//   ---
//   # 记忆标题
//   记忆内容...
//
// YAML frontmatter 包含：
//   - type: 记忆类型
//   - created: 创建时间
//   - updated: 最后更新时间
//   - tags: 标签列表（用于检索）
// ============================================================================

/// 记忆条目
///
/// 对应 Claude Code 的记忆文件格式。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemoryEntry {
    /// 记忆类型
    #[serde(rename = "type")]
    memory_type: MemoryType,
    /// 创建时间
    created: String,
    /// 最后更新时间
    updated: String,
    /// 标签列表
    tags: Vec<String>,
    /// 记忆标题
    title: String,
    /// 记忆内容
    content: String,
}

impl MemoryEntry {
    fn new(memory_type: MemoryType, title: String, content: String, tags: Vec<String>) -> Self {
        let now = "2025-01-15T10:30:00Z".to_string();
        Self {
            memory_type,
            created: now.clone(),
            updated: now,
            tags,
            title,
            content,
        }
    }

    /// 序列化为 markdown 格式（带 YAML frontmatter）
    fn to_markdown(&self) -> String {
        let tags_str = self
            .tags
            .iter()
            .map(|t| format!("[{t}]"))
            .collect::<Vec<_>>()
            .join(", ");

        format!(
            "---\ntype: {}\ncreated: {}\nupdated: {}\ntags: {}\n---\n\n# {}\n\n{}",
            self.memory_type.as_str(),
            self.created,
            self.updated,
            tags_str,
            self.title,
            self.content
        )
    }
}

// ============================================================================
// 第三部分：记忆存储 —— MEMORY.md 索引
//
// 对应 Claude Code 的记忆文件系统。
//
// Claude Code 使用文件系统存储记忆：
//   ~/.claude/projects/<slug>/memory/
//     ├── feedback/
//     │   ├── memory_001.md
//     │   └── memory_002.md
//     ├── project/
//     │   ├── MEMORY.md  (索引文件)
//     │   ├── decisions.md
//     │   └── rules.md
//     └── reference/
//         └── api_notes.md
//
// MEMORY.md 是索引文件，列出所有记忆条目的摘要。
// ============================================================================

/// 记忆存储
///
/// 模拟 Claude Code 的文件系统记忆存储。
struct MemoryStore {
    /// 按类型组织的记忆条目
    entries: HashMap<MemoryType, Vec<MemoryEntry>>,
    /// 项目标识
    project_slug: String,
}

impl MemoryStore {
    fn new(project_slug: &str) -> Self {
        Self {
            entries: HashMap::new(),
            project_slug: project_slug.to_string(),
        }
    }

    /// 添加记忆条目
    fn add(&mut self, entry: MemoryEntry) {
        self.entries
            .entry(entry.memory_type.clone())
            .or_insert_with(Vec::new)
            .push(entry);
    }

    /// 获取指定类型的所有记忆
    fn get_by_type(&self, memory_type: &MemoryType) -> Vec<&MemoryEntry> {
        self.entries.get(memory_type).map(|v| v.iter().collect()).unwrap_or_default()
    }

    /// 按标签搜索记忆
    fn search_by_tag(&self, tag: &str) -> Vec<&MemoryEntry> {
        self.entries
            .values()
            .flat_map(|v| v.iter())
            .filter(|e| e.tags.iter().any(|t| t == tag))
            .collect()
    }

    /// 生成 MEMORY.md 索引
    fn generate_index(&self) -> String {
        let mut index = String::new();
        index.push_str("# Memory Index\n\n");
        index.push_str(&format!("Project: {}\n\n", self.project_slug));

        for memory_type in &[
            MemoryType::User,
            MemoryType::Feedback,
            MemoryType::Project,
            MemoryType::Reference,
        ] {
            let entries = self.get_by_type(memory_type);
            if entries.is_empty() {
                continue;
            }
            index.push_str(&format!("## {} ({})\n\n", memory_type.as_str(), memory_type.description()));
            for entry in &entries {
                let tags_str = entry.tags.join(", ");
                index.push_str(&format!(
                    "- **{}** [{}] - {}\n",
                    entry.title, tags_str, entry.created
                ));
            }
            index.push('\n');
        }

        index
    }

    /// 总记忆条目数
#[allow(dead_code)]
    fn total_count(&self) -> usize {
        self.entries.values().map(|v| v.len()).sum()
    }
}

// ============================================================================
// 第四部分：BM25 搜索 —— 记忆检索
//
// 对应 Claude Code 的记忆检索机制。
//
// Claude Code 使用 BM25 算法搜索记忆：
//   - 对记忆的 title + content + tags 进行分词
//   - 使用 BM25 公式计算相关度分数
//   - 返回分数最高的 top-k 结果
//
// BM25 公式简化版：
//   score(q, d) = Σ IDF(qi) * (f(qi, d) * (k1 + 1)) / (f(qi, d) + k1)
//   其中 IDF(qi) = log((N - n(qi) + 0.5) / (n(qi) + 0.5) + 1)
// ============================================================================

/// 简化的 BM25 搜索引擎
///
/// 对应 Claude Code 的 BM25 检索实现。
struct Bm25Search {
    /// k1 参数（词频饱和度）
    k1: f64,
    /// b 参数（文档长度归一化）
    b: f64,
}

impl Bm25Search {
    fn new() -> Self {
        Self { k1: 1.5, b: 0.75 }
    }

    /// 对查询和文档计算 BM25 分数
#[allow(dead_code)]
    fn score(&self, query_tokens: &[String], doc_tokens: &[String], avg_doc_len: f64, total_docs: usize, docs_with_term: usize) -> f64 {
        let doc_len = doc_tokens.len() as f64;
        let mut score = 0.0;

        for q_token in query_tokens {
            let tf = doc_tokens.iter().filter(|t| *t == q_token).count() as f64;
            if tf == 0.0 {
                continue;
            }

            // IDF
            let idf = ((total_docs as f64 - docs_with_term as f64 + 0.5)
                / (docs_with_term as f64 + 0.5)
                + 1.0)
                .ln();

            // TF 归一化
            let tf_norm = (tf * (self.k1 + 1.0)) / (tf + self.k1 * (1.0 - self.b + self.b * doc_len / avg_doc_len));

            score += idf * tf_norm;
        }

        score
    }

    /// 搜索记忆条目
    fn search<'a>(&self, query: &str, entries: &[&'a MemoryEntry], top_k: usize) -> Vec<(&'a MemoryEntry, f64)> {
        let query_tokens = tokenize(query);
        if query_tokens.is_empty() {
            return Vec::new();
        }

        // 构建文档 token 列表
        let doc_tokens_list: Vec<Vec<String>> = entries
            .iter()
            .map(|e| {
                let mut tokens = tokenize(&e.title);
                tokens.extend(tokenize(&e.content));
                tokens.extend(e.tags.iter().flat_map(|t| tokenize(t)));
                tokens
            })
            .collect();

        let avg_doc_len = doc_tokens_list.iter().map(|t| t.len()).sum::<usize>() as f64
            / doc_tokens_list.len().max(1) as f64;
        let total_docs = entries.len();

        // 计算每个文档的分数
        let mut scored: Vec<(&MemoryEntry, f64)> = entries
            .iter()
            .zip(doc_tokens_list.iter())
            .map(|(entry, doc_tokens)| {
                // 对每个查询词，计算有多少文档包含它
                let mut score = 0.0;
                for q_token in &query_tokens {
                    let docs_with_term = doc_tokens_list.iter().filter(|d| d.contains(q_token)).count();
                    let idf = ((total_docs as f64 - docs_with_term as f64 + 0.5)
                        / (docs_with_term as f64 + 0.5)
                        + 1.0)
                        .ln();
                    let tf = doc_tokens.iter().filter(|t| *t == q_token).count() as f64;
                    if tf > 0.0 {
                        let tf_norm = (tf * (self.k1 + 1.0))
                            / (tf + self.k1 * (1.0 - self.b + self.b * doc_tokens.len() as f64 / avg_doc_len));
                        score += idf * tf_norm;
                    }
                }
                (*entry, score)
            })
            .collect();

        // 按分数降序排序
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        scored
    }
}

/// 简化的分词器
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|s| !s.is_empty() && s.len() > 1)
        .map(|s| s.to_string())
        .collect()
}

// ============================================================================
// 第五部分：Mock 演示 —— 完整记忆系统流程
// ============================================================================

/// 创建演示用的记忆数据
fn create_demo_memories() -> MemoryStore {
    let mut store = MemoryStore::new("my-project");

    // 用户偏好
    store.add(MemoryEntry::new(
        MemoryType::User,
        "编码风格偏好".to_string(),
        "用户偏好使用 Rust 的 idiomatic 风格，避免 unwrap，使用 anyhow 处理错误。代码注释使用中文。".to_string(),
        vec!["rust".to_string(), "style".to_string(), "coding".to_string()],
    ));

    store.add(MemoryEntry::new(
        MemoryType::User,
        "终端偏好".to_string(),
        "用户使用 zsh + starship prompt，偏好深色主题。".to_string(),
        vec!["terminal".to_string(), "shell".to_string()],
    ));

    // 用户反馈
    store.add(MemoryEntry::new(
        MemoryType::Feedback,
        "不要自动 commit".to_string(),
        "用户明确要求：不要在修改代码后自动 commit，除非用户明确请求。".to_string(),
        vec!["git".to_string(), "workflow".to_string(), "important".to_string()],
    ));

    // 项目知识
    store.add(MemoryEntry::new(
        MemoryType::Project,
        "项目架构决策".to_string(),
        "本项目使用 workspace 模式管理 Rust crates。common crate 提供共享类型，各章节 crate 依赖 common。".to_string(),
        vec!["architecture".to_string(), "rust".to_string(), "workspace".to_string()],
    ));

    store.add(MemoryEntry::new(
        MemoryType::Project,
        "测试策略".to_string(),
        "每个章节必须包含单元测试和集成测试。Mock 模式确保无需 API Key 即可运行。使用 cargo test 运行。".to_string(),
        vec!["testing".to_string(), "mock".to_string(), "strategy".to_string()],
    ));

    // 参考资料
    store.add(MemoryEntry::new(
        MemoryType::Reference,
        "Anthropic API 端点".to_string(),
        "Messages API: POST /v1/messages\nHeaders: x-api-key, anthropic-version: 2023-06-01\nBody: { model, messages, max_tokens, tools }".to_string(),
        vec!["api".to_string(), "anthropic".to_string(), "reference".to_string()],
    ));

    store.add(MemoryEntry::new(
        MemoryType::Reference,
        "Cargo workspace 配置".to_string(),
        "workspace Cargo.toml 中定义 [workspace.dependencies]，各 crate 使用 { workspace = true } 引用。".to_string(),
        vec!["cargo".to_string(), "workspace".to_string(), "reference".to_string()],
    ));

    store
}

/// 演示记忆系统的完整流程
fn demo_memory_system() {
    println!("=== 记忆系统演示 ===");
    println!();

    let store = create_demo_memories();

    // 按类型展示
    println!("--- 记忆条目（按类型）---");
    for memory_type in &[
        MemoryType::User,
        MemoryType::Feedback,
        MemoryType::Project,
        MemoryType::Reference,
    ] {
        let entries = store.get_by_type(memory_type);
        println!("\n[{}] {} ({} 条)", memory_type.as_str(), memory_type.description(), entries.len());
        for entry in &entries {
            println!("  - {}: {}", entry.title, truncate(&entry.content, 60));
        }
    }
    println!();

    // 按标签搜索
    println!("--- 按标签搜索 ---");
    for tag in &["rust", "api", "git", "testing"] {
        let results = store.search_by_tag(tag);
        println!("  标签 '{}': {} 条结果", tag, results.len());
        for entry in &results {
            println!("    - {}", entry.title);
        }
    }
    println!();

    // MEMORY.md 索引
    println!("--- MEMORY.md 索引 ---");
    println!("{}", store.generate_index());

    // YAML frontmatter 示例
    println!("--- YAML Frontmatter 示例 ---");
    let project_entries = store.get_by_type(&MemoryType::Project);
    let sample = project_entries.first().unwrap();
    println!("{}", sample.to_markdown());
}

/// 演示 BM25 搜索
fn demo_bm25_search() {
    println!("=== BM25 搜索演示 ===");
    println!();

    let store = create_demo_memories();
    let all_entries: Vec<&MemoryEntry> = store
        .entries
        .values()
        .flat_map(|v| v.iter())
        .collect();

    let search = Bm25Search::new();

    let queries = vec!["rust 测试", "api 端点", "git commit", "cargo workspace"];

    for query in &queries {
        println!("查询: \"{query}\"");
        let results = search.search(query, &all_entries, 3);
        for (entry, score) in &results {
            println!("  [{:.3}] {} ({})", score, entry.title, entry.memory_type.as_str());
        }
        println!();
    }
}

// ============================================================================
// 主函数
// ============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    println!("=== Ch06: 记忆系统 ===");
    println!("对应 claude-code-book 第 6 章");
    println!();

    demo_memory_system();
    demo_bm25_search();

    Ok(())
}

#[allow(dead_code)]
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len).collect();
        format!("{truncated}...")
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- MemoryType 测试 ----

    #[test]
    fn test_memory_type_as_str() {
        assert_eq!(MemoryType::User.as_str(), "user");
        assert_eq!(MemoryType::Feedback.as_str(), "feedback");
        assert_eq!(MemoryType::Project.as_str(), "project");
        assert_eq!(MemoryType::Reference.as_str(), "reference");
    }

    #[test]
    fn test_memory_type_descriptions() {
        assert!(MemoryType::User.description().contains("用户"));
        assert!(MemoryType::Feedback.description().contains("反馈"));
        assert!(MemoryType::Project.description().contains("项目"));
        assert!(MemoryType::Reference.description().contains("参考"));
    }

    #[test]
    fn test_memory_type_storage_path() {
        let slug = "my-project";
        assert!(MemoryType::User.storage_path(slug).contains("memory/user"));
        assert!(MemoryType::Feedback.storage_path(slug).contains("my-project"));
        assert!(MemoryType::Project.storage_path(slug).contains("my-project"));
        assert!(MemoryType::Reference.storage_path(slug).contains("my-project"));
    }

    // ---- MemoryEntry 测试 ----

    #[test]
    fn test_memory_entry_creation() {
        let entry = MemoryEntry::new(
            MemoryType::Project,
            "测试标题".to_string(),
            "测试内容".to_string(),
            vec!["tag1".to_string(), "tag2".to_string()],
        );
        assert_eq!(entry.memory_type, MemoryType::Project);
        assert_eq!(entry.title, "测试标题");
        assert_eq!(entry.content, "测试内容");
        assert_eq!(entry.tags.len(), 2);
    }

    #[test]
    fn test_memory_entry_to_markdown() {
        let entry = MemoryEntry::new(
            MemoryType::Project,
            "Test Title".to_string(),
            "Test content here.".to_string(),
            vec!["tag1".to_string()],
        );
        let md = entry.to_markdown();
        assert!(md.contains("---"));
        assert!(md.contains("type: project"));
        assert!(md.contains("# Test Title"));
        assert!(md.contains("Test content here."));
    }

    // ---- MemoryStore 测试 ----

    #[test]
    fn test_store_add_and_get() {
        let mut store = MemoryStore::new("test");
        store.add(MemoryEntry::new(
            MemoryType::User,
            "pref".to_string(),
            "content".to_string(),
            vec![],
        ));
        assert_eq!(store.total_count(), 1);
        assert_eq!(store.get_by_type(&MemoryType::User).len(), 1);
        assert_eq!(store.get_by_type(&MemoryType::Project).len(), 0);
    }

    #[test]
    fn test_store_search_by_tag() {
        let mut store = MemoryStore::new("test");
        store.add(MemoryEntry::new(
            MemoryType::Project,
            "Rust 架构".to_string(),
            "使用 workspace".to_string(),
            vec!["rust".to_string(), "architecture".to_string()],
        ));
        store.add(MemoryEntry::new(
            MemoryType::User,
            "终端设置".to_string(),
            "zsh 配置".to_string(),
            vec!["terminal".to_string()],
        ));

        let results = store.search_by_tag("rust");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust 架构");

        let results = store.search_by_tag("terminal");
        assert_eq!(results.len(), 1);

        let results = store.search_by_tag("nonexistent");
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_store_generate_index() {
        let mut store = MemoryStore::new("test-project");
        store.add(MemoryEntry::new(
            MemoryType::User,
            "pref1".to_string(),
            "c1".to_string(),
            vec!["t1".to_string()],
        ));
        store.add(MemoryEntry::new(
            MemoryType::Project,
            "proj1".to_string(),
            "c2".to_string(),
            vec!["t2".to_string()],
        ));

        let index = store.generate_index();
        assert!(index.contains("Memory Index"));
        assert!(index.contains("test-project"));
        assert!(index.contains("user"));
        assert!(index.contains("project"));
        assert!(index.contains("pref1"));
        assert!(index.contains("proj1"));
    }

    #[test]
    fn test_store_total_count() {
        let mut store = MemoryStore::new("test");
        assert_eq!(store.total_count(), 0);

        store.add(MemoryEntry::new(MemoryType::User, "a".to_string(), "".to_string(), vec![]));
        store.add(MemoryEntry::new(MemoryType::Project, "b".to_string(), "".to_string(), vec![]));
        store.add(MemoryEntry::new(MemoryType::Project, "c".to_string(), "".to_string(), vec![]));
        assert_eq!(store.total_count(), 3);
    }

    // ---- BM25 搜索测试 ----

    #[test]
    fn test_tokenize() {
        let tokens = tokenize("Hello World 你好");
        assert!(tokens.contains(&"hello".to_string()));
        assert!(tokens.contains(&"world".to_string()));
        // 中文字符会被保留
        assert!(tokens.contains(&"你好".to_string()));
    }

    #[test]
    fn test_bm25_search_basic() {
        let entries = vec![
            MemoryEntry::new(
                MemoryType::Project,
                "Rust 项目架构".to_string(),
                "使用 cargo workspace 管理多 crate".to_string(),
                vec!["rust".to_string()],
            ),
            MemoryEntry::new(
                MemoryType::Reference,
                "API 端点".to_string(),
                "POST /v1/messages 端点".to_string(),
                vec!["api".to_string()],
            ),
        ];
        let refs: Vec<&MemoryEntry> = entries.iter().collect();
        let search = Bm25Search::new();

        let results = search.search("rust workspace", &refs, 5);
        assert!(!results.is_empty());
        // Rust 相关的条目应该排在前面
        assert!(results[0].0.title.contains("Rust"));
    }

    #[test]
    fn test_bm25_search_empty_query() {
        let entries = vec![MemoryEntry::new(
            MemoryType::User,
            "test".to_string(),
            "content".to_string(),
            vec![],
        )];
        let refs: Vec<&MemoryEntry> = entries.iter().collect();
        let search = Bm25Search::new();

        let results = search.search("", &refs, 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_bm25_search_relevance_order() {
        let entries = vec![
            MemoryEntry::new(
                MemoryType::Project,
                "Rust 测试".to_string(),
                "使用 cargo test 运行单元测试".to_string(),
                vec!["rust".to_string(), "testing".to_string()],
            ),
            MemoryEntry::new(
                MemoryType::User,
                "终端设置".to_string(),
                "使用 zsh 和 starship".to_string(),
                vec!["terminal".to_string()],
            ),
            MemoryEntry::new(
                MemoryType::Reference,
                "API 参考".to_string(),
                "POST /v1/messages 端点".to_string(),
                vec!["api".to_string()],
            ),
        ];
        let refs: Vec<&MemoryEntry> = entries.iter().collect();
        let search = Bm25Search::new();

        let results = search.search("rust 测试", &refs, 5);
        assert!(results.len() >= 1);
        // Rust 相关的条目应该排在最前面
        assert!(results[0].0.title.contains("Rust"));
        // 终端设置和 API 参考不应排在第一
        assert!(!results[0].0.title.contains("终端"));
    }

    #[test]
    fn test_bm25_search_top_k() {
        let entries = vec![
            MemoryEntry::new(MemoryType::Project, "A".to_string(), "rust code".to_string(), vec!["rust".to_string()]),
            MemoryEntry::new(MemoryType::Project, "B".to_string(), "rust code".to_string(), vec!["rust".to_string()]),
            MemoryEntry::new(MemoryType::Project, "C".to_string(), "rust code".to_string(), vec!["rust".to_string()]),
        ];
        let refs: Vec<&MemoryEntry> = entries.iter().collect();
        let search = Bm25Search::new();

        let results = search.search("rust", &refs, 2);
        assert_eq!(results.len(), 2);
    }

    // ---- 完整流程测试 ----

    #[test]
    fn test_demo_memories_creation() {
        let store = create_demo_memories();
        assert_eq!(store.total_count(), 7);
        assert!(!store.get_by_type(&MemoryType::User).is_empty());
        assert!(!store.get_by_type(&MemoryType::Feedback).is_empty());
        assert!(!store.get_by_type(&MemoryType::Project).is_empty());
        assert!(!store.get_by_type(&MemoryType::Reference).is_empty());
    }

    #[test]
    fn test_yaml_frontmatter_roundtrip() {
        let entry = MemoryEntry::new(
            MemoryType::Project,
            "Test".to_string(),
            "Content".to_string(),
            vec!["tag".to_string()],
        );
        let md = entry.to_markdown();
        assert!(md.starts_with("---"));
        assert!(md.contains("type: project"));
        assert!(md.contains("tags:"));
        assert!(md.contains("# Test"));
        assert!(md.contains("Content"));
    }
}
