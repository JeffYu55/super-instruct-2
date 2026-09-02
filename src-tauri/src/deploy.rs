// Deploy 模块 — Codex config.toml 备份/修改/恢复

use regex::Regex;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct DeployManager {
    codex_home: PathBuf,
}

/// 本项目部署 skill 的清单文件名（记录本次部署到 ~/.codex/skills/ 的 id）
const SKILLS_MANIFEST: &str = "super-instruct-skills-deployed.json";
/// 被项目覆盖的同名用户 skill 备份目录
const SKILLS_BACKUP_DIR: &str = "super-instruct-skills-backup";

#[derive(Clone, Serialize)]
pub struct DeployStatus {
    pub bridge_active: bool,
    pub bridge_exists: bool,
    pub skills_count: usize,
    pub config_backed_up: bool,
    pub relay_url_valid: bool,
    pub codex_home_found: bool,
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

impl DeployManager {
    /// 查找 Codex 配置目录
    pub fn find_codex_home() -> Option<PathBuf> {
        // CODEX_HOME 环境变量
        if let Ok(home) = std::env::var("CODEX_HOME") {
            let p = PathBuf::from(home);
            if p.join("config.toml").exists() {
                return Some(p);
            }
        }
        // ~/.codex
        if let Some(home) = home_dir() {
            let codex = home.join(".codex");
            if codex.join("config.toml").exists() {
                return Some(codex);
            }
        }
        None
    }

    pub fn new() -> Option<Self> {
        Self::find_codex_home().map(|codex_home| Self { codex_home })
    }

    pub fn codex_home(&self) -> &Path {
        &self.codex_home
    }

