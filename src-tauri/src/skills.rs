// Skills 管理模块 — 扫描源 skills、解析元数据、管理启用状态
//
// 数据流:
//   codex-skills/ (源) → scan_skills() → Vec<SkillInfo>
//   skills.json (持久化启用状态) → load_prefs() / save_prefs()
//   deploy 时 → get_enabled_skills() → 只复制启用的

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::deploy::DeployManager;
use tauri::Manager;

#[derive(Clone, Serialize, Deserialize)]
pub struct SkillInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub file_count: usize,
}

#[derive(Clone, Serialize, Deserialize, Default)]
struct SkillsPrefs {
    /// 启用的 skill id 集合；不存在 = 全部启用（首次运行默认全开）
    enabled: Option<BTreeSet<String>>,
}

// ── 路径解析 ──────────────────────────────────

fn prefs_path() -> Option<PathBuf> {
    let home = DeployManager::find_codex_home()?;
    Some(home.join("super-instruct-skills.json"))
}

fn source_skills_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    // 复用 lib.rs 的 resolve_resource_dir 逻辑
    if let Ok(base) = app.path().resource_dir() {
        let p = base.join("codex-skills");
        if p.is_dir() {
            return Some(p);
        }
    }
    // Dev fallback: 项目根/codex-skills
    let candidates = [
        std::path::PathBuf::from("codex-skills"),
        std::env::current_dir().ok()?.join("codex-skills"),
        std::env::current_dir().ok()?.parent()?.join("codex-skills"),
    ];
    for p in &candidates {
        if p.is_dir() {
            return Some(p.clone());
        }
    }
    None
}

// ── 元数据解析 ────────────────────────────────

/// 从 SKILL.md frontmatter 提取 name 和 description
fn parse_skill_metadata(skill_dir: &Path) -> (String, String) {
    let skill_md = skill_dir.join("SKILL.md");
    let content = fs::read_to_string(&skill_md).unwrap_or_default();

    let mut name = skill_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut description = String::new();

    // 解析 YAML frontmatter (--- ... ---)
    if content.starts_with("---") {
        let end = content[3..].find("---");
        if let Some(end_pos) = end {
            let frontmatter = &content[3..3 + end_pos];
            for line in frontmatter.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("name:") {
                    name = rest.trim().trim_matches('"').to_string();
                } else if let Some(rest) = line.strip_prefix("description:") {
                    description = rest.trim().trim_matches('"').to_string();
                }
            }
        }
    }

    (name, description)
}

/// 递归统计目录下文件数
fn count_files(dir: &Path) -> usize {
    let mut count = 0;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                count += count_files(&path);
            } else {
                count += 1;
            }
        }
    }
    count
}

// ── 偏好读写 ──────────────────────────────────

fn load_prefs() -> SkillsPrefs {
    let Some(path) = prefs_path() else {
        return SkillsPrefs::default();
    };
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_prefs(prefs: &SkillsPrefs) {
    let Some(path) = prefs_path() else { return };
    if let Ok(json) = serde_json::to_string_pretty(prefs) {
        let _ = fs::write(&path, json);
    }
}

// ── 公开 API ──────────────────────────────────

/// 扫描源 skills 目录，返回所有 skill 信息 + 启用状态
/// 首次运行（enabled=None）时自动初始化为全启用并持久化
pub fn scan_skills(app: &tauri::AppHandle) -> Vec<SkillInfo> {
    let Some(src_dir) = source_skills_dir(app) else {
        tracing::warn!("skills: source codex-skills dir not found");
        return Vec::new();
    };

    let prefs = load_prefs();

    // 收集所有 skill id
    let mut all_ids: Vec<String> = Vec::new();
    if let Ok(entries) = fs::read_dir(&src_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                all_ids.push(entry.file_name().to_string_lossy().to_string());
            }
        }
    }

    // 首次运行（enabled=None）：初始化为全启用集合并持久化
    let enabled_set: BTreeSet<String> = match &prefs.enabled {
        None => {
            let set: BTreeSet<String> = all_ids.iter().cloned().collect();
            save_prefs(&SkillsPrefs { enabled: Some(set.clone()) });
            set
        }
        Some(s) => s.clone(),
    };

    let mut skills: Vec<SkillInfo> = Vec::new();

    for id in &all_ids {
        let path = src_dir.join(id);
        let (name, description) = parse_skill_metadata(&path);
        let file_count = count_files(&path);
        let enabled = enabled_set.contains(id);

        skills.push(SkillInfo {
            id: id.clone(),
            name,
            description,
            enabled,
            file_count,
        });
    }

    // 按名称排序
    skills.sort_by(|a, b| a.id.cmp(&b.id));
    skills
}

/// 切换单个 skill 的启用状态
pub fn toggle_skill(id: &str, enabled: bool) {
    let mut prefs = load_prefs();
    if prefs.enabled.is_none() {
        // 从全开模式切换到显式列表：不初始化全量集合
        // 语义：enabled=None 表示"无偏好"，一旦用户显式 toggle，
        // 就进入显式模式。前端在首次 toggle 前应先调用 set_all(true)
        // 来建立完整偏好列表，避免其余 skill 丢失。
        prefs.enabled = Some(BTreeSet::new());
    }
    if let Some(ref mut set) = prefs.enabled {
        if enabled {
            set.insert(id.to_string());
        } else {
            set.remove(id);
        }
    }
    save_prefs(&prefs);
}

/// 批量设置启用状态
pub fn set_enabled(ids: &[String], enabled: bool) {
    let mut prefs = load_prefs();
    if prefs.enabled.is_none() {
        prefs.enabled = Some(BTreeSet::new());
    }
    if let Some(ref mut set) = prefs.enabled {
        for id in ids {
            if enabled {
                set.insert(id.clone());
            } else {
                set.remove(id);
            }
        }
    }
    save_prefs(&prefs);
}

/// 全部启用 / 全部禁用
pub fn set_all(enabled: bool) {
    let prefs = if enabled {
        SkillsPrefs {
            enabled: None, // None = 全开
        }
    } else {
        SkillsPrefs {
            enabled: Some(BTreeSet::new()), // 空集 = 全关
        }
    };
    save_prefs(&prefs);
}

/// 返回当前启用的 skill id 列表（供 deploy 使用）
/// 需要传入 app 来扫描源目录
pub fn get_enabled_skill_ids(app: &tauri::AppHandle) -> BTreeSet<String> {
    let skills = scan_skills(app);
    skills
        .into_iter()
        .filter(|s| s.enabled)
        .map(|s| s.id)
        .collect()
}

/// 删除源 skills 目录中的某个 skill
pub fn delete_skill(app: &tauri::AppHandle, id: &str) -> Result<(), String> {
    let Some(src_dir) = source_skills_dir(app) else {
        return Err("源 skills 目录未找到".into());
    };
    let skill_dir = src_dir.join(id);
    if !skill_dir.exists() {
        return Err(format!("Skill '{}' 不存在", id));
    }
    fs::remove_dir_all(&skill_dir).map_err(|e| format!("删除失败: {}", e))?;

    // 从偏好中移除
    let mut prefs = load_prefs();
    if let Some(ref mut set) = prefs.enabled {
        set.remove(id);
    }
    save_prefs(&prefs);

    tracing::info!("skills: deleted skill '{}'", id);
    Ok(())
}