    /// Re-apply the local proxy settings when another Codex process rewrites
    /// config.toml while the proxy is running. The rewritten user config is
    /// kept as the clean backup so shutdown still restores the latest version.
    pub fn ensure_active_config(&self, proxy_url: &str) -> Result<bool, String> {
        let cfg = self.codex_home.join("config.toml");
        let bak = self.codex_home.join("config.toml.super-instruct-bak");
        let relay_file = self.codex_home.join("relay_url.txt");
        let content = fs::read_to_string(&cfg).map_err(|e| format!("read config failed: {e}"))?;
        let base_url_re = Regex::new(r#"base_url\s*=\s*"([^"]+)""#).unwrap();
        let current_url = base_url_re
            .captures(&content)
            .and_then(|captures| captures.get(1))
            .map(|value| value.as_str());
        let instructions_active = content
            .lines()
            .any(|line| line.trim() == r#"model_instructions_file = "./bridge.md""#);
        let proxy_active = current_url == Some(proxy_url);

        if proxy_active && instructions_active {
            return Ok(false);
        }

        // A non-proxy URL represents a new clean user configuration. Preserve
        // it before patching and use it as the upstream relay destination.
        if let Some(url) = current_url.filter(|url| *url != proxy_url) {
            fs::copy(&cfg, &bak).map_err(|e| format!("backup drifted config failed: {e}"))?;
            fs::write(&relay_file, url).map_err(|e| format!("update relay URL failed: {e}"))?;
        }

        let mut modified = if base_url_re.is_match(&content) {
            base_url_re
                .replace_all(&content, format!(r#"base_url = "{proxy_url}""#))
                .into_owned()
        } else {
            format!("{content}\nbase_url = \"{proxy_url}\"\n")
        };
        let instructions_re = Regex::new(r#"model_instructions_file\s*=\s*"[^"]*""#).unwrap();
        if instructions_re.is_match(&modified) {
            modified = instructions_re
                .replace_all(&modified, r#"model_instructions_file = "./bridge.md""#)
                .into_owned();
        } else {
            modified.push_str("\nmodel_instructions_file = \"./bridge.md\"\n");
        }
        fs::write(&cfg, modified).map_err(|e| format!("repair config failed: {e}"))?;
        Ok(true)
    }

    /// 部署 bridge.md + skills 到 Codex，修改 base_url 指向代理
    pub fn apply(&self, bridge_md: &str, skills_dir: &Path) -> Result<String, String> {
        self.apply_with_optional_skills(bridge_md, Some(skills_dir))
    }

    /// 部署 bridge.md 到 Codex，skills 可选。修改 base_url 指向代理
    pub fn apply_with_optional_skills(
        &self,
        bridge_md: &str,
        skills_dir: Option<&Path>,
    ) -> Result<String, String> {
        let cfg = self.codex_home.join("config.toml");
        let bak = self.codex_home.join("config.toml.super-instruct-bak");
        let relay_file = self.codex_home.join("relay_url.txt");

        tracing::info!("deploy: codex_home = {}", self.codex_home.display());

        // 1. 读取当前 config.toml，提取 base_url
        let content = fs::read_to_string(&cfg).map_err(|e| format!("read config failed: {}", e))?;
        let re = Regex::new(r#"base_url\s*=\s*"([^"]+)""#).unwrap();

        // 2. 保存真实中转站地址到 relay_url.txt（只要当前 base_url 不是代理地址）
        if let Some(caps) = re.captures(&content) {
            let current_url = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            if !current_url.contains("127.0.0.1:8080") {
                fs::write(&relay_file, current_url)
                    .map_err(|e| format!("write relay_url.txt failed: {}", e))?;
                tracing::info!("deploy: relay_url.txt saved: {}", current_url);
            }
        }

        // 3. 备份 config.toml — 仅当当前配置未指向代理时备份，避免备份污染
        // 如果当前 config.toml 已包含 127.0.0.1:8080，说明上一次 deploy 的备份可能还在，
        // 或者上次未正常退出。此时不覆盖备份，保留已有的干净版本。
        let already_patched = content.contains("127.0.0.1:8080");
        if !already_patched {
            fs::copy(&cfg, &bak).map_err(|e| format!("backup failed: {}", e))?;
            tracing::info!("deploy: backed up config.toml -> config.toml.super-instruct-bak");
        } else if bak.exists() {
            tracing::info!("deploy: config already patched, keeping existing clean backup");
        } else {
            // 已 patched 但无备份 — 极端情况：从 relay_url.txt 恢复 base_url 后再备份
            tracing::warn!(
                "deploy: config patched but no backup found, restoring base_url from relay_url.txt"
            );
            if let Ok(relay_content) = fs::read_to_string(&relay_file) {
                let relay_url = relay_content.trim();
                if !relay_url.is_empty() {
                    let restored =
                        re.replace_all(&content, format!(r#"base_url = "{}""#, relay_url));
                    fs::write(&cfg, restored.as_ref())
                        .map_err(|e| format!("restore base_url failed: {}", e))?;
                    fs::copy(&cfg, &bak).map_err(|e| format!("backup failed: {}", e))?;
                    tracing::info!("deploy: base_url restored from relay_url.txt, then backed up");
                }
            }
        }

        // 4. 修改 base_url + 补入 model_instructions_file
        let modified = re.replace_all(&content, r#"base_url = "http://127.0.0.1:8080""#);

        // model_instructions_file: 若已存在则替换，否则在 model = 行后插入，都没有则追加
        let instructions_line = r#"model_instructions_file = "./bridge.md""#;
        let final_config = if modified.contains("model_instructions_file") {
            let re2 = Regex::new(r#"model_instructions_file\s*=\s*"[^"]*""#).unwrap();
            re2.replace_all(&modified, instructions_line).into_owned()
        } else if modified.contains("model")
            && modified
                .lines()
                .any(|l| l.trim_start().starts_with("model"))
        {
            // 在 model = 行之后插入
            let mut lines = modified.lines().collect::<Vec<_>>();
            let mut inserted = false;
            for i in 0..lines.len() {
                if lines[i].trim_start().starts_with("model") {
                    lines.insert(i + 1, instructions_line);
                    inserted = true;
                    break;
                }
            }
            if inserted {
                lines.join("\n")
            } else {
                format!("{}\n{}", modified, instructions_line)
            }
        } else {
            format!("{}\n{}", modified, instructions_line)
        };

        fs::write(&cfg, &final_config).map_err(|e| format!("write config failed: {}", e))?;
        tracing::info!("deploy: base_url patched + model_instructions_file set");

        // 5. 复制 bridge.md
        let dst_bridge = self.codex_home.join("bridge.md");
        fs::write(&dst_bridge, bridge_md).map_err(|e| format!("write bridge.md failed: {}", e))?;
        tracing::info!("deploy: bridge.md written ({} bytes)", bridge_md.len());

        // 6. 部署 skills (可选, 只部署启用的) — 合并式管理
        //    * 绝不删除用户自定义的其他 skill（保留 ~/.codex/skills/ 共享目录语义）
        //    * 只管理本项目本次部署的 id（写入 manifest），供 restore 精确清理
        //    * 部署前对同名用户 skill 先备份，避免覆盖丢失
        let skill_count = if let Some(skills_dir) = skills_dir {
            let dst_skills = self.codex_home.join("skills");
            let manifest_file = self.codex_home.join(SKILLS_MANIFEST);
            let backup_dir = self.codex_home.join(SKILLS_BACKUP_DIR);
            let prefs_path = self.codex_home.join("super-instruct-skills.json");

            // 上次部署清单 — 先清理上次部署的 id，并还原用户同名备份，保证可重复部署
            let previous: std::collections::BTreeSet<String> = read_skills_manifest(&manifest_file);
            for id in &previous {
                let dst = dst_skills.join(id);
                if dst.exists() {
                    let _ = fs::remove_dir_all(&dst);
                }
                let bak = backup_dir.join(id);
                if bak.exists() {
                    if let Err(e) = copy_dir_recursive(&bak, &dst) {
                        tracing::warn!("deploy: restore user skill '{}' failed: {}", id, e);
                    }
                    let _ = fs::remove_dir_all(&bak);
                }
            }
            if backup_dir.exists() {
                let _ = fs::remove_dir_all(&backup_dir);
            }
            fs::create_dir_all(&dst_skills)
                .map_err(|e| format!("create skills dir failed: {}", e))?;

            // 读取启用列表
            let enabled_set: Option<std::collections::BTreeSet<String>> = if prefs_path.exists() {
                fs::read_to_string(&prefs_path)
                    .ok()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                    .and_then(|v| v.get("enabled").cloned())
                    .and_then(|e| serde_json::from_value(e).ok())
            } else {
                None // 无偏好文件 = 全部启用
            };

            // None = 全开; Some(空集) = 全关（用户显式禁用了所有）
            let all_enabled = enabled_set.is_none();
            let mut deployed: std::collections::BTreeSet<String> = Default::default();
            let mut count = 0;
            if let Ok(entries) = fs::read_dir(skills_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if !path.is_dir() {
                        continue;
                    }
                    let id = entry.file_name().to_string_lossy().to_string();
                    let should_copy = if all_enabled {
                        true
                    } else {
                        enabled_set.as_ref().is_some_and(|s| s.contains(&id))
                    };
                    if !should_copy {
                        continue;
                    }
                    let dst = dst_skills.join(&id);
                    // 目标已存在同名目录且非上次项目部署 → 视为用户自定义 skill，先备份再覆盖
                    if dst.exists() && !previous.contains(&id) {
                        fs::create_dir_all(&backup_dir)
                            .map_err(|e| format!("create backup dir failed: {}", e))?;
                        let user_backup = backup_dir.join(&id);
                        let _ = fs::remove_dir_all(&user_backup);
                        if let Err(e) = fs::rename(&dst, &user_backup) {
                            tracing::warn!("deploy: backup user skill '{}' failed: {}", id, e);
                        }
                    }
                    copy_dir_recursive(&path, &dst)
                        .map_err(|e| format!("copy skill '{}' failed: {}", id, e))?;
                    deployed.insert(id);
                    count += 1;
                }
            }

            // 记录本次部署的 id —— restore 时据此清理，不影响用户其他 skill
            if let Err(e) = write_skills_manifest(&manifest_file, &deployed) {
                tracing::warn!("deploy: manifest write failed: {}", e);
            }
            count
        } else {
            tracing::warn!("deploy: skills dir not provided, skipping skills");
            0
        };
        tracing::info!("deploy: {} skills deployed", skill_count);
        Ok(format!("bridge.md + {} skills deployed", skill_count))
    }

    /// 从备份恢复 Codex 配置
    pub fn restore(&self) -> Result<String, String> {
        let cfg = self.codex_home.join("config.toml");
        let bak = self.codex_home.join("config.toml.super-instruct-bak");

        tracing::info!("restore: codex_home = {}", self.codex_home.display());

        if bak.exists() {
            fs::copy(&bak, &cfg).map_err(|e| format!("restore config failed: {}", e))?;
            fs::remove_file(&bak).map_err(|e| format!("remove backup failed: {}", e))?;
            tracing::info!("restore: config.toml restored from backup");
        } else {
            tracing::warn!("restore: no backup found, config.toml unchanged");
        }

        let bridge = self.codex_home.join("bridge.md");
        if bridge.exists() {
            let _ = fs::remove_file(&bridge);
            tracing::info!("restore: bridge.md removed");
        }

        // 反向 skill 部署：只移除本项目部署的 id，还原用户同名备份；保留用户其他自定义 skill
        let skills = self.codex_home.join("skills");
        let manifest_file = self.codex_home.join(SKILLS_MANIFEST);
        let backup_dir = self.codex_home.join(SKILLS_BACKUP_DIR);

        let deployed = read_skills_manifest(&manifest_file);
        let mut skills_restored = 0usize;
        for id in &deployed {
            let dst = skills.join(id);
            if dst.exists() {
                let _ = fs::remove_dir_all(&dst);
            }
            let bak = backup_dir.join(id);
            if bak.exists() {
                if copy_dir_recursive(&bak, &dst).is_ok() {
                    skills_restored += 1;
                }
                let _ = fs::remove_dir_all(&bak);
            }
        }
        if backup_dir.exists() {
            let _ = fs::remove_dir_all(&backup_dir);
        }
        if manifest_file.exists() {
            let _ = fs::remove_file(&manifest_file);
        }
        if skills_restored > 0 {
            tracing::info!("restore: restored {} user skill(s)", skills_restored);
        } else {
            tracing::info!("restore: no user skills to restore");
        }

        Ok("Codex config restored".to_string())
    }

    /// 设置中转站地址（写入 relay_url.txt，如果 config.toml 存在则同步更新）
    pub fn set_relay_url(&self, url: &str) -> Result<String, String> {
        let relay_file = self.codex_home.join("relay_url.txt");
        fs::write(&relay_file, url).map_err(|e| format!("write relay_url.txt failed: {}", e))?;
        tracing::info!("set_relay_url: relay_url.txt saved: {}", url);

        // 如果 config.toml 存在且当前 base_url 不是代理地址，同步更新
        let cfg = self.codex_home.join("config.toml");
        if cfg.exists() {
            let content =
                fs::read_to_string(&cfg).map_err(|e| format!("read config failed: {}", e))?;
            // 只有当 base_url 不指向本地代理时才同步（避免覆盖正在运行的代理配置）
            if !content.contains("127.0.0.1:8080") {
                let re = Regex::new(r#"base_url\s*=\s*"[^"]*""#).unwrap();
                let modified = re.replace_all(&content, format!(r#"base_url = "{}""#, url));
                fs::write(&cfg, modified.as_ref())
                    .map_err(|e| format!("write config failed: {}", e))?;
                tracing::info!("set_relay_url: config.toml base_url updated to {}", url);
            } else {
                tracing::info!("set_relay_url: proxy active, config.toml not modified");
            }
        }

        Ok(format!("Relay URL saved: {}", url))
    }

    pub fn status(&self) -> DeployStatus {
        let cfg = self.codex_home.join("config.toml");
        let bridge = self.codex_home.join("bridge.md");
        let skills = self.codex_home.join("skills");
        let bak = self.codex_home.join("config.toml.super-instruct-bak");
        let relay_file = self.codex_home.join("relay_url.txt");

        let bridge_active = cfg.exists() && {
            let content = fs::read_to_string(&cfg).unwrap_or_default();
            content.contains("127.0.0.1:8080")
        };

        let relay_url_valid = relay_file.exists() && {
            let content = fs::read_to_string(&relay_file).unwrap_or_default();
            let url = content.trim();
            !url.is_empty() && !url.contains("127.0.0.1:8080")
        };

        DeployStatus {
            bridge_active,
            bridge_exists: bridge.exists(),
            skills_count: if skills.exists() {
                count_skills(&skills)
            } else {
                0
            },
            config_backed_up: bak.exists(),
            relay_url_valid,
            codex_home_found: true,
        }
    }
}

/// 读取中转站地址（优先级: relay_url.txt > config.toml 备份 > config.toml 当前）
pub fn find_relay_url() -> Option<String> {
    let home = DeployManager::find_codex_home()?;

    // 1. 优先读 relay_url.txt（用户显式设置的，或部署时自动保存的）
    let relay_file = home.join("relay_url.txt");
    if relay_file.exists() {
        if let Ok(content) = fs::read_to_string(&relay_file) {
            let url = content.trim();
            if !url.is_empty() && !url.contains("127.0.0.1:8080") {
                return Some(url.to_string());
            }
        }
    }

    // 2. 从 config.toml 备份读取（部署前的原始地址）
    let bak = home.join("config.toml.super-instruct-bak");
    if bak.exists() {
        if let Ok(content) = fs::read_to_string(&bak) {
            let re = Regex::new(r#"base_url\s*=\s*"([^"]+)""#).ok()?;
            if let Some(caps) = re.captures(&content) {
                let url = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                if !url.contains("127.0.0.1:8080") {
                    return Some(url.to_string());
                }
            }
        }
    }

    // 3. 从当前 config.toml 读取（排除代理自身地址，防自环）
    let cfg = home.join("config.toml");
    if let Ok(content) = fs::read_to_string(&cfg) {
        let re = Regex::new(r#"base_url\s*=\s*"([^"]+)""#).ok()?;
        if let Some(caps) = re.captures(&content) {
            let url = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            if !url.is_empty() && !url.contains("127.0.0.1:8080") {
                return Some(url.to_string());
            }
        }
    }

    None
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dest = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &dest)?;
        } else {
            fs::copy(&path, &dest)?;
        }
    }
    Ok(())
}

fn count_skills(dir: &Path) -> usize {
    let mut count = 0;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                count += count_skills(&path);
            } else if path.file_name().map(|n| n == "SKILL.md").unwrap_or(false) {
                count += 1;
            }
        }
    }
    count
}

/// 读取部署清单（本次部署到 ~/.codex/skills/ 的 skill id），失败返回空集
fn read_skills_manifest(path: &Path) -> std::collections::BTreeSet<String> {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<std::collections::BTreeSet<String>>(&s).ok())
        .unwrap_or_default()
}

/// 写入本次部署的 skill id 清单
fn write_skills_manifest(
    path: &Path,
    ids: &std::collections::BTreeSet<String>,
) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(ids).unwrap_or_else(|_| "[]".to_string());
    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, json)?;
    fs::rename(tmp_path, path)
}